// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Column keys. Generated from the Go `ColumnKey` enum, keep in the same order:
//! the numeric values drive the protobuf field assignment.

#![allow(non_upper_case_globals)]

/// ColumnKey is the identifier of a column.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ColumnKey(pub u32);

impl ColumnKey {
    pub const TimeReceived: ColumnKey = ColumnKey(1);
    pub const SamplingRate: ColumnKey = ColumnKey(2);
    pub const EType: ColumnKey = ColumnKey(3);
    pub const Proto: ColumnKey = ColumnKey(4);
    pub const Bytes: ColumnKey = ColumnKey(5);
    pub const Packets: ColumnKey = ColumnKey(6);
    pub const PacketSize: ColumnKey = ColumnKey(7);
    pub const PacketSizeBucket: ColumnKey = ColumnKey(8);
    pub const ForwardingStatus: ColumnKey = ColumnKey(9);
    pub const FlowDirection: ColumnKey = ColumnKey(10);
    pub const ExporterAddress: ColumnKey = ColumnKey(11);
    pub const ExporterName: ColumnKey = ColumnKey(12);
    pub const ExporterGroup: ColumnKey = ColumnKey(13);
    pub const ExporterRole: ColumnKey = ColumnKey(14);
    pub const ExporterSite: ColumnKey = ColumnKey(15);
    pub const ExporterRegion: ColumnKey = ColumnKey(16);
    pub const ExporterTenant: ColumnKey = ColumnKey(17);
    pub const SrcAddr: ColumnKey = ColumnKey(18);
    pub const DstAddr: ColumnKey = ColumnKey(19);
    pub const SrcNetMask: ColumnKey = ColumnKey(20);
    pub const DstNetMask: ColumnKey = ColumnKey(21);
    pub const SrcNetPrefix: ColumnKey = ColumnKey(22);
    pub const DstNetPrefix: ColumnKey = ColumnKey(23);
    pub const SrcAS: ColumnKey = ColumnKey(24);
    pub const DstAS: ColumnKey = ColumnKey(25);
    pub const SrcVlan: ColumnKey = ColumnKey(26);
    pub const DstVlan: ColumnKey = ColumnKey(27);
    pub const SrcPort: ColumnKey = ColumnKey(28);
    pub const DstPort: ColumnKey = ColumnKey(29);
    pub const SrcNetName: ColumnKey = ColumnKey(30);
    pub const DstNetName: ColumnKey = ColumnKey(31);
    pub const SrcNetRole: ColumnKey = ColumnKey(32);
    pub const DstNetRole: ColumnKey = ColumnKey(33);
    pub const SrcNetSite: ColumnKey = ColumnKey(34);
    pub const DstNetSite: ColumnKey = ColumnKey(35);
    pub const SrcNetRegion: ColumnKey = ColumnKey(36);
    pub const DstNetRegion: ColumnKey = ColumnKey(37);
    pub const SrcNetTenant: ColumnKey = ColumnKey(38);
    pub const DstNetTenant: ColumnKey = ColumnKey(39);
    pub const SrcCountry: ColumnKey = ColumnKey(40);
    pub const DstCountry: ColumnKey = ColumnKey(41);
    pub const SrcGeoState: ColumnKey = ColumnKey(42);
    pub const DstGeoState: ColumnKey = ColumnKey(43);
    pub const SrcGeoCity: ColumnKey = ColumnKey(44);
    pub const DstGeoCity: ColumnKey = ColumnKey(45);
    pub const DstASPath: ColumnKey = ColumnKey(46);
    pub const Dst1stAS: ColumnKey = ColumnKey(47);
    pub const Dst2ndAS: ColumnKey = ColumnKey(48);
    pub const Dst3rdAS: ColumnKey = ColumnKey(49);
    pub const SrcCommunities: ColumnKey = ColumnKey(50);
    pub const SrcLargeCommunities: ColumnKey = ColumnKey(51);
    pub const DstCommunities: ColumnKey = ColumnKey(52);
    pub const DstLargeCommunities: ColumnKey = ColumnKey(53);
    pub const InIfName: ColumnKey = ColumnKey(54);
    pub const OutIfName: ColumnKey = ColumnKey(55);
    pub const InIfDescription: ColumnKey = ColumnKey(56);
    pub const OutIfDescription: ColumnKey = ColumnKey(57);
    pub const InIfSpeed: ColumnKey = ColumnKey(58);
    pub const OutIfSpeed: ColumnKey = ColumnKey(59);
    pub const InIfProvider: ColumnKey = ColumnKey(60);
    pub const OutIfProvider: ColumnKey = ColumnKey(61);
    pub const InIfConnectivity: ColumnKey = ColumnKey(62);
    pub const OutIfConnectivity: ColumnKey = ColumnKey(63);
    pub const InIfBoundary: ColumnKey = ColumnKey(64);
    pub const OutIfBoundary: ColumnKey = ColumnKey(65);
    pub const SrcAddrNAT: ColumnKey = ColumnKey(66);
    pub const DstAddrNAT: ColumnKey = ColumnKey(67);
    pub const SrcPortNAT: ColumnKey = ColumnKey(68);
    pub const DstPortNAT: ColumnKey = ColumnKey(69);
    pub const SrcMAC: ColumnKey = ColumnKey(70);
    pub const DstMAC: ColumnKey = ColumnKey(71);
    pub const IPTTL: ColumnKey = ColumnKey(72);
    pub const IPTos: ColumnKey = ColumnKey(73);
    pub const IPFragmentID: ColumnKey = ColumnKey(74);
    pub const IPFragmentOffset: ColumnKey = ColumnKey(75);
    pub const IPv6FlowLabel: ColumnKey = ColumnKey(76);
    pub const TCPFlags: ColumnKey = ColumnKey(77);
    pub const ICMPv4: ColumnKey = ColumnKey(78);
    pub const ICMPv4Type: ColumnKey = ColumnKey(79);
    pub const ICMPv4Code: ColumnKey = ColumnKey(80);
    pub const ICMPv6: ColumnKey = ColumnKey(81);
    pub const ICMPv6Type: ColumnKey = ColumnKey(82);
    pub const ICMPv6Code: ColumnKey = ColumnKey(83);
    pub const NextHop: ColumnKey = ColumnKey(84);
    pub const MPLSLabels: ColumnKey = ColumnKey(85);
    pub const MPLS1stLabel: ColumnKey = ColumnKey(86);
    pub const MPLS2ndLabel: ColumnKey = ColumnKey(87);
    pub const MPLS3rdLabel: ColumnKey = ColumnKey(88);
    pub const MPLS4thLabel: ColumnKey = ColumnKey(89);
    pub const IngressVRFID: ColumnKey = ColumnKey(90);
    pub const EgressVRFID: ColumnKey = ColumnKey(91);
    /// Points after the last static column. Custom dictionary columns come after.
    pub const Last: ColumnKey = ColumnKey(92);
}

