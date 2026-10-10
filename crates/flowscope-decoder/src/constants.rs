// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Protocol constants. Port of `common/constants`.

pub const ETYPE_IPV4: u16 = 0x0800;
pub const ETYPE_IPV6: u16 = 0x86dd;
pub const ETYPE_MPLS: u16 = 0x8847;
pub const ETYPE_VLAN: u16 = 0x8100;

pub const PORT_VXLAN: u16 = 4789;

pub const PROTO_ICMPV4: u8 = 1;
pub const PROTO_IPV4: u8 = 4;
pub const PROTO_TCP: u8 = 6;
pub const PROTO_UDP: u8 = 17;
pub const PROTO_IPV6: u8 = 41;
pub const PROTO_SRH: u8 = 43;
pub const PROTO_GRE: u8 = 47;
pub const PROTO_ICMPV6: u8 = 58;
