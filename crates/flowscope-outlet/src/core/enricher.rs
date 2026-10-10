// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Flow enrichment. Port of `outlet/core/enricher.go`.

use super::Inner;
use super::classifier::{ExporterClassification, ExporterInfo, InterfaceClassification, InterfaceInfo};
use super::config::{AsnProvider, NetProvider};
use crate::networks::NetworkAttributes;
use flowscope_schema::{ColumnKey, FlowMessage, InterfaceBoundary};
use std::net::Ipv6Addr;
use std::sync::Arc;
use std::time::SystemTime;

impl Inner {
    /// Add data to a flow. Returns true when the flow should be skipped.
    pub(crate) fn enrich_flow(&self, flow: &mut FlowMessage, exporter_ip: Ipv6Addr, exporter_str: &str) -> bool {
        let mut exporter_name = String::new();
        let mut exp_classification = ExporterClassification::default();
        let mut in_if = InterfaceInfo::default();
        let mut out_if = InterfaceInfo::default();
        let mut in_classification = InterfaceClassification::default();
        let mut out_classification = InterfaceClassification::default();
        let mut skip = false;
        let t = SystemTime::now();

        let lookup = |if_index: u32,
                      vlan: u16,
                      info: &mut InterfaceInfo,
                      classification: &mut InterfaceClassification,
                      name: &mut String,
                      exp: &mut ExporterClassification| {
            if if_index == 0 {
                return;
            }
            let answer = self.deps.metadata.lookup(t, exporter_ip, if_index);
            if !answer.found {
                return;
            }
            *name = answer.exporter.name;
            exp.region = answer.exporter.region;
            exp.role = answer.exporter.role;
            exp.tenant = answer.exporter.tenant;
            exp.site = answer.exporter.site;
            exp.group = answer.exporter.group;
            *info = InterfaceInfo {
                index: if_index,
                name: answer.interface.name,
                description: answer.interface.description,
                speed: answer.interface.speed as u32,
                vlan,
            };
            classification.provider = answer.interface.provider;
            classification.connectivity = answer.interface.connectivity;
            classification.boundary = answer.interface.boundary;
        };
        lookup(
            flow.in_if,
            flow.src_vlan,
            &mut in_if,
            &mut in_classification,
            &mut exporter_name,
            &mut exp_classification,
        );
        lookup(
            flow.out_if,
            flow.dst_vlan,
            &mut out_if,
            &mut out_classification,
            &mut exporter_name,
            &mut exp_classification,
        );

        // We need at least one of them.
        if flow.out_if == 0 && flow.in_if == 0 {
            self.metrics.flows_errors.with_label_values(&[exporter_str, "input and output interfaces missing"]).inc();
            skip = true;
        } else if exporter_name.is_empty() {
            self.metrics.flows_errors.with_label_values(&[exporter_str, "metadata missing"]).inc();
            skip = true;
        }

        if let Some(&rate) = self.config.override_sampling_rate.lookup(&exporter_ip)
            && rate > 0
        {
            flow.sampling_rate = rate;
        }
        if flow.sampling_rate == 0 {
            match self.config.default_sampling_rate.lookup(&exporter_ip) {
                Some(&rate) if rate > 0 => flow.sampling_rate = rate,
                _ => {
                    self.metrics.flows_errors.with_label_values(&[exporter_str, "sampling rate missing"]).inc();
                    skip = true;
                }
            }
        }
        if skip {
            return true;
        }

        // Classification
        let ei = ExporterInfo { ip: exporter_str.to_string(), name: exporter_name.clone() };
        if !self.classify_exporter(t, &ei, flow, exp_classification)
            || !self.classify_interface(t, flow, &ei, &out_if, out_classification, false)
            || !self.classify_interface(t, flow, &ei, &in_if, in_classification, true)
        {
            // Flow is rejected
            return true;
        }

        let source_routing =
            self.deps.runtime.block_on(self.deps.routing.lookup(flow.src_addr, None, flow.exporter_address));
        let dest_routing =
            self.deps.runtime.block_on(self.deps.routing.lookup(flow.dst_addr, flow.next_hop, flow.exporter_address));

        flow.src_net_mask = self.get_net_mask(flow.src_net_mask, source_routing.net_mask);
        flow.dst_net_mask = self.get_net_mask(flow.dst_net_mask, dest_routing.net_mask);
        flow.next_hop = self.get_next_hop(flow.next_hop, dest_routing.next_hop);

        // Network attributes also cover the GeoIP databases and are a source
        // for AS numbers: look them up first.
        let empty = Arc::new(NetworkAttributes::default());
        let (src_net, dst_net) = match &self.deps.networks {
            Some(n) => (
                flow.src_addr.map(|ip| n.lookup(&ip)).unwrap_or_else(|| empty.clone()),
                flow.dst_addr.map(|ip| n.lookup(&ip)).unwrap_or_else(|| empty.clone()),
            ),
            None => (empty.clone(), empty),
        };

        flow.src_as = self.get_as_number(flow.src_as, source_routing.asn, src_net.asn, flow.src_net_mask);
        flow.dst_as = self.get_as_number(flow.dst_as, dest_routing.asn, dst_net.asn, flow.dst_net_mask);
        flow.append_array_uint32(ColumnKey::SrcCommunities, &source_routing.communities);
        flow.append_array_uint32(ColumnKey::DstCommunities, &dest_routing.communities);
        flow.append_array_uint32(ColumnKey::DstASPath, &dest_routing.as_path);
        if !source_routing.large_communities.is_empty() {
            let v: Vec<u128> = source_routing.large_communities.iter().map(|c| c.to_u128()).collect();
            flow.append_array_uint128(ColumnKey::SrcLargeCommunities, &v);
        }
        if !dest_routing.large_communities.is_empty() {
            let v: Vec<u128> = dest_routing.large_communities.iter().map(|c| c.to_u128()).collect();
            flow.append_array_uint128(ColumnKey::DstLargeCommunities, &v);
        }

        if self.deps.networks.is_some() {
            flow.append_string(ColumnKey::SrcNetName, &src_net.name);
            flow.append_string(ColumnKey::DstNetName, &dst_net.name);
            flow.append_string(ColumnKey::SrcNetRole, &src_net.role);
            flow.append_string(ColumnKey::DstNetRole, &dst_net.role);
            flow.append_string(ColumnKey::SrcNetSite, &src_net.site);
            flow.append_string(ColumnKey::DstNetSite, &dst_net.site);
            flow.append_string(ColumnKey::SrcNetRegion, &src_net.region);
            flow.append_string(ColumnKey::DstNetRegion, &dst_net.region);
            flow.append_string(ColumnKey::SrcNetTenant, &src_net.tenant);
            flow.append_string(ColumnKey::DstNetTenant, &dst_net.tenant);
            flow.append_string(ColumnKey::SrcCountry, &src_net.country);
            flow.append_string(ColumnKey::DstCountry, &dst_net.country);
            flow.append_string(ColumnKey::SrcGeoState, &src_net.state);
            flow.append_string(ColumnKey::DstGeoState, &dst_net.state);
            flow.append_string(ColumnKey::SrcGeoCity, &src_net.city);
            flow.append_string(ColumnKey::DstGeoCity, &dst_net.city);
        }

        flow.append_uint(ColumnKey::InIfSpeed, in_if.speed as u64);
        flow.append_uint(ColumnKey::OutIfSpeed, out_if.speed as u64);
        false
    }

