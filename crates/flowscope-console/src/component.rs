// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! The console component: the API under /api/v0/console.

// Handlers return an early response as their error.
#![allow(clippy::result_large_err)]

use crate::auth::{self, Authentication, UserInformation};
use crate::clickhouse::{self as ch, FlowsTable, InputContext, SECOND, Time};
use crate::completion;
use crate::config::Configuration;
use crate::database::{Database, SavedFilter};
use crate::filter;
use crate::graph::{LineInput, SankeyInput, union_all};
use crate::output::{LineRow, SankeyRow, go_time, line_output, sankey_output};
use crate::query;
use crate::widgets::{self, TopWidget};
use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Router, middleware};
use chrono::{DateTime, Utc};
use flowscope_clickhouse::{QueryResult, Value};
use flowscope_common::helpers::FLOWSCOPE_VERSION;
use flowscope_common::httpserver::{HttpServer, json_error, json_response};
use flowscope_common::reporter::{CounterVec, Reporter};
use flowscope_schema::{ColumnKey, Schema};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// Access to ClickHouse, behind a trait for the tests.
pub trait ClickHouseQuery: Send + Sync + 'static {
    fn query(&self, sql: String) -> Pin<Box<dyn Future<Output = Result<QueryResult, String>> + Send>>;
}

/// The ClickHouse component, as used by the console.
pub struct ClickHouseDbQuery(pub Arc<flowscope_common::clickhousedb::ClickHouseDb>);

impl ClickHouseQuery for ClickHouseDbQuery {
    fn query(&self, sql: String) -> Pin<Box<dyn Future<Output = Result<QueryResult, String>> + Send>> {
        // The pool is synchronous: use a blocking thread.
        let db = self.0.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || db.with_conn(|c| c.query(&sql, &[])).map_err(|e| e.to_string()))
                .await
                .map_err(|e| e.to_string())?
        })
    }
}

/// What the console needs.
#[derive(Clone)]
pub struct Dependencies {
    pub clickhouse: Arc<dyn ClickHouseQuery>,
    /// Database holding the dictionaries.
    pub database_name: String,
    pub schema: Arc<Schema>,
    pub auth: Authentication,
    pub database: Database,
    /// Current time, for the tests.
    pub clock: Arc<dyn Fn() -> Time + Send + Sync>,
}

/// The console component.
pub struct Console {
    config: Configuration,
    deps: Dependencies,
    flows_tables: RwLock<Vec<FlowsTable>>,
    clickhouse_queries: CounterVec,
    docs: crate::docs::Docs,
}

type Shared = State<Arc<Console>>;

/// Capitalize the first letter, like Go's helpers.Capitalize.
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn bad_request(message: impl AsRef<str>) -> Response {
    json_error(StatusCode::BAD_REQUEST, capitalize(message.as_ref()))
}

/// JSON with an indentation of 4 spaces, like Go's WriteIndentedJSON.
fn indented_json(status: StatusCode, value: &Json) -> Response {
    let mut out = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
    let mut ser = serde_json::Serializer::with_formatter(&mut out, formatter);
    if value.serialize(&mut ser).is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    out.push(b'\n');
    let mut resp = (status, out).into_response();
    resp.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json; charset=utf-8"));
    resp
}

/// A float as Go encodes it: whole numbers without a fraction.
fn go_float(f: f64) -> Json {
    if f.fract() == 0.0 && f.abs() < 1e21 { json!(f as i64) } else { json!(f) }
}

/// 204, with the content type Go sets anyway.
fn no_content() -> Response {
    (StatusCode::NO_CONTENT, [(header::CONTENT_TYPE, "application/json; charset=utf-8")]).into_response()
}

/// Decode a JSON body. Go answers "empty body" when there is nothing.
fn bind_json<T: DeserializeOwned>(body: &Bytes) -> Result<T, Response> {
    if body.is_empty() {
        return Err(bad_request("empty body"));
    }
    serde_json::from_slice(body).map_err(|e| bad_request(e.to_string()))
}

/// A validation error, written like the Go validator.
fn validation_error(namespace: &str, field: &str, tag: &str) -> String {
    format!("Key: '{namespace}.{field}' Error:Field validation for '{field}' failed on the '{tag}' tag")
}

fn with_sql_header(mut response: Response, sql: &str) -> Response {
    if let Ok(v) = HeaderValue::from_str(sql) {
        response.headers_mut().insert("X-SQL-Query", v);
    }
    response
}

/// Index of a column of a query result.
fn column_index(result: &QueryResult, name: &str) -> Option<usize> {
    result.blocks.first()?.columns.iter().position(|c| c.name == name)
}

