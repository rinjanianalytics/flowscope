// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! A connection using the ClickHouse native protocol. It is synchronous:
//! callers in async code run it in a blocking task.

use crate::block::{Block, read_block, write_block, write_block_header};
use crate::compress::{Decompressor, compress};
use crate::io::*;
use std::io::{self, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

/// Protocol revision we speak.
pub const REVISION: u64 = 54460;

const REV_CLIENT_INFO: u64 = 54032;
const REV_SERVER_TIMEZONE: u64 = 54058;
const REV_QUOTA_KEY_IN_CLIENT_INFO: u64 = 54060;
const REV_SERVER_DISPLAY_NAME: u64 = 54372;
const REV_VERSION_PATCH: u64 = 54401;
const REV_CLIENT_WRITE_INFO: u64 = 54420;
const REV_SETTINGS_AS_STRINGS: u64 = 54429;
const REV_INTERSERVER_SECRET: u64 = 54441;
const REV_OPENTELEMETRY: u64 = 54442;
const REV_DISTRIBUTED_DEPTH: u64 = 54448;
const REV_INITIAL_QUERY_START_TIME: u64 = 54449;
const REV_PARALLEL_REPLICAS: u64 = 54453;
const REV_CUSTOM_SERIALIZATION: u64 = 54454;
const REV_ADDENDUM: u64 = 54458;
const REV_PARAMETERS: u64 = 54459;
const REV_SERVER_QUERY_TIME_IN_PROGRESS: u64 = 54460;
const REV_TOTAL_BYTES_IN_PROGRESS: u64 = 54463;

mod client_packet {
    pub const HELLO: u64 = 0;
    pub const QUERY: u64 = 1;
    pub const DATA: u64 = 2;
    pub const PING: u64 = 4;
}

mod server_packet {
    pub const HELLO: u64 = 0;
    pub const DATA: u64 = 1;
    pub const EXCEPTION: u64 = 2;
    pub const PROGRESS: u64 = 3;
    pub const PONG: u64 = 4;
    pub const END_OF_STREAM: u64 = 5;
    pub const PROFILE_INFO: u64 = 6;
    pub const TOTALS: u64 = 7;
    pub const EXTREMES: u64 = 8;
    pub const LOG: u64 = 10;
    pub const TABLE_COLUMNS: u64 = 11;
    pub const PART_UUIDS: u64 = 12;
    pub const PROFILE_EVENTS: u64 = 14;
}

/// An error.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("ClickHouse exception {code} ({name}): {message}")]
    Server { code: i32, name: String, message: String },
    #[error("protocol error: {0}")]
    Protocol(String),
}

impl Error {
    /// The error code for server exceptions.
    pub fn code(&self) -> Option<i32> {
        match self {
            Error::Server { code, .. } => Some(*code),
            _ => None,
        }
    }
}

/// Connection options.
#[derive(Clone)]
pub struct Options {
    pub address: String,
    pub database: String,
    pub username: String,
    pub password: String,
    pub compression: bool,
    pub dial_timeout: Duration,
    /// Timeout for reads and writes.
    pub io_timeout: Duration,
    /// TLS configuration, when TLS is enabled.
    pub tls: Option<Arc<rustls::ClientConfig>>,
    pub client_name: String,
}

impl std::fmt::Debug for Options {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Options")
            .field("address", &self.address)
            .field("database", &self.database)
            .field("username", &self.username)
            .field("tls", &self.tls.is_some())
            .finish_non_exhaustive()
    }
}

impl Default for Options {
    fn default() -> Self {
        Options {
            address: "127.0.0.1:9000".into(),
            database: "default".into(),
            username: "default".into(),
            password: String::new(),
            compression: true,
            dial_timeout: Duration::from_secs(5),
            io_timeout: Duration::from_secs(300),
            tls: None,
            client_name: "flowscope".into(),
        }
    }
}

/// A query setting.
#[derive(Clone, Debug)]
pub struct Setting {
    pub name: String,
    pub value: String,
    pub important: bool,
}

