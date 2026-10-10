// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! The documentation: served as Markdown, or rendered to HTML for the web
//! interface, with a table of contents.

use crate::component::Console;
use crate::highlight;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use flowscope_common::httpserver::json_error;
use pulldown_cmark::{BlockQuoteKind, CodeBlockKind, CowStr, Event, Options, Parser, Tag, TagEnd};
use regex::Regex;
use serde::Serialize;
use std::collections::HashSet;
use std::sync::{Arc, LazyLock, OnceLock};

const TYPE_MARKDOWN: &str = "text/markdown";
const TYPE_JSON: &str = "application/json";
const TYPE_HTML: &str = "text/html";

static INTERNAL_LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(([0-9]+)-([a-z]+).md)(#.*|$)").unwrap());
static MARKDOWN_LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\]\(([0-9]+)-([a-z]+)\.md(#[^)]*)?\)").unwrap());
static DOCUMENT_TITLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^# +(.*\S)").unwrap());

/// Section of a document, from the number prefixing its name.
fn document_section(number: &str) -> &'static str {
    let Ok(n) = number.parse::<i64>() else { return "" };
    for (up_to, section) in [(0, ""), (9, "Tutorials"), (49, "How-to guides"), (79, "Reference"), (98, "Explanation")] {
        if n <= up_to {
            return section;
        }
    }
    ""
}

/// Old document names, to keep the links working.
fn renamed(name: &str) -> &str {
    match name {
        "operations" => "exporters",
        n => n,
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Header {
    pub level: u32,
    pub id: String,
    pub title: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct DocumentToc {
    pub name: String,
    pub section: String,
    pub headers: Vec<Header>,
}

/// The documentation files.
pub struct Docs {
    /// Files, sorted by name.
    files: Vec<(String, Vec<u8>)>,
    toc: OnceLock<Vec<DocumentToc>>,
}

/// Static assets of the web interface, embedded at build time.
pub fn frontend_files() -> &'static [(&'static str, &'static [u8])] {
    crate::embedded::FRONTEND
}

impl Default for Docs {
    fn default() -> Self {
        Docs::new(crate::embedded::DOCS.iter().map(|(n, d)| (n.to_string(), d.to_vec())).collect())
    }
}

/// Generate a heading ID like goldmark.
fn heading_id(value: &str, ids: &mut HashSet<String>) -> String {
    let mut result = String::new();
    for c in value.trim().chars() {
        if c.is_ascii_alphanumeric() {
            result.push(c.to_ascii_lowercase());
        } else if c.is_ascii_whitespace() || c == '-' || c == '_' {
            result.push('-');
        }
    }
    if result.is_empty() {
        result = "heading".into();
    }
    if ids.insert(result.clone()) {
        return result;
    }
    (1..).map(|i| format!("{result}-{i}")).find(|candidate| ids.insert(candidate.clone())).unwrap()
}

/// The raw text of a heading, from its source: the last line, without the
/// ATX markers.
fn heading_text(source: &str) -> String {
    let lines: Vec<&str> = source.trim_end_matches('\n').lines().collect();
    let Some(last) = lines.last() else { return String::new() };
    let line = last.trim();
    // A setext underline: the text is on the line before.
    if lines.len() > 1 && line.chars().all(|c| c == '=' || c == '-') {
        return lines[lines.len() - 2].trim().to_string();
    }
    let line = line.trim_start_matches('#');
    let line = line.trim();
    // Closing sequence.
    let without = line.trim_end_matches('#');
    if without.len() != line.len() && (without.is_empty() || without.ends_with(' ')) {
        return without.trim_end().to_string();
    }
    line.to_string()
}

fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_SMART_PUNCTUATION | Options::ENABLE_GFM
}

/// The headers of a document.
fn document_headers(markdown: &str) -> Vec<Header> {
    let mut ids = HashSet::new();
    let mut headers = vec![];
    for (event, range) in Parser::new_ext(markdown, Options::ENABLE_GFM).into_offset_iter() {
        if let Event::Start(Tag::Heading { level, .. }) = event {
            let title = heading_text(&markdown[range]);
            headers.push(Header { level: level as u32, id: heading_id(&title, &mut ids), title });
        }
    }
    headers
}

/// The header of an admonition.
fn admonition_header(kind: BlockQuoteKind) -> String {
    let (name, icon) = match kind {
        BlockQuoteKind::Important => (
            "Important",
            r#"<path stroke-linecap="round" stroke-linejoin="round" d="M12 9v3.75m9-.75a9 9 0 1 1-18 0 9 9 0 0 1 18 0Zm-9 3.75h.008v.008H12v-.008Z" />"#,
        ),
        BlockQuoteKind::Note => (
            "Note",
            r#"<path stroke-linecap="round" stroke-linejoin="round" d="m11.25 11.25.041-.02a.75.75 0 0 1 1.063.852l-.708 2.836a.75.75 0 0 0 1.063.853l.041-.021M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0Zm-9-3.75h.008v.008H12V8.25Z" />"#,
        ),
        BlockQuoteKind::Tip => (
            "Tip",
            r#"<path stroke-linecap="round" stroke-linejoin="round" d="M12 18v-5.25m0 0a6.01 6.01 0 0 0 1.5-.189m-1.5.189a6.01 6.01 0 0 1-1.5-.189m3.75 7.478a12.06 12.06 0 0 1-4.5 0m3.75 2.383a14.406 14.406 0 0 1-3 0M14.25 18v-.192c0-.983.658-1.823 1.508-2.316a7.5 7.5 0 1 0-7.517 0c.85.493 1.509 1.333 1.509 2.316V18" />"#,
        ),
        BlockQuoteKind::Warning => (
            "Warning",
            r#"<path stroke-linecap="round" stroke-linejoin="round" d="M12 9v3.75m-9.303 3.376c-.866 1.5.217 3.374 1.948 3.374h14.71c1.73 0 2.813-1.874 1.948-3.374L13.949 3.378c-.866-1.5-3.032-1.5-3.898 0L2.697 16.126ZM12 15.75h.007v.008H12v-.008Z" />"#,
        ),
        BlockQuoteKind::Caution => (
            "Caution",
            r#"<path stroke-linecap="round" stroke-linejoin="round" d="M12 9v3.75m0-10.036A11.959 11.959 0 0 1 3.598 6 11.99 11.99 0 0 0 3 9.75c0 5.592 3.824 10.29 9 11.622 5.176-1.332 9-6.03 9-11.622 0-1.31-.21-2.57-.598-3.75h-.152c-3.196 0-6.1-1.25-8.25-3.286Zm0 13.036h.008v.008H12v-.008Z" />"#,
        ),
    };
    format!(
        "\n<div class=\"admonition admonition-{}\" dir=\"auto\">\n  <p class=\"admonition-title\" dir=\"auto\">\n    \
         <svg viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" aria-hidden=\"true\">\n      \
         {icon}\n    </svg>\n{name}\n  </p>",
        name.to_lowercase()
    )
}

/// Render a document to HTML. Links to other documents, to anchors and to
/// images become URLs relative to the <base> tag of the web interface.
pub fn render(markdown: &str, document: &str) -> String {
    let mut ids = HashSet::new();
    let mut events = vec![];
    let mut code: Option<(String, String)> = None;
    // Image being rendered: destination, title and alternative text.
    let mut image: Option<(String, String, String)> = None;
    for (event, range) in Parser::new_ext(markdown, options()).into_offset_iter() {
        if let Some((_, _, alt)) = image.as_mut() {
            match event {
                Event::Text(t) | Event::Code(t) => alt.push_str(&t),
                Event::End(TagEnd::Image) => {
                    let (dest, title, alt) = image.take().unwrap();
                    let title = if title.is_empty() { String::new() } else { format!(" title=\"{}\"", escape(&title)) };
                    events.push(Event::InlineHtml(
                        format!("<img src=\"{}\" alt=\"{}\"{title}>", escape(&dest), escape(&alt)).into(),
                    ));
                }
                _ => {}
            }
            continue;
        }
        match event {
            Event::Start(Tag::Heading { level, classes, attrs, .. }) => {
                let id = heading_id(&heading_text(&markdown[range]), &mut ids);
                events.push(Event::Start(Tag::Heading { level, id: Some(CowStr::from(id)), classes, attrs }));
            }
            Event::Start(Tag::Link { link_type, dest_url, title, id }) => {
                let dest = if let Some(m) = INTERNAL_LINK.captures(&dest_url) {
                    format!("docs/{}{}", &m[3], &m[4])
                } else if dest_url.starts_with('#') {
                    format!("docs/{document}{dest_url}")
                } else {
                    dest_url.to_string()
                };
                events.push(Event::Start(Tag::Link { link_type, dest_url: dest.into(), title, id }));
            }
            Event::Start(Tag::Image { dest_url, title, .. }) => {
                let dest =
                    if dest_url.contains('/') { dest_url.to_string() } else { format!("assets/docs/{dest_url}") };
                image = Some((dest, title.to_string(), String::new()));
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                let language = match kind {
                    CodeBlockKind::Fenced(info) => info.split_whitespace().next().unwrap_or_default().to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                code = Some((language, String::new()));
            }
            Event::Text(text) if code.is_some() => code.as_mut().unwrap().1.push_str(&text),
            Event::End(TagEnd::CodeBlock) if code.is_some() => {
                let (language, text) = code.take().unwrap();
                // Goldmark starts a code block on its own line, even after
                // the text of a tight list item.
                let html = if language.is_empty() {
                    format!("\n<pre><code>{}</code></pre>\n", escape(&text))
                } else if UNKNOWN_LANGUAGES.contains(&language.as_str()) {
                    // No lexer for these in the Go version.
                    format!("\n<pre><code class=\"language-{}\">{}</code></pre>\n", escape(&language), escape(&text))
                } else {
                    format!("\n{}", highlight::code_block(&language, &text))
                };
                events.push(Event::Html(html.into()));
            }
            Event::Start(Tag::BlockQuote(Some(kind))) => events.push(Event::Html(admonition_header(kind).into())),
            Event::End(TagEnd::BlockQuote(Some(_))) => events.push(Event::Html("</div>".into())),
            // Like goldmark, raw HTML is not rendered.
            Event::Html(_) => events.push(Event::Html("<!-- raw HTML omitted -->\n".into())),
            Event::InlineHtml(_) => events.push(Event::InlineHtml("<!-- raw HTML omitted -->".into())),
            // Escaped like goldmark, quotes included.
            Event::Text(text) => events.push(Event::InlineHtml(escape(&text).into())),
            Event::Code(text) => events.push(Event::InlineHtml(format!("<code>{}</code>", escape(&text)).into())),
            e => events.push(e),
        }
    }
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, events.into_iter());
    html
}

/// Languages the Go version does not highlight.
const UNKNOWN_LANGUAGES: &[&str] = &["junos", "cisco", "eos"];

/// Escape like goldmark: &, <, > and the double quote.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

impl Docs {
    pub fn new(mut files: Vec<(String, Vec<u8>)>) -> Self {
        files.sort_by(|a, b| a.0.cmp(&b.0));
        Docs { files, toc: OnceLock::new() }
    }

    /// The documents, with their number and their name.
    fn documents(&self) -> impl Iterator<Item = (&str, &str, &[u8])> {
        self.files.iter().filter_map(|(file, data)| {
            let m = INTERNAL_LINK.captures(file)?;
            Some((m.get(2).unwrap().as_str(), m.get(3).unwrap().as_str(), data.as_slice()))
        })
    }

    /// The source of a document.
    pub fn find(&self, name: &str) -> Option<&[u8]> {
        let name = renamed(name);
        self.documents().find(|(_, n, _)| *n == name).map(|(_, _, d)| d)
    }

    /// A static file of the documentation (images).
    pub fn asset(&self, path: &str) -> Option<&[u8]> {
        self.files.iter().find(|(n, _)| n == path).map(|(_, d)| d.as_slice())
    }

    /// The table of contents of all the documents.
    pub fn toc(&self) -> &[DocumentToc] {
        self.toc.get_or_init(|| {
            self.documents()
                .map(|(number, name, data)| DocumentToc {
                    name: name.into(),
                    section: document_section(number).into(),
                    headers: document_headers(&String::from_utf8_lossy(data)),
                })
                .collect()
        })
    }

    /// A Markdown index of the documentation, grouped by section.
    pub fn index(&self, url_prefix: &str) -> String {
        let mut index = String::from("# FlowScope documentation\n\n");
        let mut section = "";
        for (number, name, data) in self.documents() {
            let head = &data[..data.len().min(1024)];
            let head = String::from_utf8_lossy(head);
            let Some(title) = DOCUMENT_TITLE.captures(&head).map(|m| m[1].to_string()) else { continue };
            let current = document_section(number);
            if current != section {
                section = current;
                if !section.is_empty() {
                    index.push_str(&format!("- **{section}**\n"));
                }
            }
            let indent = if section.is_empty() { "" } else { "  " };
            index.push_str(&format!("{indent}- [{title}]({url_prefix}docs/{name})\n"));
        }
        index
    }
}

/// The quality the Accept header gives to a media type. 0 when it is not
/// named. Wildcards are ignored.
fn media_type_quality(accept: &str, media_type: &str) -> f64 {
    for entry in accept.split(',') {
        let mut parts = entry.split(';');
        let range = parts.next().unwrap_or_default().trim().to_lowercase();
        if range != media_type {
            continue;
        }
        for param in parts {
            if let Some((k, v)) = param.split_once('=')
                && k.trim().eq_ignore_ascii_case("q")
            {
                return v.trim().parse().unwrap_or(1.0);
            }
        }
        return 1.0;
    }
    0.0
}

/// The client asks for this media type rather than for Markdown.
fn prefers_over_markdown(accept: &str, media_type: &str) -> bool {
    let quality = media_type_quality(accept, media_type);
    quality > 0.0 && quality > media_type_quality(accept, TYPE_MARKDOWN)
}

/// Answer with the source of a document, with the links to the other
/// documents rewritten.
fn markdown_response(markdown: &[u8]) -> Response {
    let text = String::from_utf8_lossy(markdown);
    let text = MARKDOWN_LINK.replace_all(&text, "]($2$3)").into_owned();
    (StatusCode::OK, [(header::CONTENT_TYPE, "text/markdown; charset=utf-8")], text).into_response()
}

fn accept(headers: &HeaderMap) -> &str {
    headers.get(header::ACCEPT).and_then(|v| v.to_str().ok()).unwrap_or_default()
}

/// GET /api/v0/console/docs/{name}
pub async fn docs_handler(State(c): State<Arc<Console>>, Path(name): Path<String>, headers: HeaderMap) -> Response {
    let name = renamed(&name).to_string();
    let docs = c.docs();
    let mut response = match docs.find(&name) {
        None => json_error(StatusCode::NOT_FOUND, "Document not found."),
        Some(markdown) => {
            let mut response = if !prefers_over_markdown(accept(&headers), TYPE_JSON) {
                // The most useful answer for a crawler or an LLM.
                markdown_response(markdown)
            } else {
                let html = render(&String::from_utf8_lossy(markdown), &name);
                pure_json(&serde_json::json!({"markdown": html, "toc": docs.toc()}))
            };
            response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("max-age=300, public"));
            response
        }
    };
    response.headers_mut().insert(header::VARY, HeaderValue::from_static("Accept"));
    response
}

/// JSON without escaping the HTML characters, like Go's WritePureJSON.
fn pure_json(value: &serde_json::Value) -> Response {
    let mut body = serde_json::to_vec(value).unwrap_or_default();
    body.push(b'\n');
    (StatusCode::OK, [(header::CONTENT_TYPE, "application/json; charset=utf-8")], body).into_response()
}

/// Answer with the source of a document when the request is for a page of
/// the documentation of the web interface and the client does not prefer
/// HTML. The root of the documentation gets an index. None when the
/// application should answer.
pub fn serve_markdown_document(c: &Console, path: &str, headers: &HeaderMap) -> Result<Response, bool> {
    let name = if path == "/docs" {
        ""
    } else {
        match path.strip_prefix("/docs/") {
            Some(n) if !n.contains('/') => n,
            // Not a documentation page.
            _ => return Err(false),
        }
    };
    // The answer now depends on the Accept header.
    if prefers_over_markdown(accept(headers), TYPE_HTML) {
        return Err(true);
    }
    let markdown = if name.is_empty() {
        Some(c.docs().index(&c.config().url_prefix()).into_bytes())
    } else {
        c.docs().find(name).map(|d| d.to_vec())
    };
    let Some(markdown) = markdown else { return Err(true) };
    let mut response = markdown_response(&markdown);
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("max-age=300, public"));
    response.headers_mut().insert(header::VARY, HeaderValue::from_static("Accept"));
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids() {
        let mut ids = HashSet::new();
        assert_eq!(heading_id("Getting help", &mut ids), "getting-help");
        assert_eq!(heading_id("Getting help", &mut ids), "getting-help-1");
        assert_eq!(heading_id("`inlet` service", &mut ids), "inlet-service");
        assert_eq!(heading_id("ÉÀ", &mut ids), "heading");
        assert_eq!(heading_text("## Title ##\n"), "Title");
        assert_eq!(heading_text("# C# notes\n"), "C# notes");
    }

    #[test]
    fn quality() {
        assert!(!prefers_over_markdown("", TYPE_JSON));
        assert!(prefers_over_markdown("application/json", TYPE_JSON));
        assert!(!prefers_over_markdown("text/markdown;q=0.9, text/html;q=0.1", TYPE_HTML));
        assert!(prefers_over_markdown("text/markdown;q=0.9, text/html;q=0.99", TYPE_HTML));
    }
}

