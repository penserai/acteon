use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use acteon_rules::RuleFrontend;

use crate::Gateway;

/// Default debounce interval for filesystem change events.
const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(500);

/// Watches a rules directory for changes and triggers gateway rule reloads.
///
/// The watcher debounces rapid filesystem events (e.g., editor save cycles)
/// and only reloads rules once the directory has settled.
pub struct RuleWatcher {
    gateway: Arc<RwLock<Gateway>>,
    rules_dir: PathBuf,
    debounce: Duration,
}

impl RuleWatcher {
    /// Create a new rule watcher.
    ///
    /// - `gateway`: shared gateway whose rules will be reloaded on changes.
    /// - `rules_dir`: directory to watch for rule file changes.
    pub fn new(gateway: Arc<RwLock<Gateway>>, rules_dir: impl Into<PathBuf>) -> Self {
        Self {
            gateway,
            rules_dir: rules_dir.into(),
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
    ///
    /// `frontends` provides the rule parsers (e.g., YAML, CEL) to use when
    /// reloading rules from the watched directory.
    pub fn spawn(
        self,
        frontends: Vec<Arc<dyn RuleFrontend + Send + Sync>>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            if let Err(e) = self.run(frontends).await {
                error!(error = %e, "rule watcher exited with error");
            }
        })
    }

    /// Internal run loop: set up a `notify` watcher and react to changes.
    async fn run(
        &self,
        frontends: Vec<Arc<dyn RuleFrontend + Send + Sync>>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<()>(16);

        // Build the notify watcher, forwarding relevant events into the channel.
        let _watcher = {
            let tx = tx.clone();
            let mut watcher = RecommendedWatcher::new(
                move |res: Result<notify::Event, notify::Error>| match res {
                    Ok(event) => {
                        if is_relevant_event(event.kind) {
                            // Best-effort send; if the channel is full we
                            // already have a pending reload queued.
                            let _ = tx.try_send(());
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "filesystem watcher error");
                    }
                },
                notify::Config::default(),
            )?;
            watcher.watch(self.rules_dir.as_ref(), RecursiveMode::Recursive)?;
            info!(dir = %self.rules_dir.display(), "rule watcher started");
            watcher
        };

        // Main loop: wait for change signals, debounce, then reload.
        loop {
            // Wait for the first change notification.
            if rx.recv().await.is_none() {
                // Channel closed -- watcher was dropped.
                debug!("rule watcher channel closed, shutting down");
                break;
            }

            // Debounce: drain any events that arrive within the debounce window.
            tokio::time::sleep(self.debounce).await;
            while rx.try_recv().is_ok() {
                // Drain queued events.
            }

            // Perform the reload.
            reload_rules(&self.gateway, &self.rules_dir, &frontends).await;
        }

        Ok(())
    }
}

/// Returns `true` for filesystem events that might indicate rule file changes.
fn is_relevant_event(kind: EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

/// Reload rules from the given directory into the gateway.
async fn reload_rules(
    gateway: &Arc<RwLock<Gateway>>,
    rules_dir: &Path,
    frontends: &[Arc<dyn RuleFrontend + Send + Sync>],
) {
    info!(dir = %rules_dir.display(), "reloading rules");

    // Build the trait-object slice that `load_rules_from_directory` expects.
    let fe_refs: Vec<&dyn RuleFrontend> = frontends
        .iter()
        .map(|f| f.as_ref() as &dyn RuleFrontend)
        .collect();

    let mut gw = gateway.write().await;
    match gw.load_rules_from_directory(rules_dir, &fe_refs) {
        Ok(count) => {
            info!(count, "rules reloaded successfully");
        }
        Err(e) => {
            error!(error = %e, "failed to reload rules, keeping previous rule set");
        }
    }
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