    /// AS number for a flow, depending on the configured providers.
    pub(crate) fn get_as_number(&self, flow_as: u32, routing_as: u32, net_as: u32, flow_net_mask: u8) -> u32 {
        let mut asn = 0;
        for provider in &self.config.asn_providers {
            if asn != 0 {
                break;
            }
            asn = match provider {
                AsnProvider::Flow => flow_as,
                AsnProvider::FlowExceptPrivate if is_private_as(flow_as) => 0,
                AsnProvider::FlowExceptPrivate => flow_as,
                AsnProvider::FlowExceptDefaultRoute if flow_net_mask == 0 => 0,
                AsnProvider::FlowExceptDefaultRoute => flow_as,
                AsnProvider::Routing => routing_as,
                AsnProvider::RoutingExceptPrivate if is_private_as(routing_as) => 0,
                AsnProvider::RoutingExceptPrivate => routing_as,
                AsnProvider::Networks => net_as,
            };
        }
        asn
    }

    /// Prefix length for a flow, depending on the configured providers.
    pub(crate) fn get_net_mask(&self, flow_mask: u8, routing_mask: u8) -> u8 {
        let mut mask = 0;
        for provider in &self.config.net_providers {
            if mask != 0 {
                break;
            }
            mask = match provider {
                NetProvider::Flow => flow_mask,
                NetProvider::Routing => routing_mask,
            };
        }
        mask
    }

    /// Next hop for a flow, depending on the configured providers. A missing
    /// next hop counts as unspecified, so the next provider is used. The Go
    /// version stopped at a missing next hop from the flow.
    pub(crate) fn get_next_hop(&self, flow_nh: Option<Ipv6Addr>, routing_nh: Option<Ipv6Addr>) -> Option<Ipv6Addr> {
        let mut nh: Option<Ipv6Addr> = None;
        for provider in &self.config.net_providers {
            if nh.is_some_and(|n| !n.is_unspecified()) {
                break;
            }
            nh = match provider {
                NetProvider::Flow => flow_nh,
                NetProvider::Routing => routing_nh,
            }
            .or(nh);
        }
        nh
    }

