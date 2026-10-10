// SPDX-FileCopyrightText: 2026 FlowScope contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The OpenAPI document of the console API.
//!
//! The document is built from the types of [`crate::models`]. The console
//! serves it at `/api/v0/console/openapi.json` and a copy is kept in
//! `openapi.json`, next to the manifest of this crate, for the tools
//! generating clients for other languages.

// The functions below only carry the description of a route.
#![allow(dead_code)]

use crate::models::*;
use utoipa::OpenApi;

/// A PNG file.
#[derive(utoipa::ToSchema)]
#[schema(value_type = String, format = Binary)]
struct Png(Vec<u8>);

/// Prefix of all routes.
pub const PREFIX: &str = "/api/v0/console";

#[utoipa::path(
    get, path = "/api/v0/console/configuration", tag = "console", operation_id = "getConfiguration",
    summary = "What the web interface needs to start",
    responses((status = 200, body = Configuration)),
)]
fn configuration() {}

#[utoipa::path(
    get, path = "/api/v0/console/user/info", tag = "user", operation_id = "getUserInfo",
    summary = "The current user",
    responses((status = 200, body = UserInfo), (status = 401, description = "Not logged in", body = ErrorMessage)),
)]
fn user_info() {}

#[utoipa::path(
    get, path = "/api/v0/console/user/avatar", tag = "user", operation_id = "getUserAvatar",
    summary = "Picture of the current user",
    responses(
        (status = 200, description = "The picture", content_type = "image/png", body = Png),
        (status = 401, description = "Not logged in", body = ErrorMessage),
    ),
)]
fn user_avatar() {}

#[utoipa::path(
    post, path = "/api/v0/console/graph/line", tag = "graph", operation_id = "graphLine",
    summary = "Traffic over time",
    request_body = LineInput,
    responses((status = 200, body = LineOutput), (status = 400, description = "Invalid input", body = ErrorMessage)),
)]
fn graph_line() {}

#[utoipa::path(
    post, path = "/api/v0/console/graph/sankey", tag = "graph", operation_id = "graphSankey",
    summary = "Traffic between the values of several dimensions",
    request_body = GraphInput,
    responses((status = 200, body = SankeyOutput), (status = 400, description = "Invalid input", body = ErrorMessage)),
)]
fn graph_sankey() {}

#[utoipa::path(
    post, path = "/api/v0/console/graph/table-interval", tag = "graph", operation_id = "graphTableInterval",
    summary = "Table and resolution a graph would use",
    request_body = TableIntervalInput,
    responses(
        (status = 200, body = TableIntervalOutput),
        (status = 400, description = "Invalid input", body = ErrorMessage),
    ),
)]
fn graph_table_interval() {}

#[utoipa::path(
    post, path = "/api/v0/console/graph/reverse", tag = "graph", operation_id = "graphReverse",
    summary = "Swap the direction of dimensions and of a filter",
    request_body = Reverse,
    responses((status = 200, body = Reverse), (status = 400, description = "Invalid input", body = ErrorMessage)),
)]
fn graph_reverse() {}

#[utoipa::path(
    post, path = "/api/v0/console/filter/validate", tag = "filter", operation_id = "filterValidate",
    summary = "Check a filter expression",
    request_body = ValidateInput,
    responses(
        (status = 200, description = "The expression was checked. It may be invalid.", body = ValidateOutput),
        (status = 400, description = "Invalid input", body = ErrorMessage),
    ),
)]
fn filter_validate() {}

#[utoipa::path(
    post, path = "/api/v0/console/filter/complete", tag = "filter", operation_id = "filterComplete",
    summary = "Complete a filter expression",
    request_body = CompleteInput,
    responses((status = 200, body = CompleteOutput), (status = 400, description = "Invalid input", body = ErrorMessage)),
)]
fn filter_complete() {}

#[utoipa::path(
    get, path = "/api/v0/console/filter/saved", tag = "filter", operation_id = "listSavedFilters",
    summary = "Saved filters the current user can see",
    responses((status = 200, body = SavedFilters)),
)]
fn saved_filters() {}

#[utoipa::path(
    post, path = "/api/v0/console/filter/saved", tag = "filter", operation_id = "addSavedFilter",
    summary = "Save a filter",
    request_body = SavedFilterInput,
    responses((status = 204, description = "Saved"), (status = 400, description = "Invalid input", body = ErrorMessage)),
)]
fn add_saved_filter() {}

