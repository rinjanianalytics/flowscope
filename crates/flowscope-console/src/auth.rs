// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! User authentication. It does not really authenticate: it relies on HTTP
//! headers set by a reverse proxy.

use axum::extract::Request;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use flowscope_common::config::Validate;
use flowscope_common::httpserver::json_error;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, LazyLock};

/// Configuration of the authentication.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Configuration {
    /// Headers with the user information.
    pub headers: Headers,
    /// User when no header is present. An empty login refuses access
    /// without authentication.
    pub default_user: UserInformation,
    /// URL to log out, templated from the user information.
    #[serde(rename = "logout-url")]
    pub logout_url: String,
    /// URL of the avatar, templated from the user information.
    #[serde(rename = "avatar-url")]
    pub avatar_url: String,
}

/// Headers with the user information. An empty name is skipped.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Headers {
    pub login: String,
    pub name: String,
    pub email: String,
    #[serde(rename = "logout-url")]
    pub logout_url: String,
    #[serde(rename = "avatar-url")]
    pub avatar_url: String,
}

impl Default for Headers {
    fn default() -> Self {
        Headers {
            login: "Remote-User".into(),
            name: "Remote-Name".into(),
            email: "Remote-Email".into(),
            logout_url: "X-Logout-URL".into(),
            avatar_url: "X-Avatar-URL".into(),
        }
    }
}

impl Default for Configuration {
    fn default() -> Self {
        Configuration {
            headers: Headers::default(),
            default_user: UserInformation {
                login: "__default".into(),
                name: "Default User".into(),
                ..Default::default()
            },
            logout_url: String::new(),
            avatar_url: String::new(),
        }
    }
}

impl Validate for Configuration {}

/// Information about the current user.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UserInformation {
    pub login: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub email: String,
    #[serde(rename = "logout-url", skip_serializing_if = "String::is_empty")]
    pub logout_url: String,
    #[serde(rename = "avatar-url", skip_serializing_if = "String::is_empty")]
    pub avatar_url: String,
}

