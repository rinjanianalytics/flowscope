// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! A small SNMP implementation: BER, v2c and v3 messages (USM with MD5/SHA
//! authentication and DES/AES privacy), and a client for GET requests.
//! The Go version uses gosnmp.

pub mod ber;
pub mod client;
pub mod message;
pub mod usm;

pub use client::{Credentials, EngineState, Error, Options, Response, get_v2c, get_v3};
pub use message::Value;
pub use usm::{AuthProtocol, PrivProtocol};