impl Setting {
    pub fn new(name: &str, value: impl ToString) -> Self {
        Setting { name: name.into(), value: value.to_string(), important: false }
    }

    pub fn important(name: &str, value: impl ToString) -> Self {
        Setting { name: name.into(), value: value.to_string(), important: true }
    }
}

/// Information about the server.
#[derive(Clone, Debug, Default)]
pub struct ServerInfo {
    pub name: String,
    pub version: (u64, u64, u64),
    pub revision: u64,
    pub timezone: String,
    pub display_name: String,
}

enum Stream {
    Tcp(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Stream::Tcp(s) => s.read(buf),
            Stream::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Stream::Tcp(s) => s.write(buf),
            Stream::Tls(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Stream::Tcp(s) => s.flush(),
            Stream::Tls(s) => s.flush(),
        }
    }
}

/// Result of a query.
#[derive(Clone, Debug, Default)]
pub struct QueryResult {
    /// Data blocks (empty blocks are skipped).
    pub blocks: Vec<Block>,
    /// Rows read by the query.
    pub rows_read: u64,
    pub bytes_read: u64,
}

impl QueryResult {
    /// Total number of rows.
    pub fn rows(&self) -> usize {
        self.blocks.iter().map(|b| b.rows).sum()
    }

    /// Iterate over rows as values.
    pub fn iter_rows(&self) -> impl Iterator<Item = Vec<crate::Value>> + '_ {
        self.blocks.iter().flat_map(|b| (0..b.rows).map(move |i| b.row(i)))
    }

    /// Column names.
    pub fn column_names(&self) -> Vec<String> {
        self.blocks.first().map(|b| b.columns.iter().map(|c| c.name.clone()).collect()).unwrap_or_default()
    }
}

/// A connection.
pub struct Connection {
    stream: BufReader<Stream>,
    options: Options,
    revision: u64,
    server: ServerInfo,
    broken: bool,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection").field("address", &self.options.address).finish_non_exhaustive()
    }
}

/// Read an exception packet (and nested ones).
fn read_exception(r: &mut dyn Read) -> Result<Error, io::Error> {
    let code = read_i32(r)?;
    let name = read_string(r)?;
    let message = read_string(r)?;
    let _stack = read_string(r)?;
    let nested = read_u8(r)? != 0;
    if nested {
        // Keep the outer one, it is the most relevant.
        let _ = read_exception(r)?;
    }
    Ok(Error::Server { code, name, message })
}