fn value_time(v: &Value) -> DateTime<Utc> {
    match v {
        Value::DateTime(s) => DateTime::from_timestamp(*s as i64, 0).unwrap_or_default(),
        Value::DateTime64(ticks, precision) => {
            let scale = 10i64.pow(*precision);
            DateTime::from_timestamp(
                ticks.div_euclid(scale),
                (ticks.rem_euclid(scale) * (1_000_000_000 / scale)) as u32,
            )
            .unwrap_or_default()
        }
        _ => DateTime::default(),
    }
}

fn value_strings(v: &Value) -> Vec<String> {
    match v {
        Value::Array(items) => items.iter().map(|i| i.as_str().unwrap_or_default().to_string()).collect(),
        _ => vec![],
    }
}

impl Console {
    pub fn new(r: &Reporter, config: Configuration, deps: Dependencies) -> Result<Arc<Console>, String> {
        let mut config = config;
        query::validate_columns(&mut config.default_visualize_options.dimensions, &deps.schema)?;
        config
            .homepage_top_widgets_filter
            .validate(&deps.schema, &deps.database_name)
            .map_err(|e| format!("cannot parse homepage top widgets filter: {e}"))?;
        config
            .homepage_graph_filter
            .validate(&deps.schema, &deps.database_name)
            .map_err(|e| format!("cannot parse homepage graph filter: {e}"))?;
        let clickhouse_queries = r.metrics("console").counter_vec(
            "clickhouse_queries_total",
            "Number of requests to ClickHouse.",
            &["table"],
        );
        Ok(Arc::new(Console {
            config,
            deps,
            flows_tables: RwLock::new(vec![FlowsTable {
                name: "flows".into(),
                resolution: 0,
                oldest: DateTime::<Utc>::MIN_UTC,
            }]),
            clickhouse_queries,
            docs: crate::docs::Docs::default(),
        }))
    }

    pub fn config(&self) -> &Configuration {
        &self.config
    }

    pub fn docs(&self) -> &crate::docs::Docs {
        &self.docs
    }

    fn tables(&self) -> Vec<FlowsTable> {
        self.flows_tables.read().unwrap().clone()
    }

    async fn query(&self, sql: String) -> Result<QueryResult, String> {
        self.deps.clickhouse.query(sql).await
    }

    /// Refresh the list of flows tables, with their resolution and their
    /// oldest data.
    pub async fn refresh_flows_tables(&self) -> Result<(), String> {
        let tables = self
            .query(
                "SELECT name FROM system.tables WHERE database=currentDatabase() \
                 AND (table = 'flows' OR match(table, '^flows_[0-9]')) AND table NOT LIKE '%_local' \
                 AND (engine LIKE '%MergeTree' OR engine = 'Distributed')"
                    .into(),
            )
            .await
            .map_err(|e| format!("cannot query flows table metadata: {e}"))?;
        let mut flows_tables = vec![];
        for row in tables.iter_rows() {
            let name = row[0].as_str().unwrap_or_default().to_string();
            let resolution = match name.strip_prefix("flows_") {
                None => 0,
                Some(d) => match flowscope_common::config::duration::parse(d) {
                    Ok(d) => d.as_nanos() as i64,
                    Err(e) => {
                        tracing::error!(error = %e, "cannot parse duration for table {name}");
                        continue;
                    }
                },
            };
            let oldest = self
                .query(format!("SELECT MIN(TimeReceived) AS t FROM {name}"))
                .await
                .map_err(|e| format!("cannot query table {name} for oldest timestamp: {e}"))?;
            let oldest = oldest.iter_rows().next().map(|r| value_time(&r[0])).unwrap_or_default();
            flows_tables.push(FlowsTable { name, resolution, oldest });
        }
        if flows_tables.is_empty() {
            return Err("no flows table present (yet?)".into());
        }
        *self.flows_tables.write().unwrap() = flows_tables;
        Ok(())
    }

