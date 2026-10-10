// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! A synthetic exporter, for demos and tests. Port of `demoexporter`.
//!
//! It sends NetFlow v9 packets with generated flows, answers the SNMP
//! requests about its interfaces, and announces routes over BMP.

pub mod bmp;
pub mod config;
pub mod flows;
pub mod random;
pub mod snmp;
