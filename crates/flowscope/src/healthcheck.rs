// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! `healthcheck` command: query the healthcheck endpoint over the abstract
//! Unix socket (default) or over HTTP.

use std::io::{Read, Write};

#[derive(clap::Args)]
pub struct Args {
    /// HTTP host:port for health check
    #[arg(long)]
    http: Option<String>,
    /// Service to query over Unix socket
    #[arg(long)]
    service: Option<String>,
}

const REQUEST: &[u8] = b"GET /api/v0/healthcheck HTTP/1.0\r\nHost: flowscope\r\n\r\n";

fn exchange(mut stream: impl Read + Write) -> anyhow::Result<()> {
    stream.write_all(REQUEST)?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    if !response.starts_with(b"HTTP/1.") {
        anyhow::bail!("invalid HTTP response");
    }
    println!("ok");
    Ok(())
}

pub fn run(args: Args) -> anyhow::Result<()> {
    if let Some(http) = args.http {
        let addr = if http.starts_with(':') { format!("localhost{http}") } else { http };
        return exchange(std::net::TcpStream::connect(addr)?);
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::linux::net::SocketAddrExt;
        let name = match args.service {
            Some(s) => format!("flowscope/{s}"),
            None => "flowscope".into(),
        };
        let addr = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes())?;
        exchange(std::os::unix::net::UnixStream::connect_addr(&addr)?)
    }
    #[cfg(not(target_os = "linux"))]
    exchange(std::net::TcpStream::connect("localhost:8080")?)
}