    /// Refresh the flows tables every 10 seconds until it works, then every
    /// 10 minutes.
    pub fn spawn_refresh(self: &Arc<Self>, token: tokio_util::sync::CancellationToken) {
        let this = self.clone();
        tokio::spawn(async move {
            let mut delay = Duration::from_secs(10);
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = token.cancelled() => return,
                }
                match this.refresh_flows_tables().await {
                    Ok(()) => delay = Duration::from_secs(600),
                    Err(e) => tracing::error!(error = %e, "cannot refresh flows tables"),
                }
            }
        });
    }

    /// The API routes, with the user authentication and the caches.
    pub fn router(self: &Arc<Self>, http: &HttpServer) -> Router {
        let ttl = *self.config.cache_ttl;
        let api = Router::new()
            .route("/configuration", get(configuration))
            .route("/docs/{name}", get(crate::docs::docs_handler))
            .route("/widget/flow-last", get(widget_flow_last).layer(http.cache_by_path(Duration::from_secs(5))))
            .route("/widget/flow-rate", get(widget_flow_rate).layer(http.cache_by_path(Duration::from_secs(5))))
            .route("/widget/exporters", get(widget_exporters).layer(http.cache_by_path(Duration::from_secs(30))))
            .route("/widget/top/{name}", get(widget_top).layer(http.cache_by_path(Duration::from_secs(30))))
            .route("/widget/graph", get(widget_graph).layer(http.cache_by_path(Duration::from_secs(300))))
            .route("/graph/line", post(graph_line).layer(http.cache_by_body(ttl)))
            .route("/graph/sankey", post(graph_sankey).layer(http.cache_by_body(ttl)))
            .route("/graph/table-interval", post(table_interval))
            .route("/graph/reverse", post(graph_reverse))
            .route("/filter/validate", post(filter_validate))
            .route("/filter/complete", post(filter_complete).layer(http.cache_by_body(Duration::from_secs(60))))
            .route("/filter/saved", get(saved_list).post(saved_add))
            .route("/filter/saved/{id}", delete(saved_delete))
            .route("/user/info", get(user_info))
            .route("/user/avatar", get(user_avatar))
            .with_state(self.clone());
        let auth = self.deps.auth.clone();
        Router::new()
            .nest("/api/v0/console", api)
            .layer(middleware::from_fn(move |req, next| auth.clone().middleware(req, next)))
    }
}

async fn configuration(State(c): Shared) -> Response {
    let mut dimensions = vec![];
    let mut truncatable = vec![];
    for column in c.deps.schema.columns() {
        if column.console_not_dimension || column.disabled {
            continue;
        }
        dimensions.push(column.name.clone());
        if column.console_truncate_ip {
            truncatable.push(column.name.clone());
        }
    }
    json_response(
        StatusCode::OK,
        &json!({
            "branding": c.config.branding,
            "defaultVisualizeOptions": c.config.default_visualize_options,
            "dimensions": dimensions,
            "dimensionsLimit": c.config.dimensions_limit,
            "homepageTopWidgets": c.config.homepage_top_widgets,
            "truncatable": truncatable,
            "version": FLOWSCOPE_VERSION,
        }),
    )
}

/// A value of the last flow, as JSON, like Go scans it.
fn flow_value(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::UInt(n) => n.to_string(),
        Value::Int(n) => n.to_string(),
        Value::UInt128(n) => n.to_string(),
        Value::Int128(n) => n.to_string(),
        Value::Float(f) => serde_json::to_string(f).unwrap_or_else(|_| "null".into()),
        Value::String(s) => serde_json::to_string(s).unwrap(),
        Value::Bytes(b) => serde_json::to_string(&String::from_utf8_lossy(b)).unwrap(),
        Value::IPv4(ip) => format!("\"{ip}\""),
        Value::IPv6(ip) => match ip.to_ipv4_mapped() {
            Some(v4) => format!("\"{v4}\""),
            None => format!("\"{ip}\""),
        },
        Value::DateTime(_) | Value::DateTime64(..) => format!("\"{}\"", go_time(&value_time(v))),
        Value::Date(days) => {
            let d = DateTime::from_timestamp(*days as i64 * 86400, 0).unwrap_or_default();
            format!("\"{}\"", go_time(&d))
        }
        Value::Array(items) | Value::Tuple(items) => {
            format!("[{}]", items.iter().map(flow_value).collect::<Vec<_>>().join(","))
        }
        Value::Map(entries) => format!(
            "{{{}}}",
            entries.iter().map(|(k, v)| format!("{}:{}", flow_value(k), flow_value(v))).collect::<Vec<_>>().join(",")
        ),
    }
}

async fn widget_flow_last(State(c): Shared) -> Response {
    let sql = widgets::flow_last_sql(&c.deps.schema).to_string();
    let result = match c.query(sql.clone()).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "unable to query database");
            return with_sql_header(json_error(StatusCode::INTERNAL_SERVER_ERROR, "Unable to query database."), &sql);
        }
    };
    let Some(row) = result.iter_rows().next() else {
        return with_sql_header(json_error(StatusCode::NOT_FOUND, "No flow currently in database."), &sql);
    };
    // Keys sorted, values written like Go encodes what the driver returns.
    let mut fields: Vec<(String, String)> = result.column_names().into_iter().zip(row.iter().map(flow_value)).collect();
    fields.sort();
    let raw = format!(
        "{{{}}}",
        fields.iter().map(|(k, v)| format!("{}:{v}", serde_json::to_string(k).unwrap())).collect::<Vec<_>>().join(",")
    );
    let value: Json = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, "Unable to parse flow."),
    };
    with_sql_header(indented_json(StatusCode::OK, &value), &sql)
}

