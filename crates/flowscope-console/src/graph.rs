// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! SQL for the line and sankey graphs.

use crate::clickhouse::{
    self as ch, HOUR, InputContext, Nanos, Resolution, Resolved, SECOND, Time, reverse_units, seconds, units_expr,
    units_weight_expr,
};
use crate::query;
use crate::sqlbuilder::{self as sb, Expr, Query};
use flowscope_schema::Schema;
use serde::Deserialize;
use std::sync::Arc;

/// What the line and sankey graphs have in common.
#[derive(Clone, Debug, Deserialize)]
pub struct CommonInput {
    #[serde(skip, default = "empty_schema")]
    pub schema: Arc<Schema>,
    #[serde(skip)]
    pub database: String,
    pub start: Time,
    pub end: Time,
    #[serde(default)]
    pub dimensions: Vec<query::Column>,
    #[serde(default)]
    pub limit: i64,
    #[serde(default, rename = "limitType")]
    pub limit_type: String,
    #[serde(default)]
    pub filter: query::Filter,
    /// 0 or 32 means no truncation.
    #[serde(default, rename = "truncate-v4")]
    pub truncate_v4: i64,
    /// 0 or 128 means no truncation.
    #[serde(default, rename = "truncate-v6")]
    pub truncate_v6: i64,
    #[serde(default)]
    pub units: String,
}

fn empty_schema() -> Arc<Schema> {
    Arc::new(Schema::default())
}

/// Input of /graph/line.
#[derive(Clone, Debug, Deserialize)]
pub struct LineInput {
    #[serde(flatten)]
    pub common: CommonInput,
    /// Minimum number of points.
    #[serde(default)]
    pub points: u64,
    #[serde(default)]
    pub bidirectional: bool,
    #[serde(default, rename = "previous-period")]
    pub previous_period: bool,
}

/// Input of /graph/sankey.
#[derive(Clone, Debug, Deserialize)]
pub struct SankeyInput {
    #[serde(flatten)]
    pub common: CommonInput,
    #[serde(default)]
    pub bidirectional: bool,
}

/// Cut an address down to the network of a prefix length.
fn truncate_ip(addr: Expr, bits: Expr) -> Expr {
    sb::function("tupleElement", vec![sb::function("IPv6CIDRToRange", vec![addr, bits]), sb::uint(1)])
}

/// Tell if the main table is needed for the columns and the filter.
pub fn require_main_table(schema: &Schema, columns: &[query::Column], filter: &query::Filter) -> bool {
    filter.main_table_required()
        || columns.iter().any(|c| schema.lookup_column_by_key(c.key()).is_some_and(|c| c.clickhouse_main_only))
}

impl CommonInput {
    /// The query used as a source for the data, truncating the addresses.
    pub fn source_select(&self, table: &str) -> Query {
        let v4 = if self.truncate_v4 == 0 { 32 } else { self.truncate_v4 };
        let v6 = if self.truncate_v6 == 0 { 128 } else { self.truncate_v6 };
        let mut truncated = vec![];
        for qc in &self.dimensions {
            let truncatable = self.schema.lookup_column_by_key(qc.key()).is_some_and(|c| c.console_truncate_ip);
            if !truncatable || (v4 == 32 && v6 == 128) {
                continue;
            }
            let addr = sb::column(qc.name());
            let mut bits = sb::uint(v6 as u64);
            if v6 != v4 + 96 {
                // Addresses are stored as IPv6 ones, the prefix length
                // depends on the family of each address.
                bits = sb::function(
                    "if",
                    vec![
                        sb::op(
                            truncate_ip(addr.clone(), sb::uint(96)),
                            "=",
                            sb::function("toIPv6", vec![sb::string("0.0.0.0")]),
                        ),
                        sb::uint((v4 + 96) as u64),
                        sb::uint(v6 as u64),
                    ],
                );
            }
            truncated.push(sb::alias(truncate_ip(addr, bits), qc.name()));
        }
        let modifier = (!truncated.is_empty()).then_some(sb::Modifier::Replace(truncated));
        sb::select(vec![])
            .item(sb::star(), modifier)
            .from(sb::table(table, None))
            .setting("asterisk_include_alias_columns", sb::uint(1))
    }

