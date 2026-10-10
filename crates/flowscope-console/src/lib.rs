// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Console service: API to query the flows stored in ClickHouse.

pub mod assets;
pub mod auth;
pub mod clickhouse;
pub mod completion;
pub mod component;
pub mod config;
pub mod database;
pub mod docs;
pub mod filter;
pub mod graph;
mod highlight;
pub mod output;
pub mod query;
pub mod sqlbuilder;
pub mod widgets;

/// Data files embedded at build time.
pub(crate) mod embedded {
    include!(concat!(env!("OUT_DIR"), "/embedded.rs"));
}

#[cfg(test)]
mod component_tests;
#[cfg(test)]
mod golden_tests;
