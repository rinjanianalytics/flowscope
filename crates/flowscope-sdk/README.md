# rinjani-flowscope-sdk

Client library for the console API of [FlowScope][], the flow collector of
Rinjani Analytics. FlowScope is a Rust rewrite of [Akvorado][].

The package is `rinjani-flowscope-sdk`. The library is `flowscope_sdk`.

```toml
[dependencies]
rinjani-flowscope-sdk = "2.0.0-rs.0"
```

The crate has three parts:

- the types of the API (`models`), also described by an OpenAPI document
  (`openapi`),
- an async HTTP client (`Client`, with the `client` feature, on by default),
- a builder for filter expressions (`filter`).

With the `proto` feature, `proto::RawFlow` is the message the inlet writes to
Kafka.

```rust,no_run
use flowscope_sdk::filter::{and, col};
use flowscope_sdk::models::{GraphInput, LineInput, Units};

# async fn example() -> Result<(), flowscope_sdk::Error> {
let client = flowscope_sdk::Client::new("http://127.0.0.1:8080")?;
let filter = and([col("InIfBoundary").eq_keyword("external"), col("SrcAS").eq(15169)]);
let graph = client
    .graph_line(&LineInput {
        graph: GraphInput {
            start: "2026-10-10T00:00:00Z".into(),
            end: "2026-10-10T06:00:00Z".into(),
            dimensions: vec!["DstCountry".into()],
            limit: 10,
            filter: filter.to_string(),
            units: Units::L3bps,
            ..Default::default()
        },
        points: 200,
        ..Default::default()
    })
    .await?;
println!("{} series", graph.rows.len());
# Ok(())
# }
```

The console serves the OpenAPI document at `/api/v0/console/openapi.json`.

## License

AGPL-3.0-only, like FlowScope.

[FlowScope]: https://github.com/rinjanianalytics/flowscope
[Akvorado]: https://github.com/akvorado/akvorado