async fn widget_flow_rate(State(c): Shared) -> Response {
    let sql = widgets::FLOW_RATE_SQL;
    let rate = match c.query(sql.into()).await {
        Ok(r) => r.iter_rows().next().and_then(|row| row[0].as_f64()),
        Err(e) => {
            tracing::error!(error = %e, "unable to parse result");
            None
        }
    };
    let response = match rate {
        Some(rate) => indented_json(StatusCode::OK, &json!({"period": "second", "rate": go_float(rate)})),
        None => json_error(StatusCode::INTERNAL_SERVER_ERROR, "Unable to parse result."),
    };
    with_sql_header(response, sql)
}

async fn widget_exporters(State(c): Shared) -> Response {
    let sql = widgets::EXPORTERS_SQL;
    let response = match c.query(sql.into()).await {
        Ok(r) => {
            let exporters: Vec<String> =
                r.iter_rows().map(|row| row[0].as_str().unwrap_or_default().to_string()).collect();
            indented_json(StatusCode::OK, &json!({ "exporters": exporters }))
        }
        Err(e) => {
            tracing::error!(error = %e, "unable to query database");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "Unable to query database.")
        }
    };
    with_sql_header(response, sql)
}

async fn widget_top(State(c): Shared, Path(name): Path<String>) -> Response {
    let widget = match TopWidget::parse(&name) {
        Ok(w) => w,
        Err(e) => return bad_request(e),
    };
    let (q, table) = widgets::top_sql(
        widget,
        &c.config.homepage_top_widgets_filter,
        &c.tables(),
        &(c.deps.clock)(),
        &c.deps.database_name,
    );
    let sql = q.to_string();
    c.clickhouse_queries.with_label_values(&[&table]).inc();
    let response = match c.query(sql.clone()).await {
        Ok(r) => {
            let (name, percent) = (column_index(&r, "Name"), column_index(&r, "Percent"));
            let top: Vec<Json> = r
                .iter_rows()
                .map(|row| {
                    json!({
                        "name": name.and_then(|i| row[i].as_str()).unwrap_or_default(),
                        "percent": go_float(percent.and_then(|i| row[i].as_f64()).unwrap_or_default()),
                    })
                })
                .collect();
            json_response(StatusCode::OK, &json!({ "top": top }))
        }
        Err(e) => {
            tracing::error!(error = %e, "unable to query database");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "Unable to query database.")
        }
    };
    with_sql_header(response, &sql)
}

async fn widget_graph(State(c): Shared) -> Response {
    let range = c.config.homepage_graph_time_range.as_nanos() as i64;
    let (q, table) = widgets::graph_sql(&c.config.homepage_graph_filter, range, &c.tables(), &(c.deps.clock)());
    let sql = q.to_string();
    c.clickhouse_queries.with_label_values(&[&table]).inc();
    let response = match c.query(sql.clone()).await {
        Ok(r) => {
            let (t, gbps) = (column_index(&r, "Time"), column_index(&r, "Gbps"));
            let data: Vec<Json> = r
                .iter_rows()
                .map(|row| {
                    json!({
                        "t": t.map(|i| go_time(&value_time(&row[i]))).unwrap_or_default(),
                        "gbps": go_float(gbps.and_then(|i| row[i].as_f64()).unwrap_or_default()),
                    })
                })
                .collect();
            json_response(StatusCode::OK, &json!({ "data": data }))
        }
        Err(e) => {
            tracing::error!(error = %e, "unable to query database");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "Unable to query database.")
        }
    };
    with_sql_header(response, &sql)
}