#[cfg(test)]
mod golden {
    use super::*;
    use serde::Deserialize;
    use std::collections::BTreeMap;

    #[derive(Deserialize)]
    struct Doc {
        status: u16,
        body: String,
    }

    #[derive(Deserialize)]
    struct Golden {
        docs: BTreeMap<String, Doc>,
        markdown: BTreeMap<String, String>,
        index: String,
    }

    /// Remove what differs only in form: the highlighting of the code and
    /// the whitespace between tags.
    fn normalize(html: &str) -> String {
        let spans = Regex::new(r#"<span[^>]*>|</span>"#).unwrap();
        let mut out = String::new();
        let mut rest = html;
        while let Some(start) = rest.find("<pre") {
            out.push_str(&rest[..start]);
            let end = rest[start..].find("</pre>").map(|e| start + e + 6).unwrap_or(rest.len());
            out.push_str(&spans.replace_all(&rest[start..end], ""));
            rest = &rest[end..];
        }
        out.push_str(rest);
        let between = Regex::new(r">\s+<").unwrap();
        between.replace_all(out.trim(), "><").into_owned()
    }

    #[test]
    fn same_as_go() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/golden/testdata/docs.json");
        let golden: Golden = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        // The answers of Go were recorded for the documentation as it was
        // then. A copy of it is kept with them, so the documentation can
        // change without the Go code to record again.
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/golden/testdata/docs");
        let mut files: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .map(|path| (path.file_name().unwrap().to_string_lossy().to_string(), std::fs::read(&path).unwrap()))
            .collect();
        files.sort();
        let docs = Docs::new(files);
        // The title is written in the code of the Go version.
        assert_eq!(docs.index("/"), golden.index.replace("# Akvorado documentation", "# FlowScope documentation"));
        let mut failures = vec![];
        for (name, doc) in &golden.docs {
            let Some(markdown) = docs.find(name) else {
                assert_eq!(doc.status, 404, "{name}");
                continue;
            };
            let body: serde_json::Value = serde_json::from_str(&doc.body).unwrap();
            assert_eq!(serde_json::to_value(docs.toc()).unwrap(), body["toc"], "toc");
            let got = normalize(&render(&String::from_utf8_lossy(markdown), renamed(name)));
            let want = normalize(body["markdown"].as_str().unwrap());
            if got != want {
                let at = got.chars().zip(want.chars()).take_while(|(a, b)| a == b).count();
                let ctx = |s: &str| s.chars().skip(at.saturating_sub(150)).take(400).collect::<String>();
                failures.push(format!("{name}:\n got  ...{}\n want ...{}", ctx(&got), ctx(&want)));
            }
            let text = String::from_utf8_lossy(markdown);
            let rewritten = MARKDOWN_LINK.replace_all(&text, "]($2$3)");
            assert_eq!(rewritten, golden.markdown[name], "markdown of {name}");
        }
        for f in failures.iter().take(5) {
            eprintln!("{f}\n");
        }
        assert!(failures.is_empty(), "{} documents differ", failures.len());
    }
}