    fn write_exporter(flow: &mut FlowMessage, c: &ExporterClassification) -> bool {
        if c.reject {
            return false;
        }
        flow.append_string(ColumnKey::ExporterName, &c.name);
        flow.append_string(ColumnKey::ExporterGroup, &c.group);
        flow.append_string(ColumnKey::ExporterRole, &c.role);
        flow.append_string(ColumnKey::ExporterSite, &c.site);
        flow.append_string(ColumnKey::ExporterRegion, &c.region);
        flow.append_string(ColumnKey::ExporterTenant, &c.tenant);
        true
    }

    fn classify_exporter(
        &self,
        t: SystemTime,
        ei: &ExporterInfo,
        flow: &mut FlowMessage,
        mut classification: ExporterClassification,
    ) -> bool {
        // The metadata component already provided the information.
        if classification != ExporterClassification::default() {
            classification.name = ei.name.clone();
            return Self::write_exporter(flow, &classification);
        }
        if self.config.exporter_classifiers.is_empty() {
            classification.name = ei.name.clone();
            Self::write_exporter(flow, &classification);
            return true;
        }
        if let Some(c) = self.exporter_cache.get(t, ei) {
            return Self::write_exporter(flow, &c);
        }
        for (idx, rule) in self.config.exporter_classifiers.iter().enumerate() {
            if let Err(e) = rule.exec(ei, &mut classification) {
                if self.classifier_errors_log.allow() {
                    tracing::error!(error = %e, r#type = "exporter", index = idx, exporter = %ei.name,
                        "error executing classifier");
                }
                self.metrics.classifier_errors.with_label_values(&["exporter", &idx.to_string()]).inc();
                break;
            }
            if classification.group.is_empty()
                || classification.role.is_empty()
                || classification.site.is_empty()
                || classification.region.is_empty()
                || classification.tenant.is_empty()
            {
                continue;
            }
            break;
        }
        if classification.name.is_empty() {
            classification.name = ei.name.clone();
        }
        self.exporter_cache.put(t, ei.clone(), classification.clone());
        Self::write_exporter(flow, &classification)
    }

    fn write_interface(flow: &mut FlowMessage, c: &InterfaceClassification, direction_in: bool) -> bool {
        if c.reject {
            return false;
        }
        let keys = if direction_in {
            [
                ColumnKey::InIfName,
                ColumnKey::InIfDescription,
                ColumnKey::InIfConnectivity,
                ColumnKey::InIfProvider,
                ColumnKey::InIfBoundary,
            ]
        } else {
            [
                ColumnKey::OutIfName,
                ColumnKey::OutIfDescription,
                ColumnKey::OutIfConnectivity,
                ColumnKey::OutIfProvider,
                ColumnKey::OutIfBoundary,
            ]
        };
        flow.append_string(keys[0], &c.name);
        flow.append_string(keys[1], &c.description);
        flow.append_string(keys[2], &c.connectivity);
        flow.append_string(keys[3], &c.provider);
        flow.append_uint(keys[4], c.boundary as u64);
        true
    }

    fn classify_interface(
        &self,
        t: SystemTime,
        flow: &mut FlowMessage,
        ei: &ExporterInfo,
        ii: &InterfaceInfo,
        mut classification: InterfaceClassification,
        direction_in: bool,
    ) -> bool {
        // The metadata component already provided the information.
        if classification != InterfaceClassification::default() {
            classification.name = ii.name.clone();
            classification.description = ii.description.clone();
            return Self::write_interface(flow, &classification, direction_in);
        }
        if self.config.interface_classifiers.is_empty() {
            classification.name = ii.name.clone();
            classification.description = ii.description.clone();
            Self::write_interface(flow, &classification, direction_in);
            return true;
        }
        let key = (ei.clone(), ii.clone());
        if let Some(c) = self.interface_cache.get(t, &key) {
            return Self::write_interface(flow, &c, direction_in);
        }
        for (idx, rule) in self.config.interface_classifiers.iter().enumerate() {
            if let Err(e) = rule.exec(ei, ii, &mut classification) {
                if self.classifier_errors_log.allow() {
                    tracing::error!(error = %e, r#type = "interface", index = idx, exporter = %ei.name,
                        interface = %ii.name, "error executing classifier");
                }
                self.metrics.classifier_errors.with_label_values(&["interface", &idx.to_string()]).inc();
                break;
            }
            if classification.connectivity.is_empty() || classification.provider.is_empty() {
                continue;
            }
            if classification.boundary == InterfaceBoundary::Undefined {
                continue;
            }
            break;
        }
        if classification.name.is_empty() {
            classification.name = ii.name.clone();
        }
        if classification.description.is_empty() {
            classification.description = ii.description.clone();
        }
        self.interface_cache.put(t, key, classification.clone());
        Self::write_interface(flow, &classification, direction_in)
    }
}

/// Whether an AS number is private or reserved. See
/// <https://www.iana.org/assignments/iana-as-numbers-special-registry/>.
pub fn is_private_as(asn: u32) -> bool {
    asn == 0 || asn == 23456 || (64496..=65551).contains(&asn) || asn >= 4_200_000_000
}