    /// The query picking the dimension values to display. The other values
    /// are grouped as "Other".
    fn select_rows_by_limit_type(&self, r: &Resolved, dimensions: &[Expr], r#where: &Expr, units: &Expr) -> Query {
        let rows = sb::select(dimensions.to_vec()).group_by(dimensions.to_vec()).limit(self.limit);
        if self.limit_type == "max" {
            // Rank on the highest value reached during one interval.
            let mut items = vec![sb::alias(r.to_start_of_interval(), "time")];
            items.extend(dimensions.iter().cloned());
            items.push(sb::alias(units.clone(), "sum_at_time"));
            let mut group = vec![sb::column("time")];
            group.extend(dimensions.iter().cloned());
            return rows
                .from_select(
                    sb::select(items).from(sb::table("source", None)).r#where(Some(r#where.clone())).group_by(group),
                )
                .order_by(vec![sb::order(sb::function("MAX", vec![sb::column("sum_at_time")])).desc()]);
        }
        if self.limit_type == "last" {
            // Rank on the traffic of the last interval only.
            return rows
                .from(sb::table("source", None))
                .r#where(sb::and([
                    r#where.clone(),
                    sb::op(sb::column("TimeReceived"), ">=", sb::op(r.timefilter_end(), "-", seconds(r.interval))),
                ]))
                .order_by(vec![sb::order(units.clone()).desc()]);
        }
        // On the main table, an exact ranking keeps one entry for each
        // dimension value. topKWeighted keeps a bounded number of candidates
        // instead. A limit of 0 is refused by topKWeighted.
        if r.table == "flows"
            && self.limit > 0
            && let Some(weight) = units_weight_expr(&self.units)
        {
            return self.select_rows_by_top_k(dimensions, r#where, weight);
        }
        rows.from(sb::table("source", None))
            .r#where(Some(r#where.clone()))
            .order_by(vec![sb::order(units.clone()).desc()])
    }

    /// Rank the dimension values with an approximate top-N, with one column
    /// per dimension like the exact query.
    fn select_rows_by_top_k(&self, dimensions: &[Expr], r#where: &Expr, weight: Expr) -> Query {
        let top = sb::select(vec![sb::alias(
            sb::function(
                "arrayJoin",
                vec![sb::parametric_function(
                    "topKWeighted",
                    vec![sb::int(self.limit), sb::uint(TOP_K_WEIGHTED_LOAD_FACTOR)],
                    vec![sb::function("tuple", dimensions.to_vec()), sb::function("toUInt64", vec![weight])],
                )],
            ),
            "tk",
        )])
        .from(sb::table("source", None))
        .r#where(Some(r#where.clone()));
        let fields = self
            .dimensions
            .iter()
            .enumerate()
            .map(|(idx, c)| {
                sb::alias(sb::function("tupleElement", vec![sb::column("tk"), sb::uint(idx as u64 + 1)]), c.name())
            })
            .collect();
        sb::select(fields).from_select(top)
    }

    /// The input for the opposite direction.
    fn reverse_direction(&self) -> CommonInput {
        let mut input = self.clone();
        input.filter.swap();
        query::reverse_columns(&mut input.dimensions, &self.schema);
        input
    }
}

/// How many candidates topKWeighted keeps for each requested row.
const TOP_K_WEIGHTED_LOAD_FACTOR: u64 = 20;

/// The period matching a period length, and its name. The year is a special
/// case (0) as it has no fixed length.
pub fn nearest_period(period: Nanos) -> (Nanos, &'static str) {
    let day = 24 * HOUR;
    if period < 2 * HOUR {
        (HOUR, "hour")
    } else if period < 2 * day {
        (day, "day")
    } else if period < 2 * 7 * day {
        (7 * day, "week")
    } else if period < 2 * 4 * 7 * day {
        // 4 weeks, not 1 month
        (4 * 7 * day, "month")
    } else {
        (0, "year")
    }
}

#[derive(Default)]
struct LineOptions {
    skip_with_clause: bool,
    reverse_direction: bool,
    /// How far back the range of this axis was moved.
    shifted_by: Nanos,
}