/// The email check of the Go validator (HTML5 rule).
static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:(?:(?:(?:[a-zA-Z]|\d|[!#\$%&'\*\+\-\/=\?\^_`{\|}~]|[\x{00A0}-\x{D7FF}\x{F900}-\x{FDCF}\x{FDF0}-\x{FFEF}])+(?:\.([a-zA-Z]|\d|[!#\$%&'\*\+\-\/=\?\^_`{\|}~]|[\x{00A0}-\x{D7FF}\x{F900}-\x{FDCF}\x{FDF0}-\x{FFEF}])+)*)|(?:(?:\x22)(?:(?:(?:(?:\x20|\x09)*(?:\x0d\x0a))?(?:\x20|\x09)+)?(?:(?:[\x01-\x08\x0b\x0c\x0e-\x1f\x7f]|\x21|[\x23-\x5b]|[\x5d-\x7e]|[\x{00A0}-\x{D7FF}\x{F900}-\x{FDCF}\x{FDF0}-\x{FFEF}])|(?:(?:[\x01-\x09\x0b\x0c\x0d-\x7f]|[\x{00A0}-\x{D7FF}\x{F900}-\x{FDCF}\x{FDF0}-\x{FFEF}]))))*(?:(?:(?:\x20|\x09)*(?:\x0d\x0a))?(\x20|\x09)+)?(?:\x22))))@(?:(?:(?:[a-zA-Z]|\d|[\x{00A0}-\x{D7FF}\x{F900}-\x{FDCF}\x{FDF0}-\x{FFEF}])|(?:(?:[a-zA-Z]|\d|[\x{00A0}-\x{D7FF}\x{F900}-\x{FDCF}\x{FDF0}-\x{FFEF}])(?:[a-zA-Z]|\d|-|\.|~|[\x{00A0}-\x{D7FF}\x{F900}-\x{FDCF}\x{FDF0}-\x{FFEF}])*(?:[a-zA-Z]|\d|[\x{00A0}-\x{D7FF}\x{F900}-\x{FDCF}\x{FDF0}-\x{FFEF}])))\.)+(?:(?:[a-zA-Z]|[\x{00A0}-\x{D7FF}\x{F900}-\x{FDCF}\x{FDF0}-\x{FFEF}])|(?:(?:[a-zA-Z]|[\x{00A0}-\x{D7FF}\x{F900}-\x{FDCF}\x{FDF0}-\x{FFEF}])(?:[a-zA-Z]|\d|-|\.|~|[\x{00A0}-\x{D7FF}\x{F900}-\x{FDCF}\x{FDF0}-\x{FFEF}])*(?:[a-zA-Z]|[\x{00A0}-\x{D7FF}\x{F900}-\x{FDCF}\x{FDF0}-\x{FFEF}])))\.?$",
    )
    .unwrap()
});

/// The URI check of the Go validator: Go's url.ParseRequestURI accepts an
/// absolute URL or an absolute path.
fn is_uri(s: &str) -> bool {
    let s = s.split_once('#').map(|(u, _)| u).unwrap_or(s);
    s.starts_with('/') || url::Url::parse(s).is_ok_and(|u| !u.scheme().is_empty())
}

impl UserInformation {
    fn is_valid(&self) -> bool {
        (self.email.is_empty() || EMAIL.is_match(&self.email))
            && (self.logout_url.is_empty() || is_uri(&self.logout_url))
            && (self.avatar_url.is_empty() || is_uri(&self.avatar_url))
    }

    fn field(&self, name: &str) -> Option<&str> {
        Some(match name {
            "Login" => &self.login,
            "Name" => &self.name,
            "Email" => &self.email,
            "LogoutURL" => &self.logout_url,
            "AvatarURL" => &self.avatar_url,
            _ => return None,
        })
    }
}

/// A URL template: the `{{ .Field }}` actions of Go templates.
#[derive(Clone, Debug)]
struct Template(Vec<Part>);

#[derive(Clone, Debug)]
enum Part {
    Text(String),
    Field(String),
}

static ACTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{\{-?\s*\.([A-Za-z_][A-Za-z0-9_]*)\s*-?\}\}").unwrap());

impl Template {
    /// Parse a template. None when it uses something else than fields,
    /// like Go ignoring a template that does not parse.
    fn parse(s: &str) -> Option<Template> {
        let mut parts = vec![];
        let mut last = 0;
        for m in ACTION.captures_iter(s) {
            let whole = m.get(0).unwrap();
            parts.push(Part::Text(s[last..whole.start()].to_string()));
            parts.push(Part::Field(m[1].to_string()));
            last = whole.end();
        }
        parts.push(Part::Text(s[last..].to_string()));
        if parts.iter().any(|p| matches!(p, Part::Text(t) if t.contains("{{"))) {
            return None;
        }
        Some(Template(parts))
    }

    /// Execute a template. None when a field is unknown.
    fn execute(&self, info: &UserInformation) -> Option<String> {
        let mut out = String::new();
        for p in &self.0 {
            match p {
                Part::Text(t) => out.push_str(t),
                Part::Field(f) => out.push_str(info.field(f)?),
            }
        }
        Some(out)
    }
}

/// The authentication component.
#[derive(Clone, Debug)]
pub struct Authentication {
    config: Arc<Configuration>,
    logout_url: Option<Template>,
    avatar_url: Option<Template>,
}

impl Authentication {
    pub fn new(config: Configuration) -> Self {
        let parse = |s: &str| if s.is_empty() { None } else { Template::parse(s) };
        Authentication {
            logout_url: parse(&config.logout_url),
            avatar_url: parse(&config.avatar_url),
            config: Arc::new(config),
        }
    }

    fn user_from_headers(&self, headers: &HeaderMap) -> UserInformation {
        let get = |name: &str| {
            if name.is_empty() {
                return String::new();
            }
            headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or_default().to_string()
        };
        let h = &self.config.headers;
        UserInformation {
            login: get(&h.login),
            name: get(&h.name),
            email: get(&h.email),
            logout_url: get(&h.logout_url),
            avatar_url: get(&h.avatar_url),
        }
    }

    /// The user of a request. None when no user is logged in and there is
    /// no default user.
    pub fn user(&self, headers: &HeaderMap) -> Option<UserInformation> {
        let mut info = self.user_from_headers(headers);
        if info.login.is_empty() || !info.is_valid() {
            if self.config.default_user.login.is_empty() {
                return None;
            }
            info = self.config.default_user.clone();
        }
        // The templates can use the header values and keep or override them.
        if let Some(url) = self.logout_url.as_ref().and_then(|t| t.execute(&info)) {
            info.logout_url = url;
        }
        if let Some(url) = self.avatar_url.as_ref().and_then(|t| t.execute(&info)) {
            info.avatar_url = url;
        }
        Some(info)
    }

    /// Middleware storing the current user in the request extensions.
    pub async fn middleware(self, mut req: Request, next: Next) -> Response {
        match self.user(req.headers()) {
            Some(info) => {
                req.extensions_mut().insert(info);
                next.run(req).await
            }
            None => json_error(StatusCode::UNAUTHORIZED, "No user logged in."),
        }
    }
}

/// The current user, as set by the middleware.
pub fn current_user(req_extensions: &axum::http::Extensions) -> UserInformation {
    req_extensions.get::<UserInformation>().cloned().unwrap_or_default()
}

/// Go's PCG random generator (math/rand/v2), to pick the same avatar parts.
struct Pcg {
    hi: u64,
    lo: u64,
}

impl Pcg {
    fn next(&mut self) -> (u64, u64) {
        const MUL_HI: u64 = 2549297995355413924;
        const MUL_LO: u64 = 4865540595714422341;
        const INC_HI: u64 = 6364136223846793005;
        const INC_LO: u64 = 1442695040888963407;
        let prod = (self.lo as u128) * (MUL_LO as u128);
        let (mut hi, lo) = ((prod >> 64) as u64, prod as u64);
        hi = hi.wrapping_add(self.hi.wrapping_mul(MUL_LO)).wrapping_add(self.lo.wrapping_mul(MUL_HI));
        let (lo, carry) = lo.overflowing_add(INC_LO);
        let hi = hi.wrapping_add(INC_HI).wrapping_add(carry as u64);
        self.lo = lo;
        self.hi = hi;
        (hi, lo)
    }

    fn uint64(&mut self) -> u64 {
        let (mut hi, lo) = self.next();
        const CHEAP_MUL: u64 = 0xda942042e4dd58b5;
        hi ^= hi >> 32;
        hi = hi.wrapping_mul(CHEAP_MUL);
        hi ^= hi >> 48;
        hi.wrapping_mul(lo | 1)
    }

    /// Go's `Rand.IntN` on a 64-bit system.
    fn int_n(&mut self, n: u64) -> u64 {
        if n & (n - 1) == 0 {
            return self.uint64() & (n - 1);
        }
        let mul = |x: u64| {
            let p = (x as u128) * (n as u128);
            ((p >> 64) as u64, p as u64)
        };
        let (mut hi, mut lo) = mul(self.uint64());
        if lo < n {
            let thresh = n.wrapping_neg() % n;
            while lo < thresh {
                (hi, lo) = mul(self.uint64());
            }
        }
        hi
    }
}

/// FNV-1 64-bit hash (Go's hash/fnv New64).
fn fnv1_64(data: &[u8]) -> u64 {
    let mut hash: u64 = 14695981039346656037;
    for b in data {
        hash = hash.wrapping_mul(1099511628211);
        hash ^= *b as u64;
    }
    hash
}

/// Decode a PNG part as non-premultiplied RGBA.
fn decode_part(data: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let mut decoder = png::Decoder::new(data);
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf.chunks(3).flat_map(|c| [c[0], c[1], c[2], 0xff]).collect(),
        png::ColorType::GrayscaleAlpha => buf.chunks(2).flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 0xff]).collect(),
        other => return Err(format!("unsupported PNG color type {other:?}")),
    };
    Ok((info.width, info.height, rgba))
}

