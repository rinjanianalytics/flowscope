// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! SQL for the completion of the filter values.

use crate::query;
use crate::sqlbuilder::{self as sb, Expr, OrderItem, Query};
use flowscope_schema::{DICTIONARY_ASNS, DICTIONARY_ICMP, DICTIONARY_TCP, DICTIONARY_UDP};

const PROTO_ICMPV4: u64 = 1;
const PROTO_ICMPV6: u64 = 58;
const PROTO_TCP: u64 = 6;
const PROTO_UDP: u64 = 17;

/// The flows received during the last minutes.
fn recent_flows(minutes: u64) -> Expr {
    sb::op(
        sb::column("TimeReceived"),
        ">",
        sb::function("date_sub", vec![sb::column("minute"), sb::uint(minutes), sb::function("now", vec![])]),
    )
}

/// Where the prefix appears in an expression, whatever the case. 0 when it
/// does not appear.
fn prefix_position(expr: Expr, prefix: &str) -> Expr {
    sb::function("positionCaseInsensitive", vec![expr, sb::string(prefix)])
}

/// Keep the rows where the prefix appears.
fn match_prefix(expr: Expr, prefix: &str) -> Expr {
    sb::op(prefix_position(expr, prefix), ">=", sb::uint(1))
}

/// The most used first.
fn most_used_first() -> OrderItem {
    sb::order(sb::function("COUNT", vec![sb::star()])).desc()
}

fn and(exprs: Vec<Expr>) -> Option<Expr> {
    sb::and(exprs)
}

/// MAC addresses (`column` is SrcMAC or DstMAC).
pub fn mac(column: &str, prefix: &str, limit: i64) -> Query {
    let column = sb::column(column);
    sb::select(vec![sb::alias(sb::function("MACNumToString", vec![column.clone()]), "label")])
        .from(sb::table("flows", None))
        .r#where(and(vec![recent_flows(1), match_prefix(sb::column("label"), prefix)]))
        .group_by(vec![column])
        .order_by(vec![most_used_first()])
        .limit(limit)
}

/// Communities and large communities (`direction` is Src or Dst).
pub fn communities(direction: &str, prefix: &str, limit: i64) -> Query {
    // Each community of the recent flows, out of the arrays.
    let unrolled = |column: String| {
        sb::select(vec![sb::alias(
            sb::function("arrayJoin", vec![sb::function("arrayJoin", vec![sb::column(&column)])]),
            "c",
        )])
        .from(sb::table("flows", None))
        .r#where(Some(recent_flows(1)))
        .group_by(vec![sb::column("c")])
        .order_by(vec![most_used_first()])
    };
    let communities = sb::select(vec![
        sb::alias(sb::string("community"), "detail"),
        sb::alias(query::community_to_string(sb::column("c")), "label"),
    ])
    .from_select(unrolled(format!("{direction}Communities")));
    let large = sb::select(vec![
        sb::alias(sb::string("large community"), "detail"),
        sb::alias(query::large_community_to_string(sb::column("c")), "label"),
    ])
    .from_select(unrolled(format!("{direction}LargeCommunities")));
    sb::select(sb::columns(&["label", "detail"]))
        .from_select(communities.union_all(large))
        .r#where(Some(sb::function("startsWith", vec![sb::column("label"), sb::string(prefix)])))
        .limit(limit)
}

/// AS numbers, from the flows and from the dictionary.
pub fn asn(column: &str, prefix: &str, limit: i64, database: &str) -> Query {
    let asns = sb::table(DICTIONARY_ASNS, Some(database));
    let col = sb::column(column);
    let from_flows = sb::select(vec![
        sb::alias(sb::function("concat", vec![sb::string("AS"), sb::function("toString", vec![col.clone()])]), "label"),
        sb::alias(sb::function("dictGet", vec![sb::string(&asns), sb::string("name"), col.clone()]), "detail"),
        sb::alias(sb::uint(1), "rank"),
    ])
    .from(sb::table("flows", None))
    .r#where(and(vec![
        recent_flows(1),
        sb::op(sb::column("detail"), "!=", sb::string("")),
        match_prefix(sb::column("detail"), prefix),
    ]))
    .group_by(vec![col])
    .order_by(vec![most_used_first()])
    .limit(limit);
    let from_dictionary = sb::select(vec![
        sb::alias(
            sb::function("concat", vec![sb::string("AS"), sb::function("toString", vec![sb::column("asn")])]),
            "label",
        ),
        sb::alias(sb::column("name"), "detail"),
        sb::alias(sb::uint(2), "rank"),
    ])
    .from(asns)
    .r#where(Some(match_prefix(sb::column("name"), prefix)))
    .order_by(vec![sb::order(prefix_position(sb::column("name"), prefix)), sb::order(sb::column("asn"))])
    .limit(limit);
    sb::select(sb::columns(&["label", "detail"]))
        .from_select(from_flows.union_distinct(from_dictionary))
        .group_by(sb::columns(&["label", "detail"]))
        .order_by(vec![
            sb::order(sb::function("MIN", vec![sb::column("rank")])),
            sb::order(sb::function("MIN", vec![sb::function("rowNumberInBlock", vec![])])),
        ])
        .limit(limit)
}

