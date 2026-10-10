// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Daemon lifecycle. Port of `common/daemon`.
//!
//! Components spawn their long-running tasks through the daemon. When one of
//! them ends (with or without an error), or on SIGINT/SIGTERM, the daemon
//! terminates: every component watching `token()` shuts down.

use std::future::Future;
use std::sync::{Arc, Mutex};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

/// Lifecycle manager for a daemon.
#[derive(Clone)]
pub struct Daemon {
    token: CancellationToken,
    tasks: Arc<Mutex<JoinSet<()>>>,
    error: Arc<Mutex<Option<String>>>,
}

impl Default for Daemon {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Daemon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Daemon").field("terminated", &self.token.is_cancelled()).finish()
    }
}

impl Daemon {
    pub fn new() -> Self {
        Daemon {
            token: CancellationToken::new(),
            tasks: Arc::new(Mutex::new(JoinSet::new())),
            error: Arc::new(Mutex::new(None)),
        }
    }

    /// Token cancelled when the daemon terminates.
    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    /// Ask the daemon to terminate.
    pub fn terminate(&self) {
        self.token.cancel()
    }

    /// Wait until the daemon terminates.
    pub async fn terminated(&self) {
        self.token.cancelled().await
    }

    pub fn is_terminated(&self) -> bool {
        self.token.is_cancelled()
    }

    /// First error reported by a task, if any.
    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }

    /// Spawn a task tracked by the daemon. When it returns, the daemon
    /// terminates. An error is logged and recorded.
    pub fn spawn<F, E>(&self, who: &'static str, fut: F)
    where
        F: Future<Output = Result<(), E>> + Send + 'static,
        E: std::fmt::Display + Send + 'static,
    {
        let token = self.token.clone();
        let error = self.error.clone();
        self.tasks.lock().unwrap().spawn(async move {
            match fut.await {
                Ok(()) => tracing::debug!(component = who, "component shutting down, quitting"),
                Err(e) => {
                    tracing::error!(component = who, error = %e, "component error, quitting");
                    error.lock().unwrap().get_or_insert_with(|| format!("{who}: {e}"));
                }
            }
            token.cancel();
        });
    }

    /// Spawn a task that does not terminate the daemon when it returns. It is
    /// still awaited on shutdown.
    pub fn spawn_background<F>(&self, fut: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.tasks.lock().unwrap().spawn(fut);
    }

    /// Terminate on SIGINT or SIGTERM.
    pub fn watch_signals(&self) {
        let token = self.token.clone();
        tokio::spawn(async move {
            #[cfg(unix)]
            {
                use tokio::signal::unix::{SignalKind, signal};
                let mut term = signal(SignalKind::terminate()).expect("cannot install SIGTERM handler");
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => tracing::info!("quitting"),
                    _ = term.recv() => tracing::info!("quitting"),
                    _ = token.cancelled() => return,
                }
            }
            #[cfg(not(unix))]
            tokio::select! {
                _ = tokio::signal::ctrl_c() => tracing::info!("quitting"),
                _ = token.cancelled() => return,
            }
            token.cancel();
        });
    }

    /// Wait for all tracked tasks to finish. Call after `terminated()`.
    pub async fn join(&self) {
        let mut set = std::mem::take(&mut *self.tasks.lock().unwrap());
        while set.join_next().await.is_some() {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn task_end_terminates() {
        let d = Daemon::new();
        let token = d.token();
        d.spawn("worker", async move {
            token.cancelled().await;
            Ok::<_, String>(())
        });
        d.spawn("failing", async { Err::<(), _>("boom") });
        tokio::time::timeout(Duration::from_secs(1), d.terminated()).await.unwrap();
        d.join().await;
        assert_eq!(d.error().as_deref(), Some("failing: boom"));
    }
}
