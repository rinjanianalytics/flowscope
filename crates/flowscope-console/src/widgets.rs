// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! SQL for the widgets of the home page.

use crate::clickhouse::{self as ch, FlowsTable, InputContext, Nanos, Time, seconds};
use crate::query;
use crate::sqlbuilder::{self as sb, Expr, PREC_ATOM, PREC_MULTIPLICATION, Query};
use flowscope_schema::{ColumnKey, DICTIONARY_ASNS, DICTIONARY_PROTOCOLS, Schema};
use serde::{Deserialize, Serialize};

/// A top widget of the home page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TopWidget {
    SrcAS,
    DstAS,
    SrcCountry,
    DstCountry,
    SrcPort,
    DstPort,
    Exporter,
    Protocol,
    Etype,
}

impl TopWidget {
    pub const ALL: [TopWidget; 9] = [
        TopWidget::SrcAS,
        TopWidget::DstAS,
        TopWidget::SrcCountry,
        TopWidget::DstCountry,
        TopWidget::SrcPort,
        TopWidget::DstPort,
        TopWidget::Exporter,
        TopWidget::Protocol,
        TopWidget::Etype,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            TopWidget::SrcAS => "src-as",
            TopWidget::DstAS => "dst-as",
            TopWidget::SrcCountry => "src-country",
            TopWidget::DstCountry => "dst-country",
            TopWidget::SrcPort => "src-port",
            TopWidget::DstPort => "dst-port",
            TopWidget::Exporter => "exporter",
            TopWidget::Protocol => "protocol",
            TopWidget::Etype => "etype",
        }
    }

    /// Parse a widget name, case-insensitively.
    pub fn parse(s: &str) -> Result<TopWidget, String> {
        let lower = s.to_lowercase();
        TopWidget::ALL
            .into_iter()
            .find(|w| w.as_str() == lower)
            .ok_or_else(|| format!("{s} does not belong to HomepageTopWidget values"))
    }

    /// The widget is about the destination.
    fn is_destination(self) -> bool {
        matches!(self, TopWidget::DstAS | TopWidget::DstCountry | TopWidget::DstPort)
    }
}

