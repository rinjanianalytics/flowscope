// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Static assets of the web interface and of the documentation.
//!
//! The web interface is a static export built with a placeholder as base
//! path. The placeholder is replaced by the configured URL prefix when
//! loading the files, so the console can be hosted under any prefix.

use crate::component::Console;
use crate::docs::{frontend_files, serve_markdown_document};
use axum::Router;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use std::collections::HashMap;
use std::sync::Arc;

/// Base path the web interface is built with.
const BASE_PLACEHOLDER: &str = "/__flowscope_base__";

/// The content type of a file, from its extension.
fn content_type(path: &str) -> &'static str {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "txt" => "text/plain; charset=utf-8",
        "md" => "text/markdown; charset=utf-8",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// Tell if the placeholder may appear in a file.
fn is_text(path: &str) -> bool {
    matches!(path.rsplit_once('.').map(|(_, e)| e), Some("html" | "js" | "css" | "txt" | "json" | "map" | "svg"))
}

/// The files of the web interface, ready to be served for a URL prefix.
pub struct Frontend {
    files: HashMap<String, Vec<u8>>,
}

impl Frontend {
    /// Prepare the files for a URL prefix ("/" or "/flowscope/").
    pub fn new(files: &[(&str, &[u8])], url_prefix: &str) -> Frontend {
        let base = url_prefix.trim_end_matches('/');
        let files = files
            .iter()
            .map(|(name, data)| {
                let data = match std::str::from_utf8(data) {
                    Ok(text) if is_text(name) && text.contains(BASE_PLACEHOLDER) => {
                        text.replace(BASE_PLACEHOLDER, base).into_bytes()
                    }
                    _ => data.to_vec(),
                };
                (name.to_string(), data)
            })
            .collect();
        Frontend { files }
    }

    fn get(&self, name: &str) -> Option<(&str, &[u8])> {
        self.files.get_key_value(name).map(|(k, v)| (k.as_str(), v.as_slice()))
    }

    /// The file to serve for a path, and whether it is the "not found"
    /// page. A path is a file of the export, or a page of the application.
    /// The pages with a parameter in their path are served by their parent.
    fn resolve(&self, path: &str) -> Option<(&str, &[u8], bool)> {
        let path = path.trim_start_matches('/').trim_end_matches('/');
        if path.split('/').any(|segment| segment == "..") {
            return None;
        }
        if !path.is_empty()
            && let Some((name, data)) = self.get(path)
        {
            return Some((name, data, false));
        }
        let page = match path {
            "" => "index.html".to_string(),
            p if p == "visualize" || p.starts_with("visualize/") => "visualize.html".into(),
            p if p == "docs" || p.starts_with("docs/") => "docs.html".into(),
            p => format!("{p}.html"),
        };
        if let Some((name, data)) = self.get(&page) {
            return Some((name, data, false));
        }
        self.get("404.html").map(|(name, data)| (name, data, true))
    }
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, [(header::CONTENT_TYPE, "text/plain; charset=utf-8")], "404 page not found\n")
        .into_response()
}

fn file_response(status: StatusCode, path: &str, data: &[u8]) -> Response {
    let mut response = (status, [(header::CONTENT_TYPE, content_type(path))], data.to_vec()).into_response();
    // The names of these files change with their content.
    if path.starts_with("_next/static/") {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=31536000, immutable"));
    }
    response
}

/// Serve the web interface. A documentation page is served as Markdown when
/// the client does not prefer HTML.
async fn default_handler(State((c, frontend)): State<(Arc<Console>, Arc<Frontend>)>, req: Request) -> Response {
    let mut vary = false;
    match serve_markdown_document(&c, req.uri().path(), req.headers()) {
        Ok(response) => return response,
        Err(v) => vary = vary || v,
    }
    let mut response = match frontend.resolve(req.uri().path()) {
        Some((name, data, missing)) => {
            file_response(if missing { StatusCode::NOT_FOUND } else { StatusCode::OK }, name, data)
        }
        None => not_found(),
    };
    if vary {
        response.headers_mut().insert(header::VARY, HeaderValue::from_static("Accept"));
    }
    response
}

async fn doc_asset(State(c): State<Arc<Console>>, Path(path): Path<String>) -> Response {
    match c.docs().asset(&path) {
        Some(data) => file_response(StatusCode::OK, &path, data),
        None => not_found(),
    }
}

/// The routes for the documentation assets ("/assets/docs/") and the web
/// interface ("/").
pub fn routers(console: &Arc<Console>) -> (Router, Router) {
    let assets = Router::new().route("/assets/docs/{*path}", get(doc_asset)).with_state(console.clone());
    let frontend = Arc::new(Frontend::new(frontend_files(), &console.config().url_prefix()));
    let fallback = Router::new().fallback(default_handler).with_state((console.clone(), frontend));
    (assets, fallback)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frontend(prefix: &str) -> Frontend {
        Frontend::new(
            &[
                ("index.html", b"<script src=\"/__flowscope_base__/_next/static/a.js\"></script>".as_slice()),
                ("visualize.html", b"visualize"),
                ("docs.html", b"docs"),
                ("login.html", b"login"),
                ("404.html", b"missing"),
                ("_next/static/a.js", b"fetch(\"/__flowscope_base__/api\")"),
                ("_next/static/font.woff2", b"/__flowscope_base__"),
                ("icon.svg", b"<svg/>"),
            ],
            prefix,
        )
    }

    #[test]
    fn prefix() {
        let f = frontend("/");
        assert_eq!(f.get("index.html").unwrap().1, b"<script src=\"/_next/static/a.js\"></script>");
        assert_eq!(f.get("_next/static/a.js").unwrap().1, b"fetch(\"/api\")");
        let f = frontend("/flowscope/");
        assert_eq!(f.get("index.html").unwrap().1, b"<script src=\"/flowscope/_next/static/a.js\"></script>");
        // Binary files are left alone.
        assert_eq!(f.get("_next/static/font.woff2").unwrap().1, b"/__flowscope_base__");
    }

    #[test]
    fn resolve() {
        let f = frontend("/");
        let name = |path: &str| f.resolve(path).map(|(n, _, missing)| (n.to_string(), missing)).unwrap();
        assert_eq!(name("/"), ("index.html".into(), false));
        assert_eq!(name("/visualize"), ("visualize.html".into(), false));
        assert_eq!(name("/visualize/v1-abc"), ("visualize.html".into(), false));
        assert_eq!(name("/docs/intro"), ("docs.html".into(), false));
        assert_eq!(name("/login"), ("login.html".into(), false));
        assert_eq!(name("/icon.svg"), ("icon.svg".into(), false));
        assert_eq!(name("/_next/static/a.js"), ("_next/static/a.js".into(), false));
        assert_eq!(name("/nothing"), ("404.html".into(), true));
        assert_eq!(name("/_next/static/missing.js"), ("404.html".into(), true));
        assert!(f.resolve("/../etc/passwd").is_none());
    }
}