#[utoipa::path(
    delete, path = "/api/v0/console/filter/saved/{id}", tag = "filter", operation_id = "deleteSavedFilter",
    summary = "Delete a filter of the current user",
    params(("id" = u64, Path, description = "Identifier of the filter")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 400, description = "Invalid identifier", body = ErrorMessage),
        (status = 404, description = "No such filter for this user", body = ErrorMessage),
    ),
)]
fn delete_saved_filter() {}

#[utoipa::path(
    get, path = "/api/v0/console/widget/flow-last", tag = "widget", operation_id = "widgetFlowLast",
    summary = "The most recent flow",
    responses(
        (status = 200, description = "The columns of the flow", body = HashMap<String, serde_json::Value>),
        (status = 404, description = "No flow yet", body = ErrorMessage),
    ),
)]
fn widget_flow_last() {}

#[utoipa::path(
    get, path = "/api/v0/console/widget/flow-rate", tag = "widget", operation_id = "widgetFlowRate",
    summary = "Flows received each second",
    responses((status = 200, body = FlowRate)),
)]
fn widget_flow_rate() {}

#[utoipa::path(
    get, path = "/api/v0/console/widget/exporters", tag = "widget", operation_id = "widgetExporters",
    summary = "Exporters sending flows",
    responses((status = 200, body = Exporters)),
)]
fn widget_exporters() {}

#[utoipa::path(
    get, path = "/api/v0/console/widget/top/{name}", tag = "widget", operation_id = "widgetTop",
    summary = "Top values of a column",
    params((
        "name" = String, Path,
        description = "src-as, dst-as, src-country, dst-country, exporter, protocol, etype, src-port or dst-port",
    )),
    responses((status = 200, body = Top), (status = 400, description = "Unknown widget", body = ErrorMessage)),
)]
fn widget_top() {}

#[utoipa::path(
    get, path = "/api/v0/console/widget/graph", tag = "widget", operation_id = "widgetGraph",
    summary = "Traffic of the last day",
    responses((status = 200, body = Traffic)),
)]
fn widget_graph() {}

#[utoipa::path(
    get, path = "/api/v0/console/docs/{name}", tag = "docs", operation_id = "getDocument",
    summary = "A page of the documentation",
    description = "The page is sent as its Markdown source, unless the `Accept` header prefers JSON.",
    params(("name" = String, Path, description = "Name of the page, like \"intro\"")),
    responses(
        (status = 200, description = "The page", content((Document = "application/json"), (String = "text/markdown"))),
        (status = 404, description = "No such page", body = ErrorMessage),
    ),
)]
fn document() {}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "FlowScope console API",
        description = "API of the FlowScope console. Errors are answered with a JSON object holding a message.",
        license(name = "AGPL-3.0-only", identifier = "AGPL-3.0-only"),
    ),
    paths(
        configuration,
        user_info,
        user_avatar,
        graph_line,
        graph_sankey,
        graph_table_interval,
        graph_reverse,
        filter_validate,
        filter_complete,
        saved_filters,
        add_saved_filter,
        delete_saved_filter,
        widget_flow_last,
        widget_flow_rate,
        widget_exporters,
        widget_top,
        widget_graph,
        document,
    )
)]
struct ApiDoc;

/// The OpenAPI document.
pub fn document_value() -> utoipa::openapi::OpenApi {
    ApiDoc::openapi()
}

/// The OpenAPI document as JSON, indented, with a line break at the end.
pub fn json() -> String {
    let mut text = serde_json::to_string_pretty(&document_value()).expect("the document is valid JSON");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `openapi.json` is the document. Update it with
    /// `FLOWSCOPE_UPDATE_OPENAPI=1 cargo test -p flowscope-sdk`.
    #[test]
    fn file_is_current() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/openapi.json");
        if std::env::var_os("FLOWSCOPE_UPDATE_OPENAPI").is_some() {
            std::fs::write(path, json()).unwrap();
        }
        let file = std::fs::read_to_string(path).unwrap_or_default();
        assert!(file == json(), "openapi.json is out of date, see the comment of this test");
    }

    #[test]
    fn routes() {
        let doc = document_value();
        let count: usize = doc
            .paths
            .paths
            .values()
            .map(|item| {
                [&item.get, &item.post, &item.delete, &item.put, &item.patch].iter().filter(|o| o.is_some()).count()
            })
            .sum();
        assert_eq!(count, 18);
        assert!(doc.paths.paths.keys().all(|p| p.starts_with(PREFIX)));
    }
}
