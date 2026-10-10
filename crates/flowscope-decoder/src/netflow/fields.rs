// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! IPFIX information elements (and NetFlow v9 field types) used by the
//! decoder. Numbers from the IANA registry.

pub const OCTET_DELTA_COUNT: u16 = 1;
pub const PACKET_DELTA_COUNT: u16 = 2;
pub const PROTOCOL_IDENTIFIER: u16 = 4;
pub const IP_CLASS_OF_SERVICE: u16 = 5;
pub const TCP_CONTROL_BITS: u16 = 6;
pub const SOURCE_TRANSPORT_PORT: u16 = 7;
pub const SOURCE_IPV4_ADDRESS: u16 = 8;
pub const SOURCE_IPV4_PREFIX_LENGTH: u16 = 9;
pub const INGRESS_INTERFACE: u16 = 10;
pub const DESTINATION_TRANSPORT_PORT: u16 = 11;
pub const DESTINATION_IPV4_ADDRESS: u16 = 12;
pub const DESTINATION_IPV4_PREFIX_LENGTH: u16 = 13;
pub const EGRESS_INTERFACE: u16 = 14;
pub const IP_NEXT_HOP_IPV4_ADDRESS: u16 = 15;
pub const BGP_SOURCE_AS_NUMBER: u16 = 16;
pub const BGP_DESTINATION_AS_NUMBER: u16 = 17;
pub const BGP_NEXT_HOP_IPV4_ADDRESS: u16 = 18;
/// NetFlow v9 FIRST_SWITCHED (flowStartSysUpTime in IPFIX).
pub const NFV9_FIRST_SWITCHED: u16 = 22;
pub const POST_OCTET_DELTA_COUNT: u16 = 23;
pub const POST_PACKET_DELTA_COUNT: u16 = 24;
pub const SOURCE_IPV6_ADDRESS: u16 = 27;
pub const DESTINATION_IPV6_ADDRESS: u16 = 28;
pub const SOURCE_IPV6_PREFIX_LENGTH: u16 = 29;
pub const DESTINATION_IPV6_PREFIX_LENGTH: u16 = 30;
pub const FLOW_LABEL_IPV6: u16 = 31;
pub const ICMP_TYPE_CODE_IPV4: u16 = 32;
pub const SAMPLING_INTERVAL: u16 = 34;
pub const SAMPLER_ID: u16 = 48;
pub const SAMPLER_RANDOM_INTERVAL: u16 = 50;
pub const MINIMUM_TTL: u16 = 52;
pub const FRAGMENT_IDENTIFICATION: u16 = 54;
pub const SOURCE_MAC_ADDRESS: u16 = 56;
pub const POST_DESTINATION_MAC_ADDRESS: u16 = 57;
pub const VLAN_ID: u16 = 58;
pub const POST_VLAN_ID: u16 = 59;
pub const FLOW_DIRECTION: u16 = 61;
pub const IP_NEXT_HOP_IPV6_ADDRESS: u16 = 62;
pub const BGP_NEXT_HOP_IPV6_ADDRESS: u16 = 63;
pub const MPLS_TOP_LABEL_STACK_SECTION: u16 = 70;
pub const MPLS_LABEL_STACK_SECTION10: u16 = 79;
pub const DESTINATION_MAC_ADDRESS: u16 = 80;
pub const POST_SOURCE_MAC_ADDRESS: u16 = 81;
pub const FRAGMENT_OFFSET: u16 = 88;
pub const FORWARDING_STATUS: u16 = 89;
pub const COMMON_PROPERTIES_ID: u16 = 137;
pub const ICMP_TYPE_CODE_IPV6: u16 = 139;
pub const FLOW_START_SECONDS: u16 = 150;
pub const FLOW_START_MILLISECONDS: u16 = 152;
pub const FLOW_START_MICROSECONDS: u16 = 154;
pub const FLOW_START_NANOSECONDS: u16 = 156;
pub const SYSTEM_INIT_TIME_MILLISECONDS: u16 = 160;
pub const ICMP_TYPE_IPV4: u16 = 176;
pub const ICMP_CODE_IPV4: u16 = 177;
pub const ICMP_TYPE_IPV6: u16 = 178;
pub const ICMP_CODE_IPV6: u16 = 179;
pub const IP_TTL: u16 = 192;
pub const POST_NAT_SOURCE_IPV4_ADDRESS: u16 = 225;
pub const POST_NAT_DESTINATION_IPV4_ADDRESS: u16 = 226;
pub const POST_NAPT_SOURCE_TRANSPORT_PORT: u16 = 227;
pub const POST_NAPT_DESTINATION_TRANSPORT_PORT: u16 = 228;
pub const INITIATOR_OCTETS: u16 = 231;
pub const RESPONDER_OCTETS: u16 = 232;
pub const INGRESS_VRF_ID: u16 = 234;
pub const EGRESS_VRF_ID: u16 = 235;
pub const DOT1Q_VLAN_ID: u16 = 243;
pub const INGRESS_PHYSICAL_INTERFACE: u16 = 252;
pub const EGRESS_PHYSICAL_INTERFACE: u16 = 253;
pub const POST_DOT1Q_VLAN_ID: u16 = 254;
pub const SELECTOR_ID: u16 = 302;
pub const SAMPLING_PACKET_INTERVAL: u16 = 305;
pub const SAMPLING_PACKET_SPACE: u16 = 306;
pub const DATA_LINK_FRAME_SECTION: u16 = 315;

/// Private enterprise number for reverse information elements (RFC 5103).
pub const REVERSE_PEN: u32 = 29305;
/// Private enterprise number of Juniper.
pub const JUNIPER_PEN: u32 = 2636;