/// Ports, from the flows and from the dictionaries.
pub fn port(column: &str, prefix: &str, limit: i64, database: &str) -> Query {
    let col = sb::column(column);
    let tcp = sb::table(DICTIONARY_TCP, Some(database));
    let udp = sb::table(DICTIONARY_UDP, Some(database));
    let port_name =
        |dictionary: &str| sb::function("dictGet", vec![sb::string(dictionary), sb::string("name"), col.clone()]);
    let from_flows = sb::select(vec![
        sb::alias(sb::function("toString", vec![col.clone()]), "label"),
        sb::alias(
            sb::function(
                "if",
                vec![sb::op(sb::column("Proto"), "=", sb::uint(PROTO_TCP)), port_name(&tcp), port_name(&udp)],
            ),
            "detail",
        ),
        sb::alias(sb::uint(1), "rank"),
        sb::alias(sb::function("COUNT", vec![sb::star()]), "c"),
    ])
    .from(sb::table("flows", None))
    .r#where(and(vec![
        sb::op(sb::column("Proto"), "IN", sb::tuple(vec![sb::uint(PROTO_TCP), sb::uint(PROTO_UDP)])),
        recent_flows(1),
        sb::op(sb::column("detail"), "!=", sb::string("")),
        match_prefix(sb::column("detail"), prefix),
    ]))
    .group_by(vec![col, sb::column("Proto")])
    .order_by(vec![most_used_first()])
    .limit(limit);
    let known_ports = sb::select(sb::columns(&["port", "name"]))
        .from(tcp)
        .union_distinct(sb::select(sb::columns(&["port", "name"])).from(udp));
    let from_dictionaries = sb::select(vec![
        sb::alias(sb::function("toString", vec![sb::column("port")]), "label"),
        sb::alias(sb::column("name"), "detail"),
        sb::alias(sb::uint(2), "rank"),
        sb::alias(sb::uint(0), "c"),
    ])
    .from_select(known_ports)
    .r#where(Some(match_prefix(sb::column("name"), prefix)))
    .order_by(vec![sb::order(prefix_position(sb::column("name"), prefix)), sb::order(sb::column("port"))])
    .limit(limit);
    sb::select(sb::columns(&["label", "detail"]))
        .from_select(from_flows.union_distinct(from_dictionaries))
        .group_by(sb::columns(&["rank", "label", "detail"]))
        .order_by(vec![
            sb::order(sb::column("rank")),
            sb::order(sb::function("MAX", vec![sb::column("c")])).desc(),
            sb::order(sb::function("MIN", vec![sb::function("rowNumberInBlock", vec![])])),
        ])
        .limit(limit)
}

/// Network attributes seen in the recent flows.
pub fn network_attribute(column: &str, prefix: &str, limit: i64) -> Query {
    let col = sb::column(column);
    sb::select(vec![sb::alias(col.clone(), "label")])
        .from(sb::table("flows", None))
        .r#where(and(vec![
            recent_flows(10),
            sb::op(sb::column("label"), "!=", sb::string("")),
            match_prefix(sb::column("label"), prefix),
        ]))
        .group_by(vec![col])
        .order_by(vec![sb::order(prefix_position(sb::column("label"), prefix)), most_used_first()])
        .limit(limit)
}

/// ICMP types and codes (`column` is ICMPv4 or ICMPv6).
pub fn icmp(column: &str, prefix: &str, limit: i64, database: &str) -> Query {
    let proto = if column == "ICMPv6" { PROTO_ICMPV6 } else { PROTO_ICMPV4 };
    let col = sb::column(column);
    let from_flows = sb::select(vec![sb::alias(col.clone(), "label"), sb::alias(sb::uint(1), "rank")])
        .from(sb::table("flows", None))
        .r#where(and(vec![
            recent_flows(1),
            sb::op(sb::column("Proto"), "=", sb::uint(proto)),
            match_prefix(sb::column("label"), prefix),
        ]))
        .group_by(vec![col])
        .order_by(vec![most_used_first()])
        .limit(limit);
    let from_dictionary = sb::select(vec![sb::alias(sb::column("name"), "label"), sb::alias(sb::uint(2), "rank")])
        .from(sb::table(DICTIONARY_ICMP, Some(database)))
        .r#where(and(vec![
            match_prefix(sb::column("label"), prefix),
            sb::op(sb::column("proto"), "=", sb::uint(proto)),
        ]))
        .order_by(vec![
            sb::order(prefix_position(sb::column("label"), prefix)),
            sb::order(sb::column("type")),
            sb::order(sb::column("code")),
        ])
        .limit(limit);
    sb::select(vec![sb::column("label")])
        .from_select(from_flows.union_distinct(from_dictionary))
        .group_by(vec![sb::column("label")])
        .order_by(vec![
            sb::order(sb::function("MIN", vec![sb::column("rank")])),
            sb::order(sb::function("MIN", vec![sb::function("rowNumberInBlock", vec![])])),
        ])
        .limit(limit)
}