/// Validate the parts common to the line and sankey inputs.
fn validate_common(c: &Console, common: &mut crate::graph::CommonInput, namespace: &str) -> Result<(), Response> {
    let ns = format!("{namespace}.graphCommonHandlerInput");
    let mut errors = vec![];
    let zero = DateTime::parse_from_rfc3339("0001-01-01T00:00:00Z").unwrap();
    if common.start == zero {
        errors.push(validation_error(&ns, "Start", "required"));
    }
    if common.end == zero {
        errors.push(validation_error(&ns, "End", "required"));
    } else if common.end <= common.start {
        errors.push(validation_error(&ns, "End", "gtfield"));
    }
    if common.limit < 1 {
        errors.push(validation_error(&ns, "Limit", "min"));
    }
    if !common.limit_type.is_empty() && !["avg", "max", "last"].contains(&common.limit_type.as_str()) {
        errors.push(validation_error(&ns, "LimitType", "oneof"));
    }
    if common.truncate_v4 < 0 {
        errors.push(validation_error(&ns, "TruncateAddrV4", "min"));
    } else if common.truncate_v4 > 32 {
        errors.push(validation_error(&ns, "TruncateAddrV4", "max"));
    }
    if common.truncate_v6 < 0 {
        errors.push(validation_error(&ns, "TruncateAddrV6", "min"));
    } else if common.truncate_v6 > 128 {
        errors.push(validation_error(&ns, "TruncateAddrV6", "max"));
    }
    if common.units.is_empty() {
        errors.push(validation_error(&ns, "Units", "required"));
    } else if !ch::UNITS.contains(&common.units.as_str()) {
        errors.push(validation_error(&ns, "Units", "oneof"));
    }
    if !errors.is_empty() {
        return Err(bad_request(errors.join("\n")));
    }
    common.schema = c.deps.schema.clone();
    common.database = c.deps.database_name.clone();
    query::validate_columns(&mut common.dimensions, &c.deps.schema).map_err(bad_request)?;
    common.filter.validate(&c.deps.schema, &c.deps.database_name).map_err(bad_request)?;
    if common.limit > c.config.dimensions_limit {
        return Err(json_error(
            StatusCode::BAD_REQUEST,
            format!("Limit is set beyond maximum value ({})", c.config.dimensions_limit),
        ));
    }
    Ok(())
}

/// Bind a JSON input with a zero time when missing, so the validation
/// reports it.
fn with_zero_times(body: &Bytes) -> Result<Json, Response> {
    let mut value: Json = bind_json(body)?;
    if let Json::Object(map) = &mut value {
        for key in ["start", "end"] {
            map.entry(key).or_insert_with(|| json!("0001-01-01T00:00:00Z"));
        }
    }
    Ok(value)
}

async fn graph_line(State(c): Shared, body: Bytes) -> Response {
    let mut input: LineInput =
        match with_zero_times(&body).and_then(|v| serde_json::from_value(v).map_err(|e| bad_request(e.to_string()))) {
            Ok(i) => i,
            Err(r) => return r,
        };
    let mut errors = vec![];
    if let Err(r) = validate_common(&c, &mut input.common, "graphLineHandlerInput") {
        return r;
    }
    if input.points == 0 {
        errors.push(validation_error("graphLineHandlerInput", "Points", "required"));
    } else if input.points < 5 {
        errors.push(validation_error("graphLineHandlerInput", "Points", "min"));
    } else if input.points > 2000 {
        errors.push(validation_error("graphLineHandlerInput", "Points", "max"));
    }
    if !errors.is_empty() {
        return bad_request(errors.join("\n"));
    }

    let res = ch::resolve(&c.tables(), &input.resolve_context());
    let sql = union_all(input.to_sql(&res)).to_string();
    c.clickhouse_queries.with_label_values(&[&res.table]).inc();
    let result = match c.query(sql.clone()).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, query = %sql, "unable to query database");
            return with_sql_header(json_error(StatusCode::INTERNAL_SERVER_ERROR, "Unable to query database."), &sql);
        }
    };
    let idx = |n| column_index(&result, n);
    let (axis, time, xps, dims) = (idx("axis"), idx("time"), idx("xps"), idx("dimensions"));
    let rows: Vec<LineRow> = result
        .iter_rows()
        .map(|row| LineRow {
            axis: axis.and_then(|i| row[i].as_u64()).unwrap_or_default() as u8,
            time: time.map(|i| value_time(&row[i])).unwrap_or_default(),
            xps: xps.and_then(|i| row[i].as_f64()).unwrap_or_default(),
            dimensions: dims.map(|i| value_strings(&row[i])).unwrap_or_default(),
        })
        .collect();
    with_sql_header(json_response(StatusCode::OK, &line_output(&input, rows)), &sql)
}

