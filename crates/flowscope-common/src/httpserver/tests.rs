// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::config::{decode, parse_str};
use axum::routing::{get, post};

async fn serve(server: &HttpServer) -> (Daemon, String) {
    let daemon = Daemon::new();
    server.start(&daemon).await.unwrap();
    let addr = server.local_addr().unwrap();
    (daemon, format!("http://127.0.0.1:{}", addr.port()))
}

fn config() -> Configuration {
    Configuration { listen: "127.0.0.1:0".into(), ..Default::default() }
}

#[tokio::test]
async fn handlers_and_metrics() {
    let r = Reporter::new_mock();
    let server = HttpServer::new(&r, "test", config()).await.unwrap();
    server.add_api(Router::new().route("/api/v0/test/hello", get(|| async { "hello" })));
    let (metrics, api) = common_routes(&r, "test", "v1.0");
    server.add_handler("/api/v0/metrics", metrics);
    server.add_api(api);
    let (daemon, base) = serve(&server).await;

    let body = reqwest::get(format!("{base}/api/v0/test/hello")).await.unwrap().text().await.unwrap();
    assert_eq!(body, "hello");
    let resp = reqwest::get(format!("{base}/api/v0/healthcheck")).await.unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "{\"status\":\"ok\"}\n");
    let resp = reqwest::get(format!("{base}/api/v0/nothing")).await.unwrap();
    assert_eq!(resp.status(), 404);

    let metrics = reqwest::get(format!("{base}/api/v0/metrics")).await.unwrap().text().await.unwrap();
    assert!(
        metrics.contains("flowscope_common_httpserver_requests_total{code=\"200\",handler=\"/api/\",method=\"get\"} 2"),
        "{metrics}"
    );
    daemon.terminate();
    daemon.join().await;
}

#[tokio::test]
async fn cache_by_body() {
    let r = Reporter::new_mock();
    let server = HttpServer::new(&r, "test", config()).await.unwrap();
    let counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let c = counter.clone();
    let handler = move |body: String| {
        let c = c.clone();
        async move {
            let n = c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            json_response(StatusCode::OK, &serde_json::json!({"body": body, "n": n}))
        }
    };
    server.add_api(
        Router::new().route("/api/v0/test/cached", post(handler).layer(server.cache_by_body(Duration::from_secs(60)))),
    );
    let (daemon, base) = serve(&server).await;
    let client = reqwest::Client::new();
    let mut answers = vec![];
    for body in ["a", "a", "b", "a"] {
        let resp = client.post(format!("{base}/api/v0/test/cached")).body(body).send().await.unwrap();
        assert_eq!(resp.headers()["content-type"], "application/json; charset=utf-8");
        answers.push(resp.text().await.unwrap());
    }
    assert_eq!(
        answers,
        vec![
            "{\"body\":\"a\",\"n\":0}\n",
            "{\"body\":\"a\",\"n\":0}\n",
            "{\"body\":\"b\",\"n\":1}\n",
            "{\"body\":\"a\",\"n\":0}\n",
        ]
    );
    let metrics = r.render_metrics();
    assert!(
        metrics.contains("flowscope_common_httpserver_cache_hit_total{method=\"POST\",path=\"/api/v0/test/cached\"} 2"),
        "{metrics}"
    );
    daemon.terminate();
    daemon.join().await;
}

#[test]
fn configuration() {
    let got: Configuration = decode(
        parse_str("listen: 127.0.0.1:8081\ncache:\n  type: redis\n  server: localhost:6379\n  DB: 2\n").unwrap(),
        "x",
        vec![],
    )
    .unwrap();
    assert_eq!(
        got,
        Configuration {
            listen: "127.0.0.1:8081".into(),
            profiler: true,
            cache: CacheConfiguration::Redis(RedisCacheConfiguration {
                server: "localhost:6379".into(),
                db: 2,
                ..Default::default()
            }),
        }
    );
    let dumped = crate::config::dump(&got).unwrap();
    assert!(dumped.contains("type: redis"), "{dumped}");
    let err = decode::<Configuration>(parse_str("cache:\n  type: nope\n").unwrap(), "x", vec![]).unwrap_err();
    assert!(err.to_string().contains("\"nope\" is not a known type"), "{err}");
    let err = decode::<Configuration>(parse_str("listen: nope\n").unwrap(), "x", vec![]).unwrap_err();
    assert!(err.to_string().contains("listen"), "{err}");
}
