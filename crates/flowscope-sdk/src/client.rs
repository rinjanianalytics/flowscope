// SPDX-FileCopyrightText: 2026 FlowScope contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! HTTP client for the console API.

use crate::models::*;
use crate::openapi::PREFIX;
use reqwest::header::ACCEPT;
use reqwest::{Method, StatusCode};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// What can go wrong when calling the API.
#[derive(Debug)]
pub enum Error {
    /// The URL of the console is not usable.
    Url(String),
    /// The request did not get an answer.
    Http(reqwest::Error),
    /// The server refused the request. The message is the one it gave.
    Api { status: u16, message: String },
    /// The answer is not what the client expects.
    Decode(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Url(message) => write!(f, "invalid console URL: {message}"),
            Error::Http(e) => write!(f, "cannot reach the console: {e}"),
            Error::Api { status, message } => write!(f, "the console answered {status}: {message}"),
            Error::Decode(message) => write!(f, "unexpected answer from the console: {message}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Http(e) => Some(e),
            _ => None,
        }
    }
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Error {
        Error::Http(e)
    }
}

impl Error {
    /// The HTTP status of a refused request.
    pub fn status(&self) -> Option<u16> {
        match self {
            Error::Api { status, .. } => Some(*status),
            _ => None,
        }
    }
}

/// A client for the console API.
///
/// The console does not log users in itself: it trusts the headers set by
/// the proxy in front of it. To go through such a proxy, build a
/// [`reqwest::Client`] with what the proxy needs (a token, a cookie) and
/// give it to [`Client::with_http_client`].
#[derive(Clone, Debug)]
pub struct Client {
    http: reqwest::Client,
    /// URL of the API, without trailing slash.
    api: String,
}

impl Client {
    /// A client for the console at this URL, like `http://127.0.0.1:8080`
    /// or `https://example.net/flowscope`.
    pub fn new(console_url: &str) -> Result<Client, Error> {
        Client::with_http_client(console_url, reqwest::Client::new())
    }