impl Serialize for TopWidget {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for TopWidget {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        TopWidget::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// The query of a top widget, and the table it runs on. The filter is
/// written for the source direction, it is reversed for the widgets about
/// the destination.
pub fn top_sql(
    widget: TopWidget,
    filter: &query::Filter,
    tables: &[FlowsTable],
    now: &Time,
    database: &str,
) -> (Query, String) {
    let dict_lookup =
        |dictionary: &str, column: &str| query::dictionary_lookup(database, dictionary, sb::column(column), "???");
    let mut main_table_required = false;
    let (selector, mut group_by) = match widget {
        TopWidget::SrcAS | TopWidget::DstAS => {
            let column = if widget == TopWidget::DstAS { "DstAS" } else { "SrcAS" };
            let selector = sb::function(
                "concat",
                vec![
                    sb::function("toString", vec![sb::column(column)]),
                    sb::string(": "),
                    dict_lookup(DICTIONARY_ASNS, column),
                ],
            );
            (selector, sb::columns(&[column]))
        }
        TopWidget::SrcCountry => (sb::column("SrcCountry"), vec![]),
        TopWidget::DstCountry => (sb::column("DstCountry"), vec![]),
        TopWidget::Exporter => (sb::column("ExporterName"), vec![]),
        TopWidget::Protocol => (dict_lookup(DICTIONARY_PROTOCOLS, "Proto"), sb::columns(&["Proto"])),
        TopWidget::Etype => {
            let etype = sb::column("EType");
            let selector = sb::function(
                "if",
                vec![
                    sb::function("equals", vec![etype.clone(), sb::uint(0x86dd)]),
                    sb::string("IPv6"),
                    sb::function(
                        "if",
                        vec![
                            sb::function("equals", vec![etype, sb::uint(0x800)]),
                            sb::string("IPv4"),
                            sb::string("???"),
                        ],
                    ),
                ],
            );
            (selector, sb::columns(&["EType"]))
        }
        TopWidget::SrcPort | TopWidget::DstPort => {
            let column = if widget == TopWidget::DstPort { "DstPort" } else { "SrcPort" };
            let selector = sb::function(
                "concat",
                vec![
                    dict_lookup(DICTIONARY_PROTOCOLS, "Proto"),
                    sb::string("/"),
                    sb::function("toString", vec![sb::column(column)]),
                ],
            );
            main_table_required = true;
            (selector, sb::columns(&["Proto", column]))
        }
    };
    let condition = if widget.is_destination() { filter.reverse() } else { filter.direct() };
    main_table_required = main_table_required || filter.main_table_required();
    if group_by.is_empty() {
        group_by = vec![selector.clone()];
    }

    let start = ch::add(now, -5 * 60 * ch::SECOND);
    let r =
        ch::resolve(tables, &InputContext { start, end: *now, main_table_required, points: 5 }).for_range(&start, now);
    let r#where = sb::and([Some(r.timefilter()), condition].into_iter().flatten());
    let bytes = sb::raw("SUM(Bytes * SamplingRate)", PREC_ATOM);
    let query = sb::select(vec![
        sb::alias(
            sb::function("if", vec![sb::function("empty", vec![selector.clone()]), sb::string("Unknown"), selector]),
            "Name",
        ),
        sb::alias(sb::op(sb::op(bytes.clone(), "/", sb::column("Total")), "*", sb::uint(100)), "Percent"),
    ])
    .with_scalar(sb::select(vec![bytes]).from(sb::table(&r.table, None)).r#where(r#where.clone()), "Total")
    .from(sb::table(&r.table, None))
    .r#where(r#where)
    .group_by(group_by)
    .order_by(vec![sb::order(sb::column("Percent")).desc()])
    .limit(5);
    (query, r.table)
}

/// The query of the graph of the home page, and the table it runs on.
pub fn graph_sql(filter: &query::Filter, time_range: Nanos, tables: &[FlowsTable], now: &Time) -> (Query, String) {
    let start = ch::add(now, -time_range);
    let r = ch::resolve(
        tables,
        &InputContext { start, end: *now, main_table_required: filter.main_table_required(), points: 200 },
    )
    .for_range(&start, now);
    let mut gbps: Expr = sb::function(
        "SUM",
        vec![sb::op(sb::raw("Bytes * SamplingRate * 8", PREC_MULTIPLICATION), "/", sb::uint(r.interval))],
    );
    // From bits per second to gigabits per second.
    for _ in 0..3 {
        gbps = sb::op(gbps, "/", sb::uint(1000));
    }
    let query = sb::select(vec![sb::alias(r.to_start_of_interval(), "Time"), sb::alias(gbps, "Gbps")])
        .from(sb::table(&r.table, None))
        .r#where(sb::and([Some(r.timefilter()), filter.direct()].into_iter().flatten()))
        .group_by(vec![sb::column("Time")])
        .order_by(vec![sb::order(sb::column("Time")).fill(
            r.timefilter_start(),
            sb::op(r.timefilter_end(), "+", seconds(1)),
            sb::uint(r.interval),
        )]);
    (query, r.table)
}

/// The query for the last flow. The columns not readable as they are stored
/// are replaced by a friendlier form.
pub fn flow_last_sql(schema: &Schema) -> Query {
    let replace = [
        (ColumnKey::SrcCommunities, query::communities_to_strings("SrcCommunities")),
        (ColumnKey::SrcLargeCommunities, query::large_communities_to_strings("SrcLargeCommunities")),
        (ColumnKey::DstCommunities, query::communities_to_strings("DstCommunities")),
        (ColumnKey::DstLargeCommunities, query::large_communities_to_strings("DstLargeCommunities")),
        (ColumnKey::SrcMAC, sb::function("MACNumToString", vec![sb::column("SrcMAC")])),
        (ColumnKey::DstMAC, sb::function("MACNumToString", vec![sb::column("DstMAC")])),
    ];
    let mut except = vec![];
    let mut replaced = vec![];
    for (key, with) in replace {
        if schema.lookup_column_by_key(key).is_some_and(|c| !c.disabled) {
            except.push(key.name());
            replaced.push(sb::alias(with, &key.name()));
        }
    }
    let modifier = (!except.is_empty()).then_some(sb::Modifier::Except(except));
    let mut last = sb::select(vec![]).item(sb::star(), modifier);
    for expr in replaced {
        last = last.item(expr, None);
    }
    last.from(sb::table("flows", None))
        .r#where(Some(sb::op(
            sb::column("TimeReceived"),
            "=",
            sb::select(vec![sb::function("MAX", vec![sb::column("TimeReceived")])])
                .from(sb::table("flows", None))
                .subquery(),
        )))
        .limit(1)
}

/// The query for the flow rate.
pub const FLOW_RATE_SQL: &str =
    "SELECT COUNT(*)/300 AS rate FROM flows WHERE TimeReceived > date_sub(minute, 5, now())";

/// The query for the list of exporters.
pub const EXPORTERS_SQL: &str = "SELECT ExporterName FROM exporters GROUP BY ExporterName ORDER BY ExporterName";