async fn graph_sankey(State(c): Shared, body: Bytes) -> Response {
    let mut input: SankeyInput =
        match with_zero_times(&body).and_then(|v| serde_json::from_value(v).map_err(|e| bad_request(e.to_string()))) {
            Ok(i) => i,
            Err(r) => return r,
        };
    if let Err(r) = validate_common(&c, &mut input.common, "graphSankeyHandlerInput") {
        return r;
    }
    if input.common.dimensions.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "At least one dimension is required.");
    }
    let res = ch::resolve(&c.tables(), &input.resolve_context());
    let sql = union_all(input.to_sql(&res)).to_string();
    c.clickhouse_queries.with_label_values(&[&res.table]).inc();
    let result = match c.query(sql.clone()).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, query = %sql, "unable to query database");
            return with_sql_header(json_error(StatusCode::INTERNAL_SERVER_ERROR, "Unable to query database."), &sql);
        }
    };
    let idx = |n| column_index(&result, n);
    let (axis, xps, dims) = (idx("axis"), idx("xps"), idx("dimensions"));
    let rows: Vec<SankeyRow> = result
        .iter_rows()
        .map(|row| SankeyRow {
            axis: axis.and_then(|i| row[i].as_u64()).unwrap_or_default() as u8,
            xps: xps.and_then(|i| row[i].as_f64()).unwrap_or_default(),
            dimensions: dims.map(|i| value_strings(&row[i])).unwrap_or_default(),
        })
        .collect();
    with_sql_header(json_response(StatusCode::OK, &sankey_output(&input, rows)), &sql)
}

#[derive(Deserialize)]
struct TableIntervalInput {
    start: Time,
    end: Time,
    #[serde(default)]
    points: u64,
}

async fn table_interval(State(c): Shared, body: Bytes) -> Response {
    let input: TableIntervalInput =
        match with_zero_times(&body).and_then(|v| serde_json::from_value(v).map_err(|e| bad_request(e.to_string()))) {
            Ok(i) => i,
            Err(r) => return r,
        };
    let ns = "tableIntervalInput";
    let zero = DateTime::parse_from_rfc3339("0001-01-01T00:00:00Z").unwrap();
    let mut errors = vec![];
    if input.start == zero {
        errors.push(validation_error(ns, "Start", "required"));
    }
    if input.end == zero {
        errors.push(validation_error(ns, "End", "required"));
    } else if input.end <= input.start {
        errors.push(validation_error(ns, "End", "gtfield"));
    }
    if input.points == 0 {
        errors.push(validation_error(ns, "Points", "required"));
    } else if input.points < 5 {
        errors.push(validation_error(ns, "Points", "min"));
    } else if input.points > 2000 {
        errors.push(validation_error(ns, "Points", "max"));
    }
    if !errors.is_empty() {
        return bad_request(errors.join("\n"));
    }
    let (table, interval, _) = ch::compute_table_and_interval(
        &c.tables(),
        &InputContext { start: input.start, end: input.end, main_table_required: false, points: input.points },
    );
    json_response(StatusCode::OK, &json!({"table": table, "interval": interval / SECOND}))
}

#[derive(Deserialize)]
struct ReverseInput {
    #[serde(default)]
    dimensions: Option<Vec<query::Column>>,
    #[serde(default)]
    filter: String,
}

async fn graph_reverse(State(c): Shared, body: Bytes) -> Response {
    let input: ReverseInput = match bind_json(&body) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let mut dimensions = input.dimensions.unwrap_or_default();
    if let Err(e) = query::validate_columns(&mut dimensions, &c.deps.schema) {
        return bad_request(e);
    }
    let reversed = match filter::reverse(&input.filter, &c.deps.schema) {
        Ok(r) => r,
        Err(e) => return json_error(StatusCode::BAD_REQUEST, e.human()),
    };
    query::reverse_columns(&mut dimensions, &c.deps.schema);
    json_response(StatusCode::OK, &json!({"dimensions": dimensions, "filter": reversed}))
}

#[derive(Deserialize)]
struct ValidateInput {
    #[serde(default)]
    filter: String,
}

#[derive(Serialize)]
struct ValidateOutput {
    message: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    parsed: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    errors: Vec<filter::OneError>,
}

async fn filter_validate(State(c): Shared, body: Bytes) -> Response {
    let input: ValidateInput = match bind_json(&body) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let ok = |parsed: String| ValidateOutput { message: "ok".into(), parsed, errors: vec![] };
    if input.filter.trim().is_empty() {
        return json_response(StatusCode::OK, &ok(String::new()));
    }
    let meta = filter::Meta {
        schema: c.deps.schema.clone(),
        database: c.deps.database_name.clone(),
        reverse_direction: false,
    };
    let output = match filter::parse(&input.filter, &meta) {
        Ok(parsed) => ok(parsed.expr.map(|e| e.to_string()).unwrap_or_default()),
        Err(e) => ValidateOutput { message: e.human(), parsed: String::new(), errors: e.all() },
    };
    json_response(StatusCode::OK, &output)
}

#[derive(Deserialize)]
struct CompleteInput {
    #[serde(default)]
    what: String,
    #[serde(default)]
    column: String,
    #[serde(default)]
    operator: String,
    #[serde(default)]
    prefix: String,
    #[serde(default = "default_limit")]
    limit: i64,
}

