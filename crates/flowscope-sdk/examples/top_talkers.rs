// SPDX-FileCopyrightText: 2026 FlowScope contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Print the top source AS of the last hours.
//!
//! ```console
//! cargo run -p rinjani-flowscope-sdk --example top_talkers -- http://127.0.0.1:8080 2026-10-10T00:00:00Z 2026-10-10T06:00:00Z
//! ```

use flowscope_sdk::Client;
use flowscope_sdk::filter::col;
use flowscope_sdk::models::{GraphInput, LineInput, Units};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(url), Some(start), Some(end)) = (args.next(), args.next(), args.next()) else {
        eprintln!("usage: top_talkers <console URL> <start> <end>");
        std::process::exit(2);
    };
    let client = Client::new(&url)?;
    println!("FlowScope {}", client.configuration().await?.version);

    let filter = col("InIfBoundary").eq_keyword("external");
    let check = client.filter_validate(&filter.to_string()).await?;
    if !check.is_valid() {
        return Err(format!("the server refused the filter: {}", check.message).into());
    }

    let graph = client
        .graph_line(&LineInput {
            graph: GraphInput {
                start,
                end,
                dimensions: vec!["SrcAS".into()],
                limit: 10,
                filter: filter.into(),
                units: Units::L3bps,
                ..Default::default()
            },
            points: 100,
            ..Default::default()
        })
        .await?;
    for (row, average) in graph.rows.iter().zip(&graph.average) {
        println!("{:>12.1} Mbps  {}", *average as f64 / 1e6, row.join(", "));
    }
    Ok(())
}
