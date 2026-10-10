// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Outlet service: read raw flows from Kafka, decode them, enrich them and
//! store them in ClickHouse. Port of `outlet/`.

pub mod clickhouse;
pub mod core;
pub mod geoip;
pub mod kafkainput;
pub mod kafkaoutput;
pub mod metadata;
pub mod networks;
pub mod routing;