fn default_limit() -> i64 {
    20
}

#[derive(Clone, Debug, Serialize)]
struct Completion {
    label: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    detail: String,
    quoted: bool,
}

fn completion(label: &str, detail: &str, quoted: bool) -> Completion {
    Completion { label: label.into(), detail: detail.into(), quoted }
}

impl Console {
    /// The columns usable on the right side of a column and an operator.
    fn comparable_columns(&self, name: &str, operator: &str, prefix: &str) -> Vec<Completion> {
        let schema = &self.deps.schema;
        let parser_type =
            schema.columns().find(|c| c.name.eq_ignore_ascii_case(name)).map(|c| c.parser_type).unwrap_or_default();
        let operators: &[&str] = match parser_type {
            "uint" => &["=", "!=", "<", "<=", ">", ">="],
            "asn" | "string" => &["=", "!="],
            _ => return vec![],
        };
        if !operators.contains(&operator) {
            return vec![];
        }
        let mut names: Vec<&str> = schema
            .columns()
            .filter(|c| c.parser_type == parser_type && !c.name.eq_ignore_ascii_case(name))
            .filter(|c| c.name.to_lowercase().starts_with(&prefix.to_lowercase()))
            .map(|c| c.name.as_str())
            .collect();
        names.sort();
        names.into_iter().map(|n| completion(n, "column name", false)).collect()
    }
}

async fn filter_complete(State(c): Shared, body: Bytes) -> Response {
    let mut input: CompleteInput = match bind_json(&body) {
        Ok(i) => i,
        Err(r) => return r,
    };
    if input.what.is_empty() {
        return bad_request(validation_error("filterCompleteHandlerInput", "What", "required"));
    }
    if !["column", "operator", "value"].contains(&input.what.as_str()) {
        return bad_request(validation_error("filterCompleteHandlerInput", "What", "oneof"));
    }
    if input.what != "column" && input.column.is_empty() {
        return bad_request(validation_error("filterCompleteHandlerInput", "Column", "required_unless"));
    }
    let schema = &c.deps.schema;
    let mut completions = vec![];
    match input.what.as_str() {
        "column" => {
            let mut columns: Vec<&str> = schema
                .columns()
                .filter(|col| !col.parser_type.is_empty())
                .filter(|col| col.name.to_lowercase().starts_with(&input.prefix.to_lowercase()))
                .map(|col| col.name.as_str())
                .collect();
            columns.sort();
            completions.extend(columns.into_iter().map(|col| completion(col, "column name", false)));
        }
        "operator" => {
            let meta = filter::Meta {
                schema: schema.clone(),
                database: c.deps.database_name.clone(),
                reverse_direction: false,
            };
            let errors = filter::complete_after_column(&input.column, &meta);
            for candidate in errors.expected() {
                if !candidate.starts_with('"') {
                    continue;
                }
                let inner = &candidate[1..candidate.len() - 1];
                let candidate = inner.strip_suffix("\"i").unwrap_or(inner);
                let candidate = candidate.strip_suffix('"').unwrap_or(candidate);
                if candidate != "--" && candidate != "/*" {
                    let label = if candidate == "IN" || candidate == "NOTIN" {
                        format!("{candidate} (")
                    } else {
                        candidate.to_string()
                    };
                    completions.push(completion(&label, "comparison operator", false));
                }
            }
        }
        _ => {
            let lower = input.column.to_lowercase();
            let other_columns = c.comparable_columns(&input.column, &input.operator, &input.prefix);
            let fixed = |s: &[(&str, &str)], detail: &str| -> Vec<Completion> {
                s.iter().map(|(l, _)| completion(l, detail, false)).collect()
            };
            match lower.as_str() {
                "inifboundary" | "outifboundary" => completions
                    .extend(fixed(&[("internal", ""), ("external", ""), ("undefined", "")], "network boundary")),
                "flowdirection" => {
                    completions.extend(fixed(&[("ingress", ""), ("egress", ""), ("undefined", "")], "flow direction"))
                }
                "etype" => completions.extend(fixed(&[("IPv4", ""), ("IPv6", "")], "ethernet type")),
                "proto" => {
                    for p in [
                        "TCP",
                        "UDP",
                        "SCTP",
                        "ICMP",
                        "IPv6-ICMP",
                        "GRE",
                        "ESP",
                        "AH",
                        "IPIP",
                        "VRRP",
                        "L2TP",
                        "IGMP",
                        "PIM",
                        "IPv4",
                        "IPv6",
                    ] {
                        completions.push(completion(p, "protocol", true));
                    }
                }
                _ => {}
            }
            let mut skip_rest = false;
            if let Some((q, rows)) =
                completion::value_query(schema, &input.column, &input.prefix, input.limit, &c.deps.database_name)
            {
                let is_exporter = matches!(
                    lower.as_str(),
                    "exportername"
                        | "exportergroup"
                        | "exporterrole"
                        | "exportersite"
                        | "exporterregion"
                        | "exportertenant"
                        | "inifname"
                        | "outifname"
                        | "inifdescription"
                        | "outifdescription"
                        | "inifconnectivity"
                        | "outifconnectivity"
                        | "inifprovider"
                        | "outifprovider"
                );
                match c.query(q.to_string()).await {
                    Ok(result) => {
                        let (label, detail) = (column_index(&result, "label"), column_index(&result, "detail"));
                        for row in result.iter_rows() {
                            let get = |i: Option<usize>| {
                                i.and_then(|i| row[i].as_str().map(str::to_string)).unwrap_or_default()
                            };
                            completions.push(match &rows {
                                completion::Rows::Label { detail, quoted } => {
                                    Completion { label: get(label), detail: detail.clone(), quoted: *quoted }
                                }
                                completion::Rows::LabelDetail => {
                                    Completion { label: get(label), detail: get(detail), quoted: false }
                                }
                            });
                        }
                        // The prefix was handled by the query.
                        input.prefix.clear();
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "unable to query database");
                        // In Go, an error on the exporters table leaves the
                        // switch: no custom or other columns.
                        skip_rest = is_exporter;
                    }
                }
            }
            if !skip_rest {
                // Custom dictionary columns.
                for col in schema.columns() {
                    if col.key < ColumnKey::Last || lower != col.name.to_lowercase() || col.parser_type != "string" {
                        continue;
                    }
                    let q = completion::custom_attribute(&col.name, &input.prefix, input.limit);
                    match c.query(q.to_string()).await {
                        Ok(result) => {
                            let attribute = column_index(&result, "attribute");
                            for row in result.iter_rows() {
                                let label = attribute.and_then(|i| row[i].as_str()).unwrap_or_default();
                                completions.push(completion(label, "", true));
                            }
                            input.prefix.clear();
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "unable to query database");
                            break;
                        }
                    }
                }
                completions.extend(other_columns);
            }
        }
    }
    let prefix = input.prefix.to_lowercase();
    let completions: Vec<Completion> =
        completions.into_iter().filter(|c| c.label.to_lowercase().starts_with(&prefix)).collect();
    json_response(StatusCode::OK, &json!({ "completions": completions }))
}