/// Values of a column of the exporters table.
pub fn exporter_attribute(column: &str, prefix: &str, limit: i64) -> Query {
    let name = sb::column(column);
    sb::select(vec![sb::alias(name.clone(), "label")])
        .from(sb::table("exporters", None))
        .r#where(Some(match_prefix(name.clone(), prefix)))
        .group_by(vec![name.clone()])
        .order_by(vec![sb::order(prefix_position(name.clone(), prefix)), sb::order(name)])
        .limit(limit)
}

/// Values of a custom dictionary column seen in the recent flows.
pub fn custom_attribute(column: &str, prefix: &str, limit: i64) -> Query {
    let name = sb::column(column);
    sb::select(vec![sb::alias(name.clone(), "attribute")])
        .distinct()
        .from(sb::table("flows", None))
        .r#where(and(vec![recent_flows(10), match_prefix(sb::column("attribute"), prefix)]))
        .order_by(vec![sb::order(prefix_position(sb::column("attribute"), prefix)), sb::order(name)])
        .limit(limit)
}

/// How to read the rows of a value completion query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rows {
    /// A `label` column, with a fixed detail.
    Label { detail: String, quoted: bool },
    /// `label` and `detail` columns, not quoted.
    LabelDetail,
}

/// The query completing the values of a built-in column, with how to read
/// its rows. None when the values are not completed from the database, or
/// when the column is unknown or disabled (Go then sends broken SQL).
pub fn value_query(
    schema: &flowscope_schema::Schema,
    input_column: &str,
    prefix: &str,
    limit: i64,
    database: &str,
) -> Option<(Query, Rows)> {
    let lower = input_column.to_lowercase();
    // The column name with the right case, when enabled.
    let fixed = || schema.columns().find(|c| c.name.to_lowercase() == lower).map(|c| c.name.clone());
    let label = |detail: &str, quoted: bool| Rows::Label { detail: detail.into(), quoted };
    Some(match lower.as_str() {
        "srcmac" | "dstmac" => (mac(&fixed()?, prefix, limit), label("MAC address", false)),
        "srccommunities" | "dstcommunities" => (communities(&fixed()?[..3], prefix, limit), Rows::LabelDetail),
        "srcas" | "dstas" | "dst1stas" | "dst2ndas" | "dst3rdas" | "dstaspath" => {
            let mut name = fixed()?;
            if name == "DstASPath" {
                name = "DstAS".into();
            }
            (asn(&name, prefix, limit, database), Rows::LabelDetail)
        }
        "srcport" | "dstport" => (port(&fixed()?, prefix, limit, database), Rows::LabelDetail),
        "srcnetname" | "dstnetname" | "srcnetrole" | "dstnetrole" | "srcnetsite" | "dstnetsite" | "srcnetregion"
        | "dstnetregion" | "srcnettenant" | "dstnettenant" => {
            (network_attribute(&fixed()?, prefix, limit), label(&format!("network {}", &lower[6..]), true))
        }
        "icmpv4" | "icmpv6" => {
            let name = fixed()?;
            (icmp(&name, prefix, limit, database), label(&name, true))
        }
        "exportername" | "exportergroup" | "exporterrole" | "exportersite" | "exporterregion" | "exportertenant" => {
            (exporter_attribute(&fixed()?, prefix, limit), label(&format!("exporter {}", &lower[8..]), true))
        }
        "inifname" | "outifname" => (exporter_attribute("IfName", prefix, limit), label("interface name", true)),
        "inifdescription" | "outifdescription" => {
            (exporter_attribute("IfDescription", prefix, limit), label("interface description", true))
        }
        "inifconnectivity" | "outifconnectivity" => {
            (exporter_attribute("IfConnectivity", prefix, limit), label("connectivity type", true))
        }
        "inifprovider" | "outifprovider" => {
            (exporter_attribute("IfProvider", prefix, limit), label("provider name", true))
        }
        _ => return None,
    })
}
