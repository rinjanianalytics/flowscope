// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! File input: replay files as received packets, in a loop. For tests and
//! demos. Port of `inlet/flow/input/file`.

use crate::SendFn;
use crate::config::FileConfiguration;
use flowscope_pb::RawFlow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

/// A running file input.
pub struct FileInput {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<Result<(), String>>,
}

impl FileInput {
    pub fn start(config: &FileConfiguration, send: SendFn) -> Result<FileInput, String> {
        if config.paths.is_empty() {
            return Err("no paths provided for file input".into());
        }
        tracing::info!("file input starting");
        let stop = Arc::new(AtomicBool::new(false));
        let paths = config.paths.clone();
        let max_flows = config.max_flows;
        let stopping = stop.clone();
        let thread = std::thread::Builder::new()
            .name("file-input".into())
            .spawn(move || {
                let source = std::net::Ipv4Addr::LOCALHOST.to_ipv6_mapped().octets().to_vec();
                let mut flow = RawFlow::default();
                for (count, idx) in (0u64..).zip(0usize..) {
                    if stopping.load(Ordering::Relaxed) {
                        break;
                    }
                    if max_flows > 0 && count >= max_flows {
                        // Wait for the end, like the Go version.
                        while !stopping.load(Ordering::Relaxed) {
                            std::thread::sleep(std::time::Duration::from_millis(100));
                        }
                        break;
                    }
                    let path = &paths[idx % paths.len()];
                    let data = std::fs::read(path).map_err(|e| {
                        tracing::error!(error = %e, path, "unable to read path");
                        format!("unable to read {path}: {e}")
                    })?;
                    flow.time_received = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                    flow.payload = data;
                    flow.source_address = source.clone();
                    send("127.0.0.1", &mut flow);
                }
                Ok(())
            })
            .map_err(|e| format!("cannot start file input: {e}"))?;
        Ok(FileInput { stop, thread })
    }

    /// Whether the input stopped by itself (on error).
    pub fn is_finished(&self) -> bool {
        self.thread.is_finished()
    }

    pub fn stop(self) -> Result<(), String> {
        self.stop.store(true, Ordering::Relaxed);
        let result = self.thread.join().unwrap_or_else(|_| Err("file input panicked".into()));
        tracing::info!("file input stopped");
        result
    }
}
