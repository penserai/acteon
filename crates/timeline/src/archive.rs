//! Immutable Parquet objects and committed manifests in S3-compatible storage.
use crate::{
    Error, Result,
    config::ArchiveConfig,
    ledger::{LedgerArchiveSink, LedgerEntry},
};
use async_trait::async_trait;
use object_store::{ObjectStore, aws::AmazonS3Builder, path::Path};
use parquet::{
    basic::{Compression, ZstdLevel},
    data_type::ByteArray,
    file::{properties::WriterProperties, writer::SerializedFileWriter},
    schema::parser::parse_message_type,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;

type PublishResult = std::result::Result<(), String>;
struct PublishRequest {
    entry: LedgerEntry,
    done: tokio::sync::oneshot::Sender<PublishResult>,
}
pub struct ParquetArchive {
    store: Arc<dyn ObjectStore>,
    prefix: String,
    sender: tokio::sync::mpsc::Sender<PublishRequest>,
}
impl ParquetArchive {
    pub fn new(config: &ArchiveConfig) -> Result<Self> {
        let mut builder = AmazonS3Builder::from_env().with_bucket_name(&config.bucket);
        if let Some(region) = &config.region {
            builder = builder.with_region(region);
        }
        if let Some(endpoint) = &config.endpoint {
            builder = builder
                .with_endpoint(endpoint)
                .with_allow_http(endpoint.starts_with("http://"));
        }
        let store = builder.build().map_err(|e| Error::Storage(e.to_string()))?;
        Ok(Self::with_store(
            Arc::new(store),
            config.prefix.trim_matches('/').into(),
        ))
    }
    pub fn with_store(store: Arc<dyn ObjectStore>, prefix: String) -> Self {
        let (sender, mut receiver) = tokio::sync::mpsc::channel::<PublishRequest>(64);
        let worker_store = Arc::clone(&store);
        let worker_prefix = prefix.clone();
        tokio::spawn(async move {
            while let Some(first) = receiver.recv().await {
                let mut batch_bytes = serde_json::to_vec(&first.entry)
                    .expect("ledger entry")
                    .len();
                let mut requests = vec![first];
                let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(100);
                while requests.len() < 512 && batch_bytes < 16 * 1024 * 1024 {
                    match tokio::time::timeout_at(deadline, receiver.recv()).await {
                        Ok(Some(request)) => {
                            batch_bytes += serde_json::to_vec(&request.entry)
                                .expect("ledger entry")
                                .len();
                            requests.push(request);
                        }
                        _ => break,
                    }
                }
                let mut groups: std::collections::BTreeMap<String, Vec<PublishRequest>> =
                    std::collections::BTreeMap::new();
                for request in requests {
                    let e = &request.entry;
                    let partition = format!(
                        "{}/namespace={}/tenant={}/date={}/hour={}",
                        worker_prefix,
                        crate::digest(&e.scope.namespace),
                        crate::digest(&e.scope.tenant),
                        e.received_at.format("%Y-%m-%d"),
                        e.received_at.format("%H")
                    );
                    groups.entry(partition).or_default().push(request);
                }
                for (partition, requests) in groups {
                    let entries: Vec<_> = requests.iter().map(|r| r.entry.clone()).collect();
                    let result = publish_batch(worker_store.as_ref(), &partition, entries)
                        .await
                        .map_err(|e| e.to_string());
                    for request in requests {
                        let _ = request.done.send(result.clone());
                    }
                }
            }
        });
        Self {
            store,
            prefix,
            sender,
        }
    }
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct ArchiveObject {
    path: String,
    sha256: String,
    rows: usize,
    uncompressed_bytes: usize,
    expires_at: chrono::DateTime<chrono::Utc>,
}
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Manifest {
    schema_version: u16,
    objects: Vec<ArchiveObject>,
}
async fn load_manifest(
    store: &dyn ObjectStore,
    path: &Path,
) -> Result<(Manifest, object_store::PutMode)> {
    match store.get(path).await {
        Ok(result) => {
            let version = object_store::UpdateVersion {
                e_tag: result.meta.e_tag.clone(),
                version: result.meta.version.clone(),
            };
            let bytes = result
                .bytes()
                .await
                .map_err(|e| Error::Storage(e.to_string()))?;
            Ok((
                serde_json::from_slice(&bytes)?,
                object_store::PutMode::Update(version),
            ))
        }
        Err(object_store::Error::NotFound { .. }) => Ok((
            Manifest {
                schema_version: 1,
                objects: vec![],
            },
            object_store::PutMode::Create,
        )),
        Err(e) => Err(Error::Storage(e.to_string())),
    }
}
async fn save_manifest(
    store: &dyn ObjectStore,
    path: &Path,
    manifest: &Manifest,
    mode: object_store::PutMode,
) -> Result<bool> {
    let options = object_store::PutOptions {
        mode,
        ..Default::default()
    };
    match store
        .put_opts(path, serde_json::to_vec(manifest)?.into(), options)
        .await
    {
        Ok(_) => Ok(true),
        Err(
            object_store::Error::Precondition { .. } | object_store::Error::AlreadyExists { .. },
        ) => Ok(false),
        Err(e) => Err(Error::Storage(e.to_string())),
    }
}
async fn write_object(
    store: &dyn ObjectStore,
    partition: &str,
    entries: Vec<LedgerEntry>,
) -> Result<ArchiveObject> {
    let rows = entries.len();
    let uncompressed_bytes = serde_json::to_vec(&entries)?.len();
    let expires_at = entries
        .iter()
        .map(|e| e.archive_until)
        .max()
        .ok_or_else(|| Error::Invalid("empty archive batch".into()))?;
    let bytes = tokio::task::spawn_blocking(move || encode(&entries))
        .await
        .map_err(|e| Error::Storage(e.to_string()))??;
    let sha256 = hex::encode(Sha256::digest(&bytes));
    let path = format!("{partition}/{}.parquet", uuid::Uuid::new_v4());
    store
        .put(&Path::from(path.clone()), bytes.into())
        .await
        .map_err(|e| Error::Storage(e.to_string()))?;
    Ok(ArchiveObject {
        path,
        sha256,
        rows,
        uncompressed_bytes,
        expires_at,
    })
}
async fn publish_batch(
    store: &dyn ObjectStore,
    partition: &str,
    entries: Vec<LedgerEntry>,
) -> Result<()> {
    let object = write_object(store, partition, entries).await?;
    let path = Path::from(format!("{partition}/manifest.json"));
    for _ in 0..64 {
        let (mut manifest, mode) = load_manifest(store, &path).await?;
        if manifest.objects.iter().any(|o| o.path == object.path) {
            return Ok(());
        }
        manifest.objects.push(object.clone());
        if save_manifest(store, &path, &manifest, mode).await? {
            return Ok(());
        }
    }
    Err(Error::Capacity)
}
/// Standard Parquet primitive columns keep records queryable without custom binary decoders.
pub fn encode(entries: &[LedgerEntry]) -> Result<Vec<u8>> {
    let names = [
        "namespace",
        "tenant",
        "source",
        "event_id",
        "revision",
        "operation",
        "received_at",
        "archive_until",
        "event_group",
        "disposition",
        "transport_json",
        "before_json",
        "after_json",
        "raw_json",
        "entry_json",
    ];
    let fields = names
        .iter()
        .map(|n| format!("REQUIRED BINARY {n} (UTF8);"))
        .collect::<Vec<_>>()
        .join(" ");
    let schema = Arc::new(
        parse_message_type(&format!("message cdc {{ {fields} }}"))
            .map_err(|e| Error::Storage(e.to_string()))?,
    );
    let props = Arc::new(
        WriterProperties::builder()
            .set_compression(Compression::ZSTD(ZstdLevel::default()))
            .build(),
    );
    let mut bytes = Vec::new();
    {
        let mut writer = SerializedFileWriter::new(&mut bytes, schema, props)
            .map_err(|e| Error::Storage(e.to_string()))?;
        let mut group = writer
            .next_row_group()
            .map_err(|e| Error::Storage(e.to_string()))?;
        for i in 0..names.len() {
            let values: Vec<ByteArray> = entries
                .iter()
                .map(|e| {
                    let values = [
                        e.scope.namespace.clone(),
                        e.scope.tenant.clone(),
                        e.change.source.clone(),
                        e.change.event_id.clone(),
                        e.change.revision.to_string(),
                        serde_json::to_value(&e.change.operation)
                            .expect("operation")
                            .as_str()
                            .expect("operation name")
                            .to_owned(),
                        e.received_at.to_rfc3339(),
                        e.archive_until.to_rfc3339(),
                        e.change
                            .after
                            .as_ref()
                            .or(e.change.before.as_ref())
                            .map_or(String::new(), |b| b.event_group.clone()),
                        serde_json::to_value(e.disposition)
                            .expect("disposition")
                            .as_str()
                            .expect("disposition name")
                            .to_owned(),
                        serde_json::to_string(&e.transport).expect("transport"),
                        serde_json::to_string(&e.change.before).expect("before"),
                        serde_json::to_string(&e.change.after).expect("after"),
                        serde_json::to_string(&e.raw).expect("raw"),
                        serde_json::to_string(e).expect("entry"),
                    ];
                    ByteArray::from(values[i].as_str())
                })
                .collect();
            let mut column = group
                .next_column()
                .map_err(|e| Error::Storage(e.to_string()))?
                .ok_or_else(|| Error::Storage("missing parquet column".into()))?;
            column
                .typed::<parquet::data_type::ByteArrayType>()
                .write_batch(&values, None, None)
                .map_err(|e| Error::Storage(e.to_string()))?;
            column.close().map_err(|e| Error::Storage(e.to_string()))?;
        }
        group.close().map_err(|e| Error::Storage(e.to_string()))?;
        writer.close().map_err(|e| Error::Storage(e.to_string()))?;
    }
    Ok(bytes)
}
#[async_trait]
impl LedgerArchiveSink for ParquetArchive {
    async fn publish(&self, entry: &LedgerEntry) -> Result<()> {
        let (done, result) = tokio::sync::oneshot::channel();
        self.sender
            .send(PublishRequest {
                entry: entry.clone(),
                done,
            })
            .await
            .map_err(|_| Error::Storage("archive worker stopped".into()))?;
        result
            .await
            .map_err(|_| Error::Storage("archive worker stopped".into()))?
            .map_err(Error::Storage)
    }
    #[allow(clippy::too_many_lines)]
    async fn cleanup(&self, now: chrono::DateTime<chrono::Utc>) -> Result<u64> {
        use futures::TryStreamExt;
        let prefix = Path::from(self.prefix.clone());
        let mut listing = self.store.list(Some(&prefix));
        let mut removed = 0;
        while let Some(meta) = listing
            .try_next()
            .await
            .map_err(|e| Error::Storage(e.to_string()))?
        {
            if !meta.location.as_ref().ends_with("/manifest.json") {
                continue;
            }
            let (mut manifest, mode) = load_manifest(self.store.as_ref(), &meta.location).await?;
            let expired: Vec<_> = manifest
                .objects
                .iter()
                .filter(|o| o.expires_at <= now)
                .cloned()
                .collect();
            manifest.objects.retain(|o| o.expires_at > now);
            // Keep compacting work bounded. Old files remain for in-flight external readers.
            if manifest.objects.len() >= 16 {
                let mut selected = vec![];
                let mut bytes = 0;
                for object in &manifest.objects {
                    if selected.len() < 16 && bytes + object.uncompressed_bytes <= 32 * 1024 * 1024
                    {
                        bytes += object.uncompressed_bytes;
                        selected.push(object.clone());
                    }
                }
                if selected.len() < 2 {
                    selected.clear();
                }
                let mut entries = std::collections::BTreeMap::new();
                for object in &selected {
                    let bytes = self
                        .store
                        .get(&Path::from(object.path.clone()))
                        .await
                        .map_err(|e| Error::Storage(e.to_string()))?
                        .bytes()
                        .await
                        .map_err(|e| Error::Storage(e.to_string()))?;
                    if hex::encode(Sha256::digest(&bytes)) != object.sha256 {
                        return Err(Error::Storage("archive checksum mismatch".into()));
                    }
                    let rows = tokio::task::spawn_blocking(move || decode(bytes))
                        .await
                        .map_err(|e| Error::Storage(e.to_string()))??;
                    for entry in rows {
                        if entry.archive_until > now {
                            entries.insert(entry.id.clone(), entry);
                        }
                    }
                }
                if !entries.is_empty() {
                    let partition = meta.location.as_ref().trim_end_matches("/manifest.json");
                    let compacted = write_object(
                        self.store.as_ref(),
                        partition,
                        entries.into_values().collect(),
                    )
                    .await?;
                    manifest
                        .objects
                        .retain(|object| !selected.iter().any(|old| old.path == object.path));
                    manifest.objects.push(compacted);
                }
            }
            if save_manifest(self.store.as_ref(), &meta.location, &manifest, mode).await? {
                removed += expired.len() as u64;
            }
        }
        // Garbage collection is separate from manifest publication. A 24-hour grace period
        // protects readers holding an earlier manifest and in-flight archive publications.
        let mut listing = self.store.list(Some(&prefix));
        while let Some(meta) = listing
            .try_next()
            .await
            .map_err(|e| Error::Storage(e.to_string()))?
        {
            if !meta.location.as_ref().ends_with(".parquet")
                || meta.last_modified > now - chrono::Duration::days(1)
            {
                continue;
            }
            let partition = meta
                .location
                .as_ref()
                .rsplit_once('/')
                .expect("partitioned object")
                .0;
            let (manifest, _) = load_manifest(
                self.store.as_ref(),
                &Path::from(format!("{partition}/manifest.json")),
            )
            .await?;
            if !manifest
                .objects
                .iter()
                .any(|o| o.path == meta.location.as_ref())
            {
                self.store
                    .delete(&meta.location)
                    .await
                    .map_err(|e| Error::Storage(e.to_string()))?;
            }
        }
        Ok(removed)
    }
}
fn decode(bytes: bytes::Bytes) -> Result<Vec<LedgerEntry>> {
    use parquet::file::reader::FileReader;
    use parquet::record::RowAccessor;
    let reader = parquet::file::reader::SerializedFileReader::new(bytes)
        .map_err(|e| Error::Storage(e.to_string()))?;
    reader
        .get_row_iter(None)
        .map_err(|e| Error::Storage(e.to_string()))?
        .map(|row| {
            let row = row.map_err(|e| Error::Storage(e.to_string()))?;
            let value = row
                .get_string(14)
                .map_err(|e| Error::Storage(e.to_string()))?;
            serde_json::from_str(value).map_err(Error::from)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;
    use chrono::Utc;
    #[tokio::test]
    async fn committed_parquet_compaction_and_retention() {
        let store = Arc::new(object_store::memory::InMemory::new());
        let archive = ParquetArchive::with_store(store.clone(), "cdc".into());
        let now = Utc::now();
        let mut entries = vec![];
        for i in 0..18 {
            entries.push(LedgerEntry {
                id: format!("change-{i}"),
                scope: TimelineScope {
                    namespace: "ns".into(),
                    tenant: "tenant".into(),
                },
                received_at: now,
                retain_until: now + chrono::Duration::days(1),
                archive_until: now + chrono::Duration::days(30),
                change: TimelineChange {
                    schema_version: 1,
                    change_id: format!("change-{i}"),
                    source: "test".into(),
                    event_id: i.to_string(),
                    revision: 1,
                    operation: ChangeOperation::Delete,
                    before: None,
                    after: None,
                },
                raw: serde_json::json!({"op":"delete"}),
                transport: None,
                disposition: ChangeDisposition::Applied,
                projection: None,
            });
        }
        // Independent batches exercise manifest CAS and compaction.
        let partition = format!(
            "cdc/namespace={}/tenant={}/date={}/hour={}",
            crate::digest(&"ns"),
            crate::digest(&"tenant"),
            now.format("%Y-%m-%d"),
            now.format("%H")
        );
        for entry in &entries {
            publish_batch(store.as_ref(), &partition, vec![entry.clone()])
                .await
                .unwrap();
        }
        archive.cleanup(now).await.unwrap();
        let (manifest, _) = load_manifest(
            store.as_ref(),
            &Path::from(format!("{partition}/manifest.json")),
        )
        .await
        .unwrap();
        assert_eq!(manifest.objects.len(), 3);
        let mut ids = std::collections::BTreeSet::new();
        for object in manifest.objects {
            let bytes = store
                .get(&Path::from(object.path))
                .await
                .unwrap()
                .bytes()
                .await
                .unwrap();
            for entry in decode(bytes).unwrap() {
                ids.insert(entry.id);
            }
        }
        assert_eq!(ids.len(), 18);
        archive
            .cleanup(now + chrono::Duration::days(31))
            .await
            .unwrap();
        let (manifest, _) = load_manifest(
            store.as_ref(),
            &Path::from(format!("{partition}/manifest.json")),
        )
        .await
        .unwrap();
        assert!(manifest.objects.is_empty());
    }
}