    /// Like [`Client::new`], with your own HTTP client.
    pub fn with_http_client(console_url: &str, http: reqwest::Client) -> Result<Client, Error> {
        let base = console_url.trim_end_matches('/');
        let url = reqwest::Url::parse(base).map_err(|e| Error::Url(e.to_string()))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(Error::Url(format!("unsupported scheme {:?}", url.scheme())));
        }
        Ok(Client { http, api: format!("{base}{PREFIX}") })
    }

    async fn send<B: Serialize + ?Sized>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<reqwest::Response, Error> {
        self.send_accepting(method, path, body, "application/json").await
    }

    async fn send_accepting<B: Serialize + ?Sized>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
        accept: &str,
    ) -> Result<reqwest::Response, Error> {
        let mut request = self.http.request(method, format!("{}{path}", self.api)).header(ACCEPT, accept);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await?;
        let status = response.status();
        if status.is_success() || status == StatusCode::NOT_MODIFIED {
            return Ok(response);
        }
        let text = response.text().await.unwrap_or_default();
        let message = match serde_json::from_str::<ErrorMessage>(&text) {
            Ok(error) => error.message,
            Err(_) => status.canonical_reason().unwrap_or("error").to_string(),
        };
        Err(Error::Api { status: status.as_u16(), message })
    }

    async fn decode<T: DeserializeOwned>(response: reqwest::Response) -> Result<T, Error> {
        let bytes = response.bytes().await?;
        serde_json::from_slice(&bytes).map_err(|e| Error::Decode(e.to_string()))
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, Error> {
        Self::decode(self.send::<()>(Method::GET, path, None).await?).await
    }

    async fn post<B: Serialize + ?Sized, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T, Error> {
        Self::decode(self.send(Method::POST, path, Some(body)).await?).await
    }

    /// What the web interface needs to start: version, dimensions, defaults.
    pub async fn configuration(&self) -> Result<Configuration, Error> {
        self.get("/configuration").await
    }

    /// The current user.
    pub async fn user_info(&self) -> Result<UserInfo, Error> {
        self.get("/user/info").await
    }

    /// Picture of the current user, as a PNG file.
    pub async fn user_avatar(&self) -> Result<Vec<u8>, Error> {
        let response = self.send_accepting::<()>(Method::GET, "/user/avatar", None, "image/png").await?;
        Ok(response.bytes().await?.to_vec())
    }

    /// Traffic over time.
    pub async fn graph_line(&self, input: &LineInput) -> Result<LineOutput, Error> {
        self.post("/graph/line", input).await
    }

    /// Traffic between the values of several dimensions.
    pub async fn graph_sankey(&self, input: &GraphInput) -> Result<SankeyOutput, Error> {
        self.post("/graph/sankey", input).await
    }

    /// Table and resolution a graph would use.
    pub async fn graph_table_interval(&self, input: &TableIntervalInput) -> Result<TableIntervalOutput, Error> {
        self.post("/graph/table-interval", input).await
    }

    /// Swap the direction of dimensions and of a filter: sources become
    /// destinations, inputs become outputs.
    pub async fn graph_reverse(&self, input: &Reverse) -> Result<Reverse, Error> {
        self.post("/graph/reverse", input).await
    }

    /// Check a filter expression. An invalid expression is not an error of
    /// the call: look at [`ValidateOutput::is_valid`].
    pub async fn filter_validate(&self, filter: &str) -> Result<ValidateOutput, Error> {
        self.post("/filter/validate", &ValidateInput { filter: filter.to_string() }).await
    }

    /// Ways to complete a filter expression.
    pub async fn filter_complete(&self, input: &CompleteInput) -> Result<Vec<Completion>, Error> {
        Ok(self.post::<_, CompleteOutput>("/filter/complete", input).await?.completions)
    }

    /// Saved filters the current user can see.
    pub async fn saved_filters(&self) -> Result<Vec<SavedFilter>, Error> {
        Ok(self.get::<SavedFilters>("/filter/saved").await?.filters)
    }

    /// Save a filter for the current user.
    pub async fn add_saved_filter(&self, filter: &SavedFilterInput) -> Result<(), Error> {
        self.send(Method::POST, "/filter/saved", Some(filter)).await.map(|_| ())
    }

    /// Delete a filter of the current user.
    pub async fn delete_saved_filter(&self, id: u64) -> Result<(), Error> {
        self.send::<()>(Method::DELETE, &format!("/filter/saved/{id}"), None).await.map(|_| ())
    }

    /// The most recent flow. `None` when there is no flow yet.
    pub async fn widget_flow_last(&self) -> Result<Option<LastFlow>, Error> {
        match self.get("/widget/flow-last").await {
            Ok(flow) => Ok(Some(flow)),
            Err(Error::Api { status: 404, .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Flows received each second.
    pub async fn widget_flow_rate(&self) -> Result<FlowRate, Error> {
        self.get("/widget/flow-rate").await
    }

    /// Names of the exporters sending flows.
    pub async fn widget_exporters(&self) -> Result<Vec<String>, Error> {
        Ok(self.get::<Exporters>("/widget/exporters").await?.exporters)
    }

    /// Top values of a column: `src-as`, `dst-as`, `src-country`,
    /// `dst-country`, `exporter`, `protocol`, `etype`, `src-port` or
    /// `dst-port`.
    pub async fn widget_top(&self, name: &str) -> Result<Vec<TopEntry>, Error> {
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(Error::Api { status: 400, message: format!("invalid widget name {name:?}") });
        }
        Ok(self.get::<Top>(&format!("/widget/top/{name}")).await?.top)
    }

    /// Traffic of the last day.
    pub async fn widget_graph(&self) -> Result<Vec<TrafficPoint>, Error> {
        Ok(self.get::<Traffic>("/widget/graph").await?.data)
    }

    /// A page of the documentation, as HTML, with the headers of all pages.
    pub async fn document(&self, name: &str) -> Result<Document, Error> {
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(Error::Api { status: 404, message: "Document not found.".into() });
        }
        self.get(&format!("/docs/{name}")).await
    }

    /// A page of the documentation, as its Markdown source.
    pub async fn document_markdown(&self, name: &str) -> Result<String, Error> {
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(Error::Api { status: 404, message: "Document not found.".into() });
        }
        let response = self.send_accepting::<()>(Method::GET, &format!("/docs/{name}"), None, "text/markdown").await?;
        Ok(response.text().await?)
    }

    /// The OpenAPI document of the console.
    pub async fn openapi(&self) -> Result<serde_json::Value, Error> {
        self.get("/openapi.json").await
    }
}