/// Draw a non-premultiplied image over a premultiplied one, like Go's
/// draw.Draw with draw.Over.
fn draw_over(dst: &mut [u8], src: &[u8]) {
    const M: u32 = (1 << 16) - 1;
    for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
        let sa = s[3] as u32 * 0x101;
        let sr = s[0] as u32 * sa / 0xff;
        let sg = s[1] as u32 * sa / 0xff;
        let sb = s[2] as u32 * sa / 0xff;
        let a = (M - sa) * 0x101;
        d[0] = ((d[0] as u32 * a / M + sr) >> 8) as u8;
        d[1] = ((d[1] as u32 * a / M + sg) >> 8) as u8;
        d[2] = ((d[2] as u32 * a / M + sb) >> 8) as u8;
        d[3] = ((d[3] as u32 * a / M + sa) >> 8) as u8;
    }
}

/// Encode a premultiplied image as PNG, like Go's encoder: RGB when opaque,
/// non-premultiplied RGBA otherwise.
fn encode_png(width: u32, height: u32, pix: &[u8]) -> Result<Vec<u8>, String> {
    let opaque = pix.chunks_exact(4).all(|p| p[3] == 0xff);
    let (color, data): (png::ColorType, Vec<u8>) = if opaque {
        (png::ColorType::Rgb, pix.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect())
    } else {
        const M: u32 = 0x101 * 0xffff;
        let data = pix
            .chunks_exact(4)
            .flat_map(|s| match s[3] {
                0 => [0, 0, 0, 0],
                0xff => [s[0], s[1], s[2], s[3]],
                alpha => {
                    let a = alpha as u32 * 0x101;
                    let c = |v: u8| ((v as u32 * M / a) >> 8) as u8;
                    [c(s[0]), c(s[1]), c(s[2]), alpha]
                }
            })
            .collect();
        (png::ColorType::Rgba, data)
    };
    let mut out = vec![];
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(color);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(&data).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// Build the avatar of a login: one part of each kind, picked from the
/// login hash. Returns the image (premultiplied RGBA) and its size.
fn avatar_pixels(login: &str) -> Result<(u32, u32, Vec<u8>), String> {
    let mut rand = Pcg { hi: fnv1_64(login.as_bytes()), lo: 0 };
    let file = |name: &str| crate::embedded::AVATARS.iter().find(|(n, _)| *n == name).map(|(_, d)| *d);
    let partlist = file("partlist.txt").ok_or("missing partlist.txt")?;
    let mut image: Option<(u32, u32, Vec<u8>)> = None;
    for part in String::from_utf8_lossy(partlist).lines() {
        // Like fs.Glob, the candidates are sorted by name.
        let prefix = format!("{part}_");
        let mut candidates: Vec<&str> =
            crate::embedded::AVATARS.iter().map(|(n, _)| *n).filter(|n| n.starts_with(&prefix)).collect();
        candidates.sort();
        if candidates.is_empty() {
            return Err(format!("missing part {part}"));
        }
        let chosen = candidates[rand.int_n(candidates.len() as u64) as usize];
        let (w, h, pix) =
            decode_part(file(chosen).unwrap()).map_err(|e| format!("cannot decode part {chosen}: {e}"))?;
        let (_, _, dst) = image.get_or_insert_with(|| (w, h, vec![0; (w * h * 4) as usize]));
        draw_over(dst, &pix);
    }
    image.ok_or_else(|| "no avatar part".to_string())
}

/// Answer with the avatar of the current user.
pub fn avatar_response(info: &UserInformation, if_none_match: Option<&HeaderValue>) -> Response {
    let hash = fnv1_64(info.login.as_bytes());
    let etag = format!("\"{hash:x}\"");
    if if_none_match.and_then(|v| v.to_str().ok()) == Some(etag.as_str()) {
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response();
    }
    match avatar_pixels(&info.login).and_then(|(w, h, pix)| encode_png(w, h, &pix)) {
        Ok(png) => (StatusCode::OK, [(header::CONTENT_TYPE, "image/png".to_string()), (header::ETAG, etag)], png)
            .into_response(),
        Err(e) => {
            tracing::error!(error = %e, "cannot build avatar");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "Cannot build avatar.")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize)]
    struct Golden {
        avatars: std::collections::BTreeMap<String, String>,
        etags: std::collections::BTreeMap<String, String>,
        pcg: Vec<u64>,
        intn: Vec<u64>,
    }

    /// Decode standard base64 (Go encodes byte slices this way in JSON).
    fn base64(s: &str) -> Vec<u8> {
        let value = |c: u8| match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            _ => 63,
        };
        let digits: Vec<u8> = s.bytes().filter(|&c| c != b'=').map(value).collect();
        let mut out = vec![];
        for chunk in digits.chunks(4) {
            let n = chunk.iter().enumerate().fold(0u32, |acc, (i, &d)| acc | (d as u32) << (18 - 6 * i));
            out.extend(&n.to_be_bytes()[1..chunk.len()]);
        }
        out
    }

    fn golden() -> Golden {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/golden/testdata/avatars.json");
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn pcg() {
        let golden = golden();
        let mut p = Pcg { hi: 1, lo: 2 };
        let got: Vec<u64> = (0..3).map(|_| p.uint64()).collect();
        assert_eq!(got, golden.pcg);
        let got: Vec<u64> = [3, 7, 20, 64, 1000000007].iter().map(|&n| p.int_n(n)).collect();
        assert_eq!(got, golden.intn);
    }

    /// Avatars have the same pixels as the ones of Go, and the same ETag.
    #[test]
    fn avatars() {
        let golden = golden();
        for (login, want) in &golden.avatars {
            let info = UserInformation { login: login.clone(), ..Default::default() };
            let response = avatar_response(&info, None);
            assert_eq!(response.headers()[header::ETAG], golden.etags[login], "{login:?}");
            let (w, h, pix) = avatar_pixels(login).unwrap();
            let got = decode_part(&encode_png(w, h, &pix).unwrap()).unwrap();
            assert_eq!(got, decode_part(&base64(want)).unwrap(), "{login:?}");
        }
        let info = UserInformation { login: "alfred".into(), ..Default::default() };
        let etag = HeaderValue::from_str(&golden.etags["alfred"]).unwrap();
        assert_eq!(avatar_response(&info, Some(&etag)).status(), StatusCode::NOT_MODIFIED);
    }

    #[test]
    fn templates() {
        let info = UserInformation { login: "alfred".into(), ..Default::default() };
        assert_eq!(Template::parse("/avatar/{{ .Login }}.png").unwrap().execute(&info).unwrap(), "/avatar/alfred.png");
        assert!(Template::parse("{{ if .Login }}x{{ end }}").is_none());
        assert!(Template::parse("{{ .Unknown }}").unwrap().execute(&info).is_none());
    }

    #[test]
    fn users() {
        let auth =
            Authentication::new(Configuration { logout_url: "/logout?user={{.Login}}".into(), ..Default::default() });
        let mut headers = HeaderMap::new();
        assert_eq!(auth.user(&headers).unwrap().login, "__default");
        headers.insert("Remote-User", "alfred".parse().unwrap());
        headers.insert("Remote-Email", "alfred@example.com".parse().unwrap());
        let user = auth.user(&headers).unwrap();
        assert_eq!(user.login, "alfred");
        assert_eq!(user.logout_url, "/logout?user=alfred");
        headers.insert("Remote-Email", "not an email".parse().unwrap());
        assert_eq!(auth.user(&headers).unwrap().login, "__default");
        let auth =
            Authentication::new(Configuration { default_user: UserInformation::default(), ..Default::default() });
        assert!(auth.user(&HeaderMap::new()).is_none());
    }
}