impl LineInput {
    fn reverse_direction(&self) -> LineInput {
        LineInput { common: self.common.reverse_direction(), ..self.clone() }
    }

    /// The input for the previous period, without dimensions, and by how
    /// much it moved.
    fn previous_period(&self) -> (LineInput, Nanos) {
        let mut input = self.clone();
        input.common.dimensions = vec![];
        let (mut period, _) = nearest_period(ch::sub(&self.common.end, &self.common.start));
        if period == 0 {
            // A full year, measured from the start of the period.
            period = ch::sub(&self.common.start, &ch::one_year_before(&self.common.start));
        }
        input.common.start = ch::add(&self.common.start, -period);
        input.common.end = ch::add(&self.common.end, -period);
        (input, period)
    }

    fn to_sql1(&self, axis: i64, r: &Resolved, options: LineOptions) -> Query {
        let input = &self.common;
        let r#where = r.r#where(&input.filter);
        // The previous period is drawn on the time axis of the main one.
        let shift = |expr: Expr| {
            if options.shifted_by == 0 {
                expr
            } else {
                sb::op(expr, "+", seconds((options.shifted_by / SECOND) as u64))
            }
        };
        let units = if options.reverse_direction { reverse_units(&input.units) } else { &input.units };
        let units_sql = units_expr(units);

        let mut select_fields = vec![];
        let mut dimensions = vec![];
        let mut others = vec![];
        for column in &input.dimensions {
            select_fields.push(column.to_sql_select(&input.schema, &input.database));
            dimensions.push(sb::column(column.name()));
            others.push(sb::string("Other"));
        }
        // Rows outside the selected ones are folded into "Other". The same
        // value fills the gaps left by WITH FILL.
        let others_array =
            || if dimensions.is_empty() { sb::function("emptyArrayString", vec![]) } else { sb::array(others.clone()) };
        let dimensions_field = if dimensions.is_empty() {
            others_array()
        } else {
            sb::function(
                "if",
                vec![
                    sb::op(sb::tuple(dimensions.clone()), "IN", sb::column("rows")),
                    sb::array(select_fields),
                    others_array(),
                ],
            )
        };

        let inner = sb::select(vec![
            sb::alias(shift(r.to_start_of_interval()), "time"),
            sb::alias(sb::op(units_sql.clone(), "/", sb::uint(r.interval)), "xps"),
            sb::alias(dimensions_field, "dimensions"),
        ])
        .from(sb::table("source", None))
        .r#where(Some(r#where.clone()))
        .group_by(vec![sb::column("time"), sb::column("dimensions")])
        .order_by(vec![sb::order(sb::column("time")).fill(
            shift(r.timefilter_start()),
            shift(sb::op(r.timefilter_end(), "+", seconds(1))),
            sb::uint(r.interval),
        )])
        .interpolate("dimensions", others_array());

        let mut query = sb::select(vec![sb::alias(sb::int(axis), "axis"), sb::star()]).from_select(inner);
        if !options.skip_with_clause {
            query = query.with("source", input.source_select(&r.table));
            if !dimensions.is_empty() {
                query = query.with("rows", input.select_rows_by_limit_type(r, &dimensions, &r#where, &units_sql));
            }
        }
        query
    }

    /// What is needed to select the table.
    pub fn resolve_context(&self) -> InputContext {
        let c = &self.common;
        InputContext {
            start: c.start,
            end: c.end,
            main_table_required: require_main_table(&c.schema, &c.dimensions, &c.filter),
            points: self.points,
        }
    }

    /// The queries, one per axis.
    pub fn to_sql(&self, res: &Resolution) -> Vec<Query> {
        // The previous period reuses the main range, moved back in time, so
        // both cover the same number of points.
        let main = res.for_range(&self.common.start, &self.common.end);
        let mut queries = vec![self.to_sql1(1, &main, LineOptions::default())];
        if self.bidirectional {
            queries.push(self.reverse_direction().to_sql1(
                2,
                &main,
                LineOptions { skip_with_clause: true, reverse_direction: true, ..Default::default() },
            ));
        }
        if self.previous_period {
            let (previous, period) = self.previous_period();
            queries.push(previous.to_sql1(
                3,
                &main.shift_back(period),
                LineOptions { skip_with_clause: true, shifted_by: period, ..Default::default() },
            ));
        }
        if self.bidirectional && self.previous_period {
            let (previous, period) = self.reverse_direction().previous_period();
            queries.push(previous.to_sql1(
                4,
                &main.shift_back(period),
                LineOptions { skip_with_clause: true, reverse_direction: true, shifted_by: period },
            ));
        }
        queries
    }
}

/// Combine the per-axis queries into one statement. Only the first axis has
/// the WITH clause.
pub fn union_all(queries: Vec<Query>) -> Query {
    let mut queries = queries.into_iter();
    let first = queries.next().expect("at least one query");
    queries.fold(first, |acc, q| acc.union_all(q))
}

#[derive(Default)]
struct SankeyOptions<'a> {
    skip_with_clause: bool,
    reverse_direction: bool,
    /// For each dimension, the column of the forward `rows` CTE matching it.
    rows_columns: Option<&'a [query::Column]>,
}

impl SankeyInput {
    fn to_sql1(&self, axis: i64, res: &Resolution, options: SankeyOptions) -> Query {
        let input = &self.common;
        let r = res.for_range(&input.start, &input.end);
        let r#where = r.r#where(&input.filter);
        let rows_columns = options.rows_columns.unwrap_or(&input.dimensions);
        let units = if options.reverse_direction { reverse_units(&input.units) } else { &input.units };
        let units_sql = units_expr(units);

        let mut array_fields = vec![];
        let mut dimensions = vec![];
        for (i, column) in input.dimensions.iter().enumerate() {
            array_fields.push(sb::function(
                "if",
                vec![
                    sb::op(
                        sb::column(column.name()),
                        "IN",
                        sb::select(vec![sb::column(rows_columns[i].name())]).from(sb::table("rows", None)).subquery(),
                    ),
                    column.to_sql_select(&input.schema, &input.database),
                    sb::string("Other"),
                ],
            ));
            dimensions.push(sb::column(column.name()));
        }

        let inner = sb::select(vec![
            sb::alias(sb::op(units_sql.clone(), "/", sb::column("range")), "xps"),
            sb::alias(sb::array(array_fields), "dimensions"),
        ])
        .from(sb::table("source", None))
        .r#where(Some(r#where.clone()))
        .group_by(vec![sb::column("dimensions")])
        .order_by(vec![sb::order(sb::column("xps")).desc()]);

        let mut query = sb::select(vec![sb::alias(sb::int(axis), "axis"), sb::star()]).from_select(inner);
        if !options.skip_with_clause {
            // No time axis: the traffic is averaged over the period covered
            // by the data.
            let range = sb::select(vec![sb::op(
                sb::function("MAX", vec![sb::column("TimeReceived")]),
                "-",
                sb::function("MIN", vec![sb::column("TimeReceived")]),
            )])
            .from(sb::table("source", None))
            .r#where(Some(r#where.clone()));
            query = query
                .with("source", input.source_select(&r.table))
                .with_scalar(range, "range")
                .with("rows", input.select_rows_by_limit_type(&r, &dimensions, &r#where, &units_sql));
        }
        query
    }

    /// What is needed to select the table. There is no time axis, the number
    /// of points only steers towards a reasonably consolidated table.
    pub fn resolve_context(&self) -> InputContext {
        let c = &self.common;
        InputContext {
            start: c.start,
            end: c.end,
            main_table_required: require_main_table(&c.schema, &c.dimensions, &c.filter),
            points: 20,
        }
    }

    /// The queries, one per axis.
    pub fn to_sql(&self, res: &Resolution) -> Vec<Query> {
        let mut queries = vec![self.to_sql1(1, res, SankeyOptions::default())];
        if self.bidirectional {
            let reversed = SankeyInput { common: self.common.reverse_direction(), ..self.clone() };
            queries.push(reversed.to_sql1(
                2,
                res,
                SankeyOptions {
                    skip_with_clause: true,
                    reverse_direction: true,
                    rows_columns: Some(&self.common.dimensions),
                },
            ));
        }
        queries
    }
}
