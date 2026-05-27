//! Generic file / directory watcher with debounced reload callbacks.
//!
//! Several subsystems (auth, static quotas, rules, templates) need the
//! same pattern: watch a path with the `notify` crate, debounce
//! editor save bursts, and run an async reload routine when the
//! coalesced batch settles. This module owns that machinery so each
//! subsystem just provides a path and a closure.
//!
//! See also [`crate::auth::watcher`] (the original implementation
//! kept in-place because it carries auth-specific decrypt logic) and
//! [`crate::quotas_loader::QuotaWatcher`] (which still owns the
//! manual-nudge channel for HTTP coalescing).

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::Notify;
use tracing::{debug, error, info, warn};

/// Default debounce window for editor save bursts. Mirrors the auth
/// and quotas watchers — 500ms is enough to absorb the 3-5 events
/// vim/vscode typically emit during a single save.
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(500);

/// Whether to watch a single file or recurse into a directory tree.
#[derive(Debug, Clone, Copy)]
pub enum WatchMode {
    /// Watch the immediate parent directory non-recursively, then
    /// filter events by exact filename match. This catches both
    /// direct writes and atomic-replace editor saves where the inode
    /// changes — watching the file itself would miss the rename
    /// flavour.
    SingleFile,
    /// Recursively watch the given directory. Used for sources that
    /// span multiple files (e.g. a rules directory with many YAML
    /// files in subfolders).
    Directory,
}

/// Spawn a tokio task that watches `path` and invokes `reload` when
/// debounced changes arrive. Reload also fires whenever `nudge`
/// is notified, so an HTTP reload endpoint can coalesce with the
/// file-event stream.
///
/// The returned `JoinHandle` keeps the watcher alive for the
/// caller's lifetime; aborting it stops the watcher cleanly.
///
/// `reload` is called with no arguments; capture whatever state the
/// reload needs by move. It should be idempotent — the same file
/// contents may produce multiple reload invocations during rapid
/// editor saves even with debouncing.
pub fn spawn_watcher<F, Fut>(
    path: impl Into<PathBuf>,
    mode: WatchMode,
    debounce: Duration,
    nudge: Option<Arc<Notify>>,
    reload: F,
) -> tokio::task::JoinHandle<()>
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let path = path.into();
    tokio::spawn(async move {
        if let Err(e) = run_loop(path, mode, debounce, nudge, reload).await {
            error!(error = %e, "file watcher exited with error");
        }
    })
}

async fn run_loop<F, Fut>(
    path: PathBuf,
    mode: WatchMode,
    debounce: Duration,
    nudge: Option<Arc<Notify>>,
    reload: F,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let (tx, mut rx) = tokio::sync::mpsc::channel::<()>(16);

    let (watch_target, filename_filter) = match mode {
        WatchMode::SingleFile => {
            let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
            let name = path
                .file_name()
                .map(std::ffi::OsStr::to_os_string)
                .unwrap_or_default();
            (dir, Some(name))
        }
        WatchMode::Directory => (path.clone(), None),
    };

    let recurse = match mode {
        WatchMode::SingleFile => RecursiveMode::NonRecursive,
        WatchMode::Directory => RecursiveMode::Recursive,
    };

    let _watcher = {
        let tx = tx.clone();
        let filename_filter = filename_filter.clone();
        let mut watcher = RecommendedWatcher::new(
            move |res: Result<notify::Event, notify::Error>| match res {
                Ok(event) => {
                    if !is_relevant_event(event.kind) {
                        return;
                    }
                    let matches = match &filename_filter {
                        None => true,
                        Some(name) => event
                            .paths
                            .iter()
                            .any(|p| p.file_name().is_some_and(|n| n == name.as_os_str())),
                    };
                    if matches {
                        let _ = tx.try_send(());
                    }
                }
                Err(e) => warn!(error = %e, "filesystem watcher error"),
            },
            notify::Config::default(),
        )?;
        watcher.watch(&watch_target, recurse)?;
        info!(path = %path.display(), mode = ?mode, "file watcher started");
        watcher
    };

    let dummy_nudge = Arc::new(Notify::new());
    let nudge_handle = nudge.unwrap_or(dummy_nudge);

    loop {
        tokio::select! {
            file_event = rx.recv() => {
                if file_event.is_none() {
                    debug!("file watcher channel closed");
                    break;
                }
            }
            () = nudge_handle.notified() => {
                debug!("file watcher received manual nudge");
            }
        }

        // Debounce: absorb additional events arriving during the
        // window so a single editor save produces one reload, not
        // three.
        tokio::time::sleep(debounce).await;
        while rx.try_recv().is_ok() {}

        reload().await;
    }

    Ok(())
}

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
    fn is_relevant_event_create_modify_remove() {
        assert!(is_relevant_event(EventKind::Create(
            notify::event::CreateKind::File
        )));
        assert!(is_relevant_event(EventKind::Modify(
            notify::event::ModifyKind::Data(notify::event::DataChange::Content)
        )));
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