impl Connection {
    /// Connect and authenticate.
    pub fn connect(options: &Options) -> Result<Connection, Error> {
        let addr = options
            .address
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| Error::Protocol(format!("cannot resolve {}", options.address)))?;
        let tcp = TcpStream::connect_timeout(&addr, options.dial_timeout)?;
        tcp.set_nodelay(true)?;
        tcp.set_read_timeout(Some(options.io_timeout))?;
        tcp.set_write_timeout(Some(options.io_timeout))?;
        let stream = match &options.tls {
            None => Stream::Tcp(tcp),
            Some(config) => {
                let host = options.address.rsplit_once(':').map(|(h, _)| h).unwrap_or(&options.address);
                let host = host.trim_matches(['[', ']']).to_string();
                let name = rustls::pki_types::ServerName::try_from(host)
                    .map_err(|e| Error::Protocol(format!("invalid server name: {e}")))?;
                let conn = rustls::ClientConnection::new(config.clone(), name)
                    .map_err(|e| Error::Protocol(format!("TLS error: {e}")))?;
                Stream::Tls(Box::new(rustls::StreamOwned::new(conn, tcp)))
            }
        };
        let mut c = Connection {
            stream: BufReader::with_capacity(64 * 1024, stream),
            options: options.clone(),
            revision: REVISION,
            server: ServerInfo::default(),
            broken: false,
        };
        c.hello()?;
        Ok(c)
    }

    fn send(&mut self, buf: &[u8]) -> Result<(), Error> {
        let s = self.stream.get_mut();
        if let Err(e) = s.write_all(buf).and_then(|_| s.flush()) {
            self.broken = true;
            return Err(e.into());
        }
        Ok(())
    }

    fn hello(&mut self) -> Result<(), Error> {
        let mut out = Vec::new();
        put_uvarint(&mut out, client_packet::HELLO);
        put_str(&mut out, self.options.client_name.as_bytes());
        put_uvarint(&mut out, 25);
        put_uvarint(&mut out, 1);
        put_uvarint(&mut out, REVISION);
        put_str(&mut out, self.options.database.as_bytes());
        put_str(&mut out, self.options.username.as_bytes());
        put_str(&mut out, self.options.password.as_bytes());
        self.send(&out)?;

        let r = &mut self.stream;
        match read_uvarint(r)? {
            server_packet::HELLO => {}
            server_packet::EXCEPTION => return Err(read_exception(r)?),
            p => return Err(Error::Protocol(format!("unexpected packet {p} during handshake"))),
        }
        let name = read_string(r)?;
        let major = read_uvarint(r)?;
        let minor = read_uvarint(r)?;
        let server_revision = read_uvarint(r)?;
        self.revision = REVISION.min(server_revision);
        let timezone = if self.revision >= REV_SERVER_TIMEZONE { read_string(r)? } else { String::new() };
        let display_name = if self.revision >= REV_SERVER_DISPLAY_NAME { read_string(r)? } else { String::new() };
        let patch = if self.revision >= REV_VERSION_PATCH { read_uvarint(r)? } else { server_revision };
        self.server =
            ServerInfo { name, version: (major, minor, patch), revision: server_revision, timezone, display_name };
        if self.revision >= REV_ADDENDUM {
            // Quota key.
            let mut out = Vec::new();
            put_str(&mut out, b"");
            self.send(&out)?;
        }
        Ok(())
    }

    /// Information about the server.
    pub fn server(&self) -> &ServerInfo {
        &self.server
    }

    /// Whether the connection is unusable.
    pub fn is_broken(&self) -> bool {
        self.broken
    }

    /// Send a ping and wait for the answer.
    pub fn ping(&mut self) -> Result<(), Error> {
        let mut out = Vec::new();
        put_uvarint(&mut out, client_packet::PING);
        self.send(&out)?;
        loop {
            match self.read_packet_type()? {
                server_packet::PONG => return Ok(()),
                server_packet::PROGRESS => self.read_progress()?,
                p => {
                    self.broken = true;
                    return Err(Error::Protocol(format!("unexpected packet {p} after ping")));
                }
            }
        }
    }

    fn read_packet_type(&mut self) -> Result<u64, Error> {
        read_uvarint(&mut self.stream).map_err(|e| {
            self.broken = true;
            e.into()
        })
    }

    fn write_query(&mut self, query: &str, settings: &[Setting]) -> Result<(), Error> {
        let rev = self.revision;
        let mut out = Vec::new();
        put_uvarint(&mut out, client_packet::QUERY);
        put_str(&mut out, b""); // query ID
        if rev >= REV_CLIENT_INFO {
            out.push(1); // initial query
            put_str(&mut out, b""); // initial user
            put_str(&mut out, b""); // initial query ID
            put_str(&mut out, b"[::ffff:127.0.0.1]:0"); // initial address
            if rev >= REV_INITIAL_QUERY_START_TIME {
                out.extend_from_slice(&0i64.to_le_bytes());
            }
            out.push(1); // TCP interface
            put_str(&mut out, b""); // OS user
            put_str(&mut out, hostname().as_bytes());
            put_str(&mut out, self.options.client_name.as_bytes());
            put_uvarint(&mut out, 25);
            put_uvarint(&mut out, 1);
            put_uvarint(&mut out, REVISION);
            if rev >= REV_QUOTA_KEY_IN_CLIENT_INFO {
                put_str(&mut out, b"");
            }
            if rev >= REV_DISTRIBUTED_DEPTH {
                put_uvarint(&mut out, 0);
            }
            if rev >= REV_VERSION_PATCH {
                put_uvarint(&mut out, 0);
            }
            if rev >= REV_OPENTELEMETRY {
                out.push(0);
            }
            if rev >= REV_PARALLEL_REPLICAS {
                put_uvarint(&mut out, 0);
                put_uvarint(&mut out, 0);
                put_uvarint(&mut out, 0);
            }
        }
        if rev < REV_SETTINGS_AS_STRINGS && !settings.is_empty() {
            return Err(Error::Protocol("server too old for settings".into()));
        }
        for s in settings {
            put_str(&mut out, s.name.as_bytes());
            put_uvarint(&mut out, s.important as u64);
            put_str(&mut out, s.value.as_bytes());
        }
        put_str(&mut out, b""); // end of settings
        if rev >= REV_INTERSERVER_SECRET {
            put_str(&mut out, b"");
        }
        put_uvarint(&mut out, 2); // stage: complete
        put_uvarint(&mut out, self.options.compression as u64);
        put_str(&mut out, query.as_bytes());
        if rev >= REV_PARAMETERS {
            put_str(&mut out, b""); // end of parameters
        }
        // End of external tables: an empty data block.
        self.encode_data(&mut out, |b| write_block_header(b, 0, 0));
        self.send(&out)
    }

    /// Append a data packet whose block is produced by `f`.
    fn encode_data(&self, out: &mut Vec<u8>, f: impl FnOnce(&mut Vec<u8>)) {
        put_uvarint(out, client_packet::DATA);
        put_str(out, b"");
        if self.options.compression {
            let mut raw = Vec::new();
            f(&mut raw);
            compress(&raw, out);
        } else {
            f(out);
        }
    }

    fn with_custom(&self) -> bool {
        self.revision >= REV_CUSTOM_SERIALIZATION
    }

    fn read_data_block(&mut self, compressed: bool) -> Result<Block, Error> {
        let with_custom = self.with_custom();
        let r = &mut self.stream;
        let _table = read_string(r)?;
        let block = if compressed {
            let mut d = Decompressor::new(r);
            read_block(&mut d, with_custom)?
        } else {
            read_block(r, with_custom)?
        };
        Ok(block)
    }

    fn read_progress(&mut self) -> Result<(), Error> {
        let rev = self.revision;
        let r = &mut self.stream;
        read_uvarint(r)?; // rows
        read_uvarint(r)?; // bytes
        read_uvarint(r)?; // total rows
        if rev >= REV_TOTAL_BYTES_IN_PROGRESS {
            read_uvarint(r)?;
        }
        if rev >= REV_CLIENT_WRITE_INFO {
            read_uvarint(r)?;
            read_uvarint(r)?;
        }
        if rev >= REV_SERVER_QUERY_TIME_IN_PROGRESS {
            read_uvarint(r)?;
        }
        Ok(())
    }

    /// Read server packets until the end of the stream. Data blocks are
    /// given to `on_data`. When `stop_at_data` is set, stop at the first data
    /// block (the header of an INSERT).
    fn read_until_end(
        &mut self,
        mut on_data: impl FnMut(Block),
        stop_at_data: bool,
        result: &mut QueryResult,
    ) -> Result<(), Error> {
        loop {
            let packet = self.read_packet_type()?;
            let res: Result<(), Error> = (|| {
                match packet {
                    server_packet::DATA | server_packet::TOTALS | server_packet::EXTREMES => {
                        let block = self.read_data_block(self.options.compression)?;
                        if packet == server_packet::DATA {
                            on_data(block);
                            if stop_at_data {
                                return Err(Error::Protocol("stop".into()));
                            }
                        }
                    }
                    server_packet::EXCEPTION => return Err(read_exception(&mut self.stream)?),
                    server_packet::PROGRESS => self.read_progress()?,
                    server_packet::END_OF_STREAM => return Err(Error::Protocol("end".into())),
                    server_packet::PROFILE_INFO => {
                        let r = &mut self.stream;
                        let rows = read_uvarint(r)?;
                        read_uvarint(r)?; // blocks
                        let bytes = read_uvarint(r)?;
                        read_u8(r)?; // applied limit
                        read_uvarint(r)?; // rows before limit
                        read_u8(r)?; // calculated rows before limit
                        result.rows_read = rows;
                        result.bytes_read = bytes;
                    }
                    // Logs and profile events are never compressed.
                    server_packet::LOG | server_packet::PROFILE_EVENTS => {
                        self.read_data_block(false)?;
                    }
                    server_packet::TABLE_COLUMNS => {
                        let r = &mut self.stream;
                        read_string(r)?;
                        read_string(r)?;
                    }
                    server_packet::PART_UUIDS => {
                        let r = &mut self.stream;
                        let n = read_uvarint(r)?;
                        for _ in 0..n {
                            read_fixed::<16>(r)?;
                        }
                    }
                    p => return Err(Error::Protocol(format!("unexpected packet {p}"))),
                }
                Ok(())
            })();
            match res {
                Ok(()) => {}
                Err(Error::Protocol(s)) if s == "end" || s == "stop" => return Ok(()),
                Err(e @ Error::Server { .. }) => return Err(e),
                Err(e) => {
                    self.broken = true;
                    return Err(e);
                }
            }
        }
    }

    /// Run a query and collect the result.
    pub fn query(&mut self, query: &str, settings: &[Setting]) -> Result<QueryResult, Error> {
        self.write_query(query, settings)?;
        let mut result = QueryResult::default();
        let mut blocks = Vec::new();
        self.read_until_end(
            |b| {
                if b.rows > 0 || blocks.is_empty() {
                    blocks.push(b)
                }
            },
            false,
            &mut result,
        )?;
        // Keep the header block only when there is no data.
        if blocks.len() > 1 && blocks[0].rows == 0 {
            blocks.remove(0);
        }
        result.blocks = blocks;
        Ok(result)
    }

    /// Run a statement without result.
    pub fn execute(&mut self, query: &str, settings: &[Setting]) -> Result<(), Error> {
        self.query(query, settings).map(|_| ())
    }

    /// Insert pre-encoded columns. `query` is an `INSERT INTO ... VALUES`
    /// statement. `encode` writes the columns (header and data) and gets
    /// whether the custom serialization flag is expected.
    pub fn insert_raw(
        &mut self,
        query: &str,
        settings: &[Setting],
        ncols: usize,
        rows: usize,
        encode: impl FnOnce(&mut Vec<u8>, bool),
    ) -> Result<Block, Error> {
        self.write_query(query, settings)?;
        let mut header = None;
        let mut result = QueryResult::default();
        self.read_until_end(|b| header = Some(b), true, &mut result)?;
        let header = header.ok_or_else(|| Error::Protocol("no header for INSERT".into()))?;
        if header.columns.len() != ncols {
            // Let the server report the problem.
            tracing::debug!(expected = ncols, got = header.columns.len(), "INSERT header mismatch");
        }
        let with_custom = self.with_custom();
        let mut out = Vec::new();
        self.encode_data(&mut out, |b| {
            write_block_header(b, ncols, rows);
            encode(b, with_custom);
        });
        self.encode_data(&mut out, |b| write_block_header(b, 0, 0));
        self.send(&out)?;
        self.read_until_end(|_| {}, false, &mut result)?;
        Ok(header)
    }

    /// Insert a block.
    pub fn insert(&mut self, query: &str, settings: &[Setting], block: &Block) -> Result<(), Error> {
        let mut encoded = Vec::new();
        let with_custom = self.with_custom();
        write_block(&mut encoded, block, with_custom).map_err(Error::Protocol)?;
        // Skip the block header written by write_block, insert_raw adds one.
        let mut header = Vec::new();
        write_block_header(&mut header, block.columns.len(), block.rows);
        let body = encoded[header.len()..].to_vec();
        self.insert_raw(query, settings, block.columns.len(), block.rows, |out, _| out.extend_from_slice(&body))
            .map(|_| ())
    }
}

fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname").map(|s| s.trim().to_string()).unwrap_or_default()
}
