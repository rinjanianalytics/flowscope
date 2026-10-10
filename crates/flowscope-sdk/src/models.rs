// SPDX-FileCopyrightText: 2026 FlowScope contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Types of the console API. The field names are the ones of the JSON.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use utoipa::ToSchema;

fn is_zero(v: &u8) -> bool {
    *v == 0
}

fn is_false(v: &bool) -> bool {
    !*v
}

/// Answer to a request the server refused.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ErrorMessage {
    pub message: String,
}

/// How a graph is displayed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum GraphType {
    #[default]
    Stacked,
    Stacked100,
    Lines,
    Grid,
    Heatmap,
    Sankey,
}

/// What the rows are sorted by to keep the top ones.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum LimitType {
    /// Average over the time range.
    #[default]
    Avg,
    /// Highest value over the time range.
    Max,
    /// Value at the end of the time range.
    Last,
}

/// Unit of the values of a graph.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum Units {
    /// Layer 3 bits per second.
    #[default]
    #[serde(rename = "l3bps")]
    L3bps,
    /// Layer 2 bits per second.
    #[serde(rename = "l2bps")]
    L2bps,
    /// Packets per second.
    #[serde(rename = "pps")]
    Pps,
    /// Flows per second.
    #[serde(rename = "fps")]
    Fps,
    /// Share of the input interface capacity, in percent.
    #[serde(rename = "inl2%")]
    InL2Percent,
    /// Share of the output interface capacity, in percent.
    #[serde(rename = "outl2%")]
    OutL2Percent,
}

/// Defaults of the "visualize" page.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct VisualizeOptions {
    pub graph_type: GraphType,
    /// Start of the time range, as a date or as words ("6 hours ago").
    pub start: String,
    /// End of the time range, as a date or as words ("now").
    pub end: String,
    pub filter: String,
    pub dimensions: Vec<String>,
    pub limit: u32,
    pub limit_type: LimitType,
    pub bidirectional: bool,
    pub previous_period: bool,
}

/// Answer of `/configuration`: what the web interface needs to start.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Configuration {
    pub version: String,
    pub default_visualize_options: VisualizeOptions,
    /// Columns usable as dimensions.
    pub dimensions: Vec<String>,
    /// Highest value accepted for `limit`.
    pub dimensions_limit: u32,
    /// Dimensions holding addresses, which can be truncated to a prefix.
    pub truncatable: Vec<String>,
    /// Names of the "top" widgets of the home page.
    pub homepage_top_widgets: Vec<String>,
    pub branding: bool,
}

/// Answer of `/user/info`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct UserInfo {
    pub login: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub email: Option<String>,
    #[serde(default, rename = "logout-url", skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub logout_url: Option<String>,
    #[serde(default, rename = "avatar-url", skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub avatar_url: Option<String>,
}

/// What the line and sankey graphs have in common. It is the input of
/// `/graph/sankey`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct GraphInput {
    /// Start of the time range (RFC 3339).
    #[schema(format = DateTime)]
    pub start: String,
    /// End of the time range (RFC 3339).
    #[schema(format = DateTime)]
    pub end: String,
    /// Columns to group by. The sankey graph needs at least one.
    #[serde(default)]
    pub dimensions: Vec<String>,
    /// Number of rows to keep. The other ones are summed as "Other".
    pub limit: u32,
    #[serde(default, rename = "limitType")]
    pub limit_type: LimitType,
    /// Prefix length IPv4 addresses are truncated to. 0 keeps them whole.
    #[serde(default, rename = "truncate-v4", skip_serializing_if = "is_zero")]
    #[schema(maximum = 32)]
    pub truncate_v4: u8,
    /// Prefix length IPv6 addresses are truncated to. 0 keeps them whole.
    #[serde(default, rename = "truncate-v6", skip_serializing_if = "is_zero")]
    #[schema(maximum = 128)]
    pub truncate_v6: u8,
    /// Filter expression. Empty to keep all flows.
    #[serde(default)]
    pub filter: String,
    pub units: Units,
    /// Also graph the opposite direction.
    #[serde(default)]
    pub bidirectional: bool,
}

/// Input of `/graph/line`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct LineInput {
    #[serde(flatten)]
    pub graph: GraphInput,
    /// Minimum number of points on the time axis.
    #[schema(minimum = 5, maximum = 2000)]
    pub points: u32,
    /// Also graph the period before (previous hour, day, week...).
    #[serde(default, rename = "previous-period", skip_serializing_if = "is_false")]
    pub previous_period: bool,
}

/// Answer of `/graph/line`. A row is a set of dimension values. The arrays
/// `rows`, `points`, `axis` and the statistics share their indexes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct LineOutput {
    /// Times of the points.
    #[schema(value_type = Vec<String>, format = DateTime)]
    pub t: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// For each row, its value at each time.
    pub points: Vec<Vec<i64>>,
    /// For each row, its axis: 1 for the direct direction, 2 for the reverse
    /// one, 3 and 4 for the same ones in the previous period.
    pub axis: Vec<u8>,
    /// Display names of the axes.
    #[serde(rename = "axis-names")]
    pub axis_names: BTreeMap<String, String>,
    pub average: Vec<i64>,
    /// Sum over the time range. Absent for the percent units.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub total: Option<Vec<i64>>,
    pub min: Vec<i64>,
    pub max: Vec<i64>,
    pub last: Vec<i64>,
    #[serde(rename = "95th")]
    pub percentile_95: Vec<i64>,
}