async fn saved_list(State(c): Shared, Extension(user): Extension<UserInformation>) -> Response {
    match c.deps.database.list_saved_filters(&user.login).await {
        Ok(filters) => json_response(StatusCode::OK, &json!({ "filters": filters })),
        Err(e) => {
            tracing::error!(error = %e, "unable to list filters");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "unable to list filters")
        }
    }
}

async fn saved_delete(
    State(c): Shared,
    Extension(user): Extension<UserInformation>,
    Path(id): Path<String>,
) -> Response {
    let Ok(id) = id.parse::<u64>() else {
        return json_error(StatusCode::BAD_REQUEST, "bad ID format");
    };
    match c.deps.database.delete_saved_filter(id, &user.login).await {
        Ok(()) => no_content(),
        // Assume this is because it is not found.
        Err(_) => json_error(StatusCode::NOT_FOUND, "filter not found"),
    }
}

async fn saved_add(State(c): Shared, Extension(user): Extension<UserInformation>, body: Bytes) -> Response {
    let mut filter: SavedFilter = match bind_json(&body) {
        Ok(f) => f,
        Err(r) => return r,
    };
    let mut errors = vec![];
    if filter.description.is_empty() {
        errors.push(validation_error("SavedFilter", "Description", "required"));
    }
    if filter.content.is_empty() {
        errors.push(validation_error("SavedFilter", "Content", "required"));
    }
    if !errors.is_empty() {
        return bad_request(errors.join("\n"));
    }
    filter.user = user.login;
    match c.deps.database.create_saved_filter(&filter).await {
        Ok(()) => no_content(),
        Err(e) => {
            tracing::error!(error = %e, "cannot create saved filter");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "cannot create new filter")
        }
    }
}

async fn user_info(Extension(user): Extension<UserInformation>) -> Response {
    json_response(StatusCode::OK, &user)
}

async fn user_avatar(Extension(user): Extension<UserInformation>, headers: HeaderMap) -> Response {
    auth::avatar_response(&user, headers.get(header::IF_NONE_MATCH))
}
