//! Checksummed immutable ledger segments. Blocking disk operations run off the async executor.
use crate::{ChangeDisposition, Error, Result, TimelineChange, TimelineScope, store::Projection};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub id: String,
    pub scope: TimelineScope,
    pub received_at: DateTime<Utc>,
    pub retain_until: DateTime<Utc>,
    pub archive_until: DateTime<Utc>,
    pub change: TimelineChange,
    pub raw: serde_json::Value,
    pub transport: Option<serde_json::Value>,
    pub disposition: ChangeDisposition,
    pub projection: Option<Projection>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerStatus {
    pub records: u64,
    pub bytes: u64,
    pub pending: u64,
    pub oldest_received_at: Option<DateTime<Utc>>,
}
#[async_trait]
pub trait ChangeLedger: Send + Sync {
    async fn append(&self, entry: &LedgerEntry) -> Result<()>;
    async fn complete(&self, scope: &TimelineScope, id: &str) -> Result<()>;
    async fn read(
        &self,
        scope: &TimelineScope,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<LedgerEntry>>;
    async fn status(&self, scope: &TimelineScope) -> Result<LedgerStatus>;
    async fn cleanup(&self, now: DateTime<Utc>) -> Result<u64>;
}
#[async_trait]
pub trait LedgerArchiveSink: Send + Sync {
    /// Must publish remotely before returning. Called again on uncertain outcomes.
    async fn publish(&self, entry: &LedgerEntry) -> Result<()>;
    async fn cleanup(&self, _now: DateTime<Utc>) -> Result<u64> {
        Ok(0)
    }
}

pub struct DiskLedger {
    root: PathBuf,
    max_bytes: u64,
    lock: Arc<Mutex<()>>,
    used: Arc<AtomicU64>,
    _directory_lock: File,
}
impl DiskLedger {
    pub fn open(root: impl Into<PathBuf>, max_bytes: u64) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        if max_bytes == 0 {
            return Err(Error::Invalid("ledger max_bytes must be positive".into()));
        }
        let directory_lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join(".lock"))?;
        fs2::FileExt::try_lock_exclusive(&directory_lock).map_err(|_| {
            Error::Storage("ledger directory is already owned by another process".into())
        })?;
        let mut used = 0;
        for scope in fs::read_dir(&root)? {
            let scope = scope?.path();
            if scope.is_dir() {
                for p in files(&scope)? {
                    used += p.metadata()?.len();
                }
                for entry in fs::read_dir(&scope)? {
                    let path = entry?.path();
                    if path.extension().is_some_and(|extension| extension == "tmp") {
                        fs::remove_file(path)?;
                    }
                }
            }
        }
        Ok(Self {
            root,
            max_bytes,
            lock: Arc::new(Mutex::new(())),
            used: Arc::new(AtomicU64::new(used)),
            _directory_lock: directory_lock,
        })
    }
    fn scope_dir(&self, scope: &TimelineScope) -> PathBuf {
        self.root.join(crate::digest(scope))
    }
}
fn files(root: &Path) -> Result<Vec<PathBuf>> {
    if !root.exists() {
        return Ok(vec![]);
    }
    let mut result = vec![];
    for e in fs::read_dir(root)? {
        let p = e?.path();
        if p.extension().is_some_and(|e| e == "cdc") {
            result.push(p);
        }
    }
    result.sort();
    Ok(result)
}
fn read_entry(path: &Path) -> Result<LedgerEntry> {
    let mut f = File::open(path)?;
    let mut header = [0u8; 40];
    f.read_exact(&mut header)?;
    if &header[..4] != b"CDC1" {
        return Err(Error::Storage("invalid ledger segment header".into()));
    }
    let length = u32::from_le_bytes(header[4..8].try_into().expect("four bytes")) as usize;
    if length > 2 * 1024 * 1024 || f.metadata()?.len() != (40 + length) as u64 {
        return Err(Error::Storage("invalid ledger segment length".into()));
    }
    let mut body = vec![0; length];
    f.read_exact(&mut body)?;
    if Sha256::digest(&body)[..] != header[8..] {
        return Err(Error::Storage("ledger checksum mismatch".into()));
    }
    Ok(serde_json::from_slice(&body)?)
}
fn atomic_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    let parent = path.parent().expect("ledger parent");
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!("{}.tmp", uuid::Uuid::new_v4()));
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    let result = fs::hard_link(&temp, path);
    fs::remove_file(&temp)?;
    match result {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    File::open(parent)?.sync_all()?;
    Ok(())
}
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| Error::Storage(e.to_string()))?
}
#[async_trait]
impl ChangeLedger for DiskLedger {
    async fn append(&self, entry: &LedgerEntry) -> Result<()> {
        let _guard = self.lock.lock().await;
        let root = self.root.clone();
        let dir = self.scope_dir(&entry.scope);
        let max = self.max_bytes;
        let entry = entry.clone();
        let usage = Arc::clone(&self.used);
        blocking(move || {
            let path = dir.join(format!("{}.cdc", crate::digest(&entry.id)));
            if path.exists() {
                if crate::digest(&read_entry(&path)?) != crate::digest(&entry) {
                    return Err(Error::Conflict);
                }
                return Ok(());
            }
            let body = serde_json::to_vec(&entry)?;
            if body.len() > 2 * 1024 * 1024 {
                return Err(Error::Invalid("ledger record exceeds 2 MiB".into()));
            }
            let used = usage.load(Ordering::Relaxed);
            if used.saturating_add(body.len() as u64 + 40) > max {
                return Err(Error::Capacity);
            }
            let mut bytes = b"CDC1".to_vec();
            bytes.extend(
                u32::try_from(body.len())
                    .map_err(|_| Error::Capacity)?
                    .to_le_bytes(),
            );
            bytes.extend(Sha256::digest(&body));
            bytes.extend(body);
            atomic_file(&path, &bytes)?;
            usage.fetch_add(bytes.len() as u64, Ordering::Relaxed);
            File::open(&root)?.sync_all()?;
            Ok(())
        })
        .await
    }
    async fn complete(&self, scope: &TimelineScope, id: &str) -> Result<()> {
        let path = self
            .scope_dir(scope)
            .join(format!("{}.done", crate::digest(&id)));
        blocking(move || atomic_file(&path, b"applied\n")).await
    }
    async fn read(
        &self,
        scope: &TimelineScope,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<LedgerEntry>> {
        let dir = self.scope_dir(scope);
        let after = after.map(str::to_owned);
        blocking(move || {
            files(&dir)?
                .into_iter()
                .filter(|p| {
                    after.as_ref().is_none_or(|a| {
                        p.file_stem()
                            .is_some_and(|s| s.to_string_lossy().as_ref() > a.as_str())
                    })
                })
                .take(limit)
                .map(|p| read_entry(&p))
                .collect()
        })
        .await
    }
    async fn status(&self, scope: &TimelineScope) -> Result<LedgerStatus> {
        let dir = self.scope_dir(scope);
        blocking(move || {
            let mut status = LedgerStatus::default();
            for p in files(&dir)? {
                let entry = read_entry(&p)?;
                status.records += 1;
                status.bytes += p.metadata()?.len();
                if !p.with_extension("done").exists() {
                    status.pending += 1;
                }
                status.oldest_received_at = Some(
                    status
                        .oldest_received_at
                        .map_or(entry.received_at, |t| t.min(entry.received_at)),
                );
            }
            Ok(status)
        })
        .await
    }
    async fn cleanup(&self, now: DateTime<Utc>) -> Result<u64> {
        let _guard = self.lock.lock().await;
        let root = self.root.clone();
        let usage = Arc::clone(&self.used);
        blocking(move || {
            let mut count = 0;
            for scope in fs::read_dir(&root)? {
                let scope = scope?.path();
                if !scope.is_dir() {
                    continue;
                }
                for p in files(&scope)? {
                    if p.with_extension("done").exists() && read_entry(&p)?.retain_until <= now {
                        let size = p.metadata()?.len();
                        fs::remove_file(&p)?;
                        usage.fetch_sub(size, Ordering::Relaxed);
                        fs::remove_file(p.with_extension("done"))?;
                        count += 1;
                    }
                }
                File::open(scope)?.sync_all()?;
            }
            Ok(count)
        })
        .await
    }
}