/// A node of the sankey graph.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SankeyNode {
    /// Dimension and value ("SrcAS: 15169").
    pub name: String,
    pub axis: u8,
}

/// A link between two nodes of the sankey graph.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SankeyLink {
    pub source: String,
    pub target: String,
    pub xps: i64,
    pub axis: u8,
}

/// Answer of `/graph/sankey`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SankeyOutput {
    pub rows: Vec<Vec<String>>,
    /// For each row, its average value.
    pub xps: Vec<i64>,
    pub axis: Vec<u8>,
    #[serde(rename = "axis-names")]
    pub axis_names: BTreeMap<String, String>,
    pub nodes: Vec<SankeyNode>,
    pub links: Vec<SankeyLink>,
}

/// Input of `/graph/table-interval`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct TableIntervalInput {
    #[schema(format = DateTime)]
    pub start: String,
    #[schema(format = DateTime)]
    pub end: String,
    #[schema(minimum = 5, maximum = 2000)]
    pub points: u32,
}

/// Answer of `/graph/table-interval`: the table a graph would be computed
/// from and the time between two points.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct TableIntervalOutput {
    pub table: String,
    /// In seconds.
    pub interval: u64,
}

/// Input and answer of `/graph/reverse`: dimensions and a filter, to swap
/// between source and destination, or input and output.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Reverse {
    #[serde(default)]
    pub dimensions: Vec<String>,
    #[serde(default)]
    pub filter: String,
}

/// Input of `/filter/validate`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ValidateInput {
    pub filter: String,
}

/// An error in a filter expression.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct FilterError {
    pub message: String,
    /// Starts at 1.
    pub line: u32,
    /// Starts at 1.
    pub column: u32,
    /// Starts at 0.
    pub offset: u32,
}

/// Answer of `/filter/validate`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ValidateOutput {
    /// "ok", or the first error.
    pub message: String,
    /// The expression as the server understood it, when valid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub parsed: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<FilterError>,
}

impl ValidateOutput {
    /// Tell if the expression is valid.
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty() && self.parsed.is_some()
    }
}

/// What to complete in a filter expression.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum CompleteWhat {
    #[default]
    Column,
    Operator,
    Value,
}

/// Input of `/filter/complete`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CompleteInput {
    pub what: CompleteWhat,
    /// Column the operator or the value is for. Needed unless completing a
    /// column name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub column: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub operator: Option<String>,
    /// What the user typed so far.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub prefix: Option<String>,
    /// Highest number of answers (20 by default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub limit: Option<u32>,
}

/// A way to complete a filter expression.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Completion {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub detail: Option<String>,
    /// Tell if the label must be written between quotes.
    pub quoted: bool,
}

/// Answer of `/filter/complete`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CompleteOutput {
    pub completions: Vec<Completion>,
}

/// A saved filter.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SavedFilter {
    pub id: u64,
    /// Login of the owner. "__system" for the filters of the configuration.
    pub user: String,
    /// Tell if the other users see it.
    pub shared: bool,
    pub description: String,
    pub content: String,
}

/// Input of `POST /filter/saved`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SavedFilterInput {
    pub description: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub shared: bool,
}

/// Answer of `GET /filter/saved`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SavedFilters {
    pub filters: Vec<SavedFilter>,
}

/// Answer of `/widget/flow-rate`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct FlowRate {
    /// Always "second".
    pub period: String,
    pub rate: f64,
}

/// Answer of `/widget/exporters`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Exporters {
    pub exporters: Vec<String>,
}

/// An entry of a "top" widget.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct TopEntry {
    pub name: String,
    pub percent: f64,
}

/// Answer of `/widget/top/{name}`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Top {
    pub top: Vec<TopEntry>,
}

/// A point of the traffic graph of the home page.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct TrafficPoint {
    #[schema(format = DateTime)]
    pub t: String,
    pub gbps: f64,
}

/// Answer of `/widget/graph`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Traffic {
    pub data: Vec<TrafficPoint>,
}

/// Answer of `/widget/flow-last`: the columns of the most recent flow. The
/// columns depend on the schema.
pub type LastFlow = serde_json::Map<String, serde_json::Value>;

/// A header of a documentation page.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DocumentHeader {
    pub level: u32,
    /// Anchor of the header in the page.
    pub id: String,
    pub title: String,
}

/// The headers of a documentation page.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DocumentToc {
    pub name: String,
    pub section: String,
    pub headers: Vec<DocumentHeader>,
}

/// Answer of `/docs/{name}`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Document {
    /// The page, as HTML.
    pub markdown: String,
    /// The headers of all pages.
    pub toc: Vec<DocumentToc>,
}
