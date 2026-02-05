//! File watcher for auth configuration hot-reload.
//!
//! This module provides an [`AuthWatcher`] that monitors `auth.toml` for changes
//! and automatically reloads the [`AuthProvider`]'s user and API key tables.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tracing::{debug, error, info, warn};

use super::AuthProvider;
use super::config::AuthFileConfig;
use super::crypto::{MasterKey, decrypt_auth_config};

/// Default debounce interval for filesystem change events.
const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(500);

/// Watches the auth config file for changes and triggers hot-reloads.
///
/// The watcher debounces rapid filesystem events (e.g., editor save cycles)
/// and only reloads the auth tables once the file has settled.
pub struct AuthWatcher {
    provider: Arc<AuthProvider>,
    auth_path: PathBuf,
    master_key: MasterKey,
    debounce: Duration,
}

impl AuthWatcher {
    /// Create a new auth watcher.
    ///
    /// - `provider`: shared auth provider whose tables will be reloaded on changes.
    /// - `auth_path`: path to the `auth.toml` file to watch.
    /// - `master_key`: encryption key for decrypting sensitive fields in the config.
    pub fn new(
        provider: Arc<AuthProvider>,
        auth_path: impl Into<PathBuf>,
        master_key: MasterKey,
    ) -> Self {
        Self {
            provider,
            auth_path: auth_path.into(),
            master_key,
            debounce: DEFAULT_DEBOUNCE,
        }
    }

    /// Override the default debounce duration.
    #[must_use]
    pub fn with_debounce(mut self, debounce: Duration) -> Self {
        self.debounce = debounce;
        self
    }

    /// Spawn the watcher as a background tokio task.
    ///
    /// Returns a `JoinHandle` that can be awaited or aborted to stop watching.
    /// The watcher runs until the handle is aborted or the process exits.
    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            if let Err(e) = self.run().await {
                error!(error = %e, "auth watcher exited with error");
            }
        })
    }

    /// Internal run loop: set up a `notify` watcher and react to changes.
    async fn run(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<()>(16);

        // Watch the parent directory since some editors replace the file atomically.
        let watch_dir = self
            .auth_path
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();

        let auth_filename = self
            .auth_path
            .file_name()
            .map(std::ffi::OsStr::to_os_string)
            .unwrap_or_default();

        // Build the notify watcher, forwarding relevant events into the channel.
        let _watcher = {
            let tx = tx.clone();
            let auth_filename = auth_filename.clone();
            let mut watcher = RecommendedWatcher::new(
                move |res: Result<notify::Event, notify::Error>| match res {
                    Ok(event) => {
                        if is_relevant_event(event.kind) {
                            // Check if the event is for our specific file.
                            let is_our_file = event.paths.iter().any(|p| {
                                p.file_name()
                                    .is_some_and(|name| name == auth_filename.as_os_str())
                            });
                            if is_our_file {
                                // Best-effort send; if the channel is full we
                                // already have a pending reload queued.
                                let _ = tx.try_send(());
                            }
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "filesystem watcher error");
                    }
                },
                notify::Config::default(),
            )?;
            watcher.watch(&watch_dir, RecursiveMode::NonRecursive)?;
            info!(path = %self.auth_path.display(), "auth watcher started");
            watcher
        };

        // Main loop: wait for change signals, debounce, then reload.
        loop {
            // Wait for the first change notification.
            if rx.recv().await.is_none() {
                // Channel closed -- watcher was dropped.
                debug!("auth watcher channel closed, shutting down");
                break;
            }

            // Debounce: drain any events that arrive within the debounce window.
            tokio::time::sleep(self.debounce).await;
            while rx.try_recv().is_ok() {
                // Drain queued events.
            }

            // Perform the reload.
            self.reload_auth().await;
        }

        Ok(())
    }

    /// Reload auth tables from the config file.
    async fn reload_auth(&self) {
        info!(path = %self.auth_path.display(), "reloading auth config");

        // Read and parse the config file.
        let contents = match std::fs::read_to_string(&self.auth_path) {
            Ok(c) => c,
            Err(e) => {
                error!(error = %e, "failed to read auth config file");
                return;
            }
        };

        let mut config: AuthFileConfig = match toml::from_str(&contents) {
            Ok(c) => c,
            Err(e) => {
                error!(error = %e, "failed to parse auth config");
                return;
            }
        };

        // Decrypt encrypted fields.
        if let Err(e) = decrypt_auth_config(&mut config, &self.master_key) {
            error!(error = %e, "failed to decrypt auth config");
            return;
        }

        // Reload the provider's tables.
        match self.provider.reload(&config).await {
            Ok(()) => {
                info!("auth config reloaded successfully");
            }
            Err(e) => {
                error!(error = %e, "failed to reload auth config, keeping previous state");
            }
        }
    }
}

/// Returns `true` for filesystem events that might indicate config file changes.
fn is_relevant_event(kind: EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_relevant_event_create() {
        assert!(is_relevant_event(EventKind::Create(
            notify::event::CreateKind::File
        )));
    }

    #[test]
    fn is_relevant_event_modify() {
        assert!(is_relevant_event(EventKind::Modify(
            notify::event::ModifyKind::Data(notify::event::DataChange::Content)
        )));
    }

    #[test]
    fn is_relevant_event_remove() {
        assert!(is_relevant_event(EventKind::Remove(
            notify::event::RemoveKind::File
        )));
    }

    #[test]
    fn is_relevant_event_access_is_false() {
        assert!(!is_relevant_event(EventKind::Access(
            notify::event::AccessKind::Read
        )));
    }
}