/// Static column names, indexed by key (index 0 is unused).
pub(crate) const STATIC_NAMES: [&str; 92] = [
    "",
    "TimeReceived",
    "SamplingRate",
    "EType",
    "Proto",
    "Bytes",
    "Packets",
    "PacketSize",
    "PacketSizeBucket",
    "ForwardingStatus",
    "FlowDirection",
    "ExporterAddress",
    "ExporterName",
    "ExporterGroup",
    "ExporterRole",
    "ExporterSite",
    "ExporterRegion",
    "ExporterTenant",
    "SrcAddr",
    "DstAddr",
    "SrcNetMask",
    "DstNetMask",
    "SrcNetPrefix",
    "DstNetPrefix",
    "SrcAS",
    "DstAS",
    "SrcVlan",
    "DstVlan",
    "SrcPort",
    "DstPort",
    "SrcNetName",
    "DstNetName",
    "SrcNetRole",
    "DstNetRole",
    "SrcNetSite",
    "DstNetSite",
    "SrcNetRegion",
    "DstNetRegion",
    "SrcNetTenant",
    "DstNetTenant",
    "SrcCountry",
    "DstCountry",
    "SrcGeoState",
    "DstGeoState",
    "SrcGeoCity",
    "DstGeoCity",
    "DstASPath",
    "Dst1stAS",
    "Dst2ndAS",
    "Dst3rdAS",
    "SrcCommunities",
    "SrcLargeCommunities",
    "DstCommunities",
    "DstLargeCommunities",
    "InIfName",
    "OutIfName",
    "InIfDescription",
    "OutIfDescription",
    "InIfSpeed",
    "OutIfSpeed",
    "InIfProvider",
    "OutIfProvider",
    "InIfConnectivity",
    "OutIfConnectivity",
    "InIfBoundary",
    "OutIfBoundary",
    "SrcAddrNAT",
    "DstAddrNAT",
    "SrcPortNAT",
    "DstPortNAT",
    "SrcMAC",
    "DstMAC",
    "IPTTL",
    "IPTos",
    "IPFragmentID",
    "IPFragmentOffset",
    "IPv6FlowLabel",
    "TCPFlags",
    "ICMPv4",
    "ICMPv4Type",
    "ICMPv4Code",
    "ICMPv6",
    "ICMPv6Type",
    "ICMPv6Code",
    "NextHop",
    "MPLSLabels",
    "MPLS1stLabel",
    "MPLS2ndLabel",
    "MPLS3rdLabel",
    "MPLS4thLabel",
    "IngressVRFID",
    "EgressVRFID",
];
