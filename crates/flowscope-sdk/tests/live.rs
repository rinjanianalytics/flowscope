// SPDX-FileCopyrightText: 2026 FlowScope contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Call every method of the client on a running console holding flows of
//! the last hours:
//!
//! ```console
//! FLOWSCOPE_TEST_CONSOLE_URL=http://127.0.0.1:8080 cargo test -p flowscope-sdk --test live -- --ignored
//! ```

#![cfg(feature = "client")]

use flowscope_sdk::filter::col;
use flowscope_sdk::models::*;
use flowscope_sdk::{Client, Error};

fn seconds_ago(seconds: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    // The API takes RFC 3339 dates. Build one without a date library.
    let t = now - seconds;
    let (days, rest) = (t / 86400, t % 86400);
    let (mut year, mut day) = (1970u64, days);
    let leap = |y: u64| (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400);
    while day >= if leap(year) { 366 } else { 365 } {
        day -= if leap(year) { 366 } else { 365 };
        year += 1;
    }
    let lengths = [31, if leap(year) { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 0;
    while day >= lengths[month] {
        day -= lengths[month];
        month += 1;
    }
    format!("{year:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", month + 1, day + 1, rest / 3600, rest % 3600 / 60, rest % 60)
}

#[tokio::test]
#[ignore = "needs a running console"]
async fn every_method() {
    let url = std::env::var("FLOWSCOPE_TEST_CONSOLE_URL").expect("FLOWSCOPE_TEST_CONSOLE_URL");
    let client = Client::new(&url).unwrap();

    let config = client.configuration().await.unwrap();
    assert!(!config.version.is_empty());
    assert!(config.dimensions.contains(&"SrcAS".to_string()));
    let user = client.user_info().await.unwrap();
    assert!(!user.login.is_empty());
    assert!(client.user_avatar().await.unwrap().starts_with(b"\x89PNG"));
    let openapi = client.openapi().await.unwrap();
    assert_eq!(openapi, serde_json::from_str::<serde_json::Value>(&flowscope_sdk::openapi::json()).unwrap());

    // Widgets
    let flow = client.widget_flow_last().await.unwrap().expect("a flow in the database");
    assert!(flow.contains_key("TimeReceived"));
    let rate = client.widget_flow_rate().await.unwrap();
    assert_eq!(rate.period, "second");
    assert!(rate.rate > 0.0);
    assert!(!client.widget_exporters().await.unwrap().is_empty());
    assert!(!client.widget_graph().await.unwrap().is_empty());
    for name in &config.homepage_top_widgets {
        assert!(!client.widget_top(name).await.unwrap().is_empty(), "top {name}");
    }
    assert_eq!(client.widget_top("nothing").await.unwrap_err().status(), Some(400));

    // Filters
    let filter = col("InIfBoundary").eq_keyword("external").and(col("SrcAS").ne(64512));
    let check = client.filter_validate(&filter.to_string()).await.unwrap();
    assert!(check.is_valid(), "{check:?}");
    let check = client.filter_validate("SrcAS = ").await.unwrap();
    assert!(!check.is_valid());
    assert!(!check.errors.is_empty());
    let completions = client
        .filter_complete(&CompleteInput {
            what: CompleteWhat::Column,
            prefix: Some("Src".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(completions.iter().any(|c| c.label == "SrcAS"));
    let completions = client
        .filter_complete(&CompleteInput {
            what: CompleteWhat::Operator,
            column: Some("SrcAS".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(completions.iter().any(|c| c.label == "="));

    // Graphs
    let graph = GraphInput {
        start: seconds_ago(6 * 3600),
        end: seconds_ago(0),
        dimensions: vec!["SrcAS".into(), "InIfProvider".into()],
        limit: 5,
        filter: filter.to_string(),
        units: Units::L3bps,
        bidirectional: true,
        ..Default::default()
    };
    let line =
        client.graph_line(&LineInput { graph: graph.clone(), points: 100, previous_period: true }).await.unwrap();
    assert!(!line.rows.is_empty());
    assert_eq!(line.rows.len(), line.points.len());
    assert_eq!(line.rows.len(), line.axis.len());
    assert_eq!(line.points[0].len(), line.t.len());
    assert!(line.axis.contains(&2), "the reverse direction is there");
    assert_eq!(line.axis_names.get("1").map(String::as_str), Some("Direct"));
    let sankey = client.graph_sankey(&graph).await.unwrap();
    assert!(!sankey.links.is_empty());
    let refused = client.graph_sankey(&GraphInput { dimensions: vec![], ..graph.clone() }).await.unwrap_err();
    assert!(matches!(&refused, Error::Api { status: 400, message } if message.contains("dimension")), "{refused}");
    let interval = client
        .graph_table_interval(&TableIntervalInput { start: graph.start.clone(), end: graph.end.clone(), points: 100 })
        .await
        .unwrap();
    assert!(interval.table.starts_with("flows"));
    assert!(interval.interval > 0);
    let reversed = client
        .graph_reverse(&Reverse { dimensions: graph.dimensions.clone(), filter: "SrcAS = 1".into() })
        .await
        .unwrap();
    assert_eq!(reversed.dimensions, ["DstAS", "OutIfProvider"]);
    assert_eq!(reversed.filter, "DstAS = 1");

    // Saved filters
    let description = format!("sdk live test {}", seconds_ago(0));
    client
        .add_saved_filter(&SavedFilterInput {
            description: description.clone(),
            content: filter.to_string(),
            shared: false,
        })
        .await
        .unwrap();
    let saved = client.saved_filters().await.unwrap();
    let mine = saved.iter().find(|f| f.description == description).expect("the filter just saved");
    assert_eq!(mine.content, filter.to_string());
    client.delete_saved_filter(mine.id).await.unwrap();
    assert_eq!(client.delete_saved_filter(mine.id).await.unwrap_err().status(), Some(404));

    // Documentation
    let document = client.document("intro").await.unwrap();
    assert!(document.markdown.contains("<h1"));
    assert!(document.toc.iter().any(|d| d.name == "intro"));
    assert!(client.document_markdown("intro").await.unwrap().starts_with("# "));
    assert_eq!(client.document("nothing").await.unwrap_err().status(), Some(404));
}
