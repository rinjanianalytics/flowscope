// SPDX-FileCopyrightText: 2026 FlowScope contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Client library for the FlowScope console API.
//!
//! The package is `rinjani-flowscope-sdk`. The library is `flowscope_sdk`.
//!
//! The crate has three parts:
//!
//! - the types of the API ([`models`]), also described by an OpenAPI
//!   document ([`openapi`]),
//! - an HTTP client ([`Client`], with the `client` feature, on by default),
//! - a builder for filter expressions ([`filter`]).
//!
//! With the `proto` feature, `proto::RawFlow` is the message the inlet writes
//! to Kafka, for the programs reading or writing raw flows.
//!
//! ```no_run
//! use flowscope_sdk::filter::{and, col};
//! use flowscope_sdk::models::{GraphInput, LineInput, Units};
//!
//! # #[cfg(feature = "client")]
//! # async fn example() -> Result<(), flowscope_sdk::Error> {
//! let client = flowscope_sdk::Client::new("http://127.0.0.1:8080")?;
//! let filter = and([col("InIfBoundary").eq_keyword("external"), col("SrcAS").eq(15169)]);
//! let graph = client
//!     .graph_line(&LineInput {
//!         graph: GraphInput {
//!             start: "2026-10-10T00:00:00Z".into(),
//!             end: "2026-10-10T06:00:00Z".into(),
//!             dimensions: vec!["DstCountry".into()],
//!             limit: 10,
//!             filter: filter.to_string(),
//!             units: Units::L3bps,
//!             ..Default::default()
//!         },
//!         points: 200,
//!         ..Default::default()
//!     })
//!     .await?;
//! println!("{} series", graph.rows.len());
//! # Ok(())
//! # }
//! ```

#[cfg(feature = "client")]
mod client;
pub mod filter;
pub mod models;
pub mod openapi;

#[cfg(feature = "client")]
pub use client::{Client, Error};
/// The messages exchanged over Kafka.
#[cfg(feature = "proto")]
pub use flowscope_pb as proto;
