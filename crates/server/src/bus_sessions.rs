//! Bounded process-local registry of live Kafka consumers. One actor owns each
//! consumer; disconnecting an HTTP request cannot cancel an in-flight commit.
use crate::{
    api::bus_sessions::{
        ReceiptPosition, ReceiptResponse, ReceiveRequest, ReceiveResponse, SessionDelivery,
        SessionResponse,
    },
    auth::identity::CallerIdentity,
    config::BusSessionConfig,
};
use acteon_bus::{
    AcknowledgedSubscription, SharedBackend, SubscriptionConfig, SubscriptionError,
    SubscriptionReceipt,
};
use acteon_core::Subscription;
use axum::http::StatusCode;
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    time::Instant,
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct SessionError {
    pub status: StatusCode,
    pub message: String,
}
impl SessionError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
    fn closed() -> Self {
        Self::new(
            StatusCode::GONE,
            "session closed; open a new session and replay from the durable checkpoint",
        )
    }
}
impl From<SubscriptionError> for SessionError {
    fn from(e: SubscriptionError) -> Self {
        let status = match e {
            SubscriptionError::Unsupported => StatusCode::NOT_IMPLEMENTED,
            SubscriptionError::Closed => StatusCode::GONE,
            SubscriptionError::UnknownReceipt => StatusCode::NOT_FOUND,
            SubscriptionError::InvalidConfig(_) => StatusCode::BAD_REQUEST,
            SubscriptionError::Bus(_) | SubscriptionError::InvalidRecord(_) => {
                StatusCode::BAD_GATEWAY
            }
            _ => StatusCode::CONFLICT,
        };
        Self::new(status, e.to_string())
    }
}
#[derive(Clone, PartialEq, Eq, Hash)]
struct Scope {
    namespace: String,
    tenant: String,
    id: String,
}
impl From<&Subscription> for Scope {
    fn from(s: &Subscription) -> Self {
        Self {
            namespace: s.namespace.clone(),
            tenant: s.tenant.clone(),
            id: s.id.clone(),
        }
    }
}
#[derive(Clone, PartialEq, Eq, Hash)]
struct Owner {
    id: String,
    method: String,
}
impl From<&CallerIdentity> for Owner {
    fn from(i: &CallerIdentity) -> Self {
        Self {
            id: i.id.clone(),
            method: i.auth_method.clone(),
        }
    }
}
#[derive(PartialEq, Eq, Hash)]
struct OpenKey {
    scope: Scope,
    owner: Owner,
    request: Uuid,
}
struct RegistryInner {
    entries: Mutex<HashMap<Uuid, Arc<SessionHandle>>>,
    cancel: CancellationToken,
    tasks: TaskTracker,
    config: BusSessionConfig,
}
impl Drop for RegistryInner {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
#[derive(Clone)]
pub struct BusSessionRegistry {
    inner: Arc<RegistryInner>,
}
impl Default for BusSessionRegistry {
    fn default() -> Self {
        Self::new(BusSessionConfig::default()).expect("default session limits")
    }
}
impl BusSessionRegistry {
    pub fn new(config: BusSessionConfig) -> Result<Self, String> {
        config.validate()?;
        Ok(Self {
            inner: Arc::new(RegistryInner {
                entries: Mutex::new(HashMap::new()),
                cancel: CancellationToken::new(),
                tasks: TaskTracker::new(),
                config,
            }),
        })
    }
    pub fn open(
        &self,
        sub: Subscription,
        identity: &CallerIdentity,
        request: Uuid,
        backend: SharedBackend,
    ) -> Result<Arc<SessionHandle>, SessionError> {
        if self.inner.cancel.is_cancelled() {
            return Err(SessionError::closed());
        }
        if !sub.receipt_required || sub.ack_mode != acteon_core::AckMode::Manual {
            return Err(SessionError::new(
                StatusCode::CONFLICT,
                "create a manual, receipt_required subscription first",
            ));
        }
        let key = OpenKey {
            scope: Scope::from(&sub),
            owner: Owner::from(identity),
            request,
        };
        let mut entries = self.inner.entries.lock().expect("session registry lock");
        if self.inner.cancel.is_cancelled() {
            return Err(SessionError::closed());
        }
        let retention = Duration::from_millis(self.inner.config.closed_retention_ms);
        entries.retain(|_, h| {
            h.closed_at
                .lock()
                .expect("closed timestamp")
                .is_none_or(|t| t.elapsed() < retention)
        });
        if let Some(h) = entries.values().find(|h| h.key == key) {
            if !h.matches(&sub) {
                return Err(SessionError::new(
                    StatusCode::CONFLICT,
                    "subscription definition changed",
                ));
            }
            return Ok(Arc::clone(h));
        }
        if entries.len() >= self.inner.config.max_sessions
            || entries
                .values()
                .filter(|h| h.key.scope.tenant == sub.tenant)
                .count()
                >= self.inner.config.max_sessions_per_tenant
        {
            return Err(SessionError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "session capacity reached (closed sessions count during retention)",
            ));
        }
        let id = Uuid::new_v4();
        let (tx, rx) = mpsc::channel(32);
        let (snapshot_tx, snapshot) = watch::channel(SessionResponse {
            session_id: id,
            consumer_group: sub.consumer_group(),
            phase: "opening".into(),
            assignment_epoch: 0,
            partitions: vec![],
            pending: 0,
            buffered_bytes: 0,
            closed_reason: None,
        });
        let handle = Arc::new(SessionHandle {
            key,
            definition: serde_json::to_value(&sub).expect("subscription serializes"),
            commands: tx,
            snapshot,
            cancel: self.inner.cancel.child_token(),
            closed_at: Mutex::new(None),
            opening_error: Mutex::new(None),
        });
        entries.insert(id, Arc::clone(&handle));
        let actor_handle = Arc::clone(&handle);
        let config = self.inner.config.clone();
        self.inner.tasks.spawn(async move {
            let start=match sub.starting_offset {acteon_core::SubscriptionStartOffset::Earliest=>acteon_bus::StartOffset::Earliest,acteon_core::SubscriptionStartOffset::Latest=>acteon_bus::StartOffset::Latest};
            let consumer_group = sub.consumer_group();
            let opened=tokio::select! {
                ()=actor_handle.cancel.cancelled()=>Err(SessionError::closed()),
                r=tokio::time::timeout(Duration::from_secs(10),backend.subscribe_acknowledged(&sub.topic,&consumer_group,start,SubscriptionConfig {max_in_flight:config.max_in_flight}))=>match r { Ok(r)=>r.map_err(SessionError::from),Err(_)=>Err(SessionError::new(StatusCode::GATEWAY_TIMEOUT,"consumer open timed out")) }
            };
            match opened {
                Ok(consumer)=>Actor::new(consumer,config,sub.ack_timeout_ms,snapshot_tx.clone()).run(rx,&actor_handle).await,
                Err(e)=>{ *actor_handle.opening_error.lock().expect("opening error")=Some(e.clone()); snapshot_tx.send_modify(|s|s.closed_reason=Some(e.message)); }
            }
            *actor_handle.closed_at.lock().expect("closed timestamp")=Some(Instant::now());
            snapshot_tx.send_modify(|s| {s.phase="closed".into();s.pending=0;s.buffered_bytes=0;if s.closed_reason.is_none(){s.closed_reason=Some("session closed".into());}});
        });
        Ok(handle)
    }
    pub fn get(
        &self,
        sub: &Subscription,
        identity: &CallerIdentity,
        id: Uuid,
    ) -> Result<Arc<SessionHandle>, SessionError> {
        let entries = self.inner.entries.lock().expect("session registry lock");
        let h = entries
            .get(&id)
            .filter(|h| h.key.scope == Scope::from(sub) && h.key.owner == Owner::from(identity))
            .ok_or_else(|| {
                SessionError::new(StatusCode::NOT_FOUND, "session not found on this server")
            })?;
        if !h.matches(sub) {
            h.close();
            return Err(SessionError::new(
                StatusCode::CONFLICT,
                "subscription definition changed",
            ));
        }
        Ok(Arc::clone(h))
    }
    pub fn close_subscription(&self, namespace: &str, tenant: &str, id: &str) {
        for h in self
            .inner
            .entries
            .lock()
            .expect("session registry lock")
            .values()
        {
            if h.key.scope.namespace == namespace
                && h.key.scope.tenant == tenant
                && h.key.scope.id == id
            {
                h.close();
            }
        }
    }
    pub async fn shutdown(&self) {
        {
            // Serialize shutdown against registration: no actor can be spawned
            // after the tracker has drained. Never hold this lock across await.
            let _entries = self.inner.entries.lock().expect("session registry lock");
            self.inner.cancel.cancel();
            self.inner.tasks.close();
        }
        self.inner.tasks.wait().await;
    }
}

pub struct SessionHandle {
    key: OpenKey,
    definition: serde_json::Value,
    commands: mpsc::Sender<Command>,
    snapshot: watch::Receiver<SessionResponse>,
    cancel: CancellationToken,
    closed_at: Mutex<Option<Instant>>,
    opening_error: Mutex<Option<SessionError>>,
}
impl SessionHandle {
    pub fn matches(&self, s: &Subscription) -> bool {
        serde_json::to_value(s).is_ok_and(|v| v == self.definition)
    }
    pub fn snapshot(&self) -> SessionResponse {
        self.snapshot.borrow().clone()
    }
    pub fn close(&self) {
        self.cancel.cancel();
    }
    pub async fn ready(&self) -> Result<SessionResponse, SessionError> {
        if self.cancel.is_cancelled() {
            return Err(SessionError::closed());
        }
        let mut changes = self.snapshot.clone();
        loop {
            let s = changes.borrow_and_update().clone();
            match s.phase.as_str() {
                "active" => return Ok(s),
                "closed" => {
                    return Err(self
                        .opening_error
                        .lock()
                        .expect("opening error")
                        .clone()
                        .unwrap_or_else(SessionError::closed));
                }
                _ => {}
            }
            changes
                .changed()
                .await
                .map_err(|_| SessionError::closed())?;
        }
    }
    pub async fn receive(&self, request: ReceiveRequest) -> Result<ReceiveResponse, SessionError> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::Receive(request, reply))?;
        rx.await.unwrap_or_else(|_| Err(SessionError::closed()))
    }
    pub async fn receipts(
        &self,
        ids: Vec<Uuid>,
        commit: bool,
    ) -> Result<ReceiptResponse, SessionError> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::Receipts(ids, commit, reply))?;
        rx.await.unwrap_or_else(|_| Err(SessionError::closed()))
    }
    fn send(&self, c: Command) -> Result<(), SessionError> {
        if self.cancel.is_cancelled() || self.snapshot.borrow().phase == "closed" {
            return Err(SessionError::closed());
        }
        self.commands.try_send(c).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => {
                SessionError::new(StatusCode::TOO_MANY_REQUESTS, "session command queue full")
            }
            mpsc::error::TrySendError::Closed(_) => SessionError::closed(),
        })
    }
}
enum Command {
    Receive(
        ReceiveRequest,
        oneshot::Sender<Result<ReceiveResponse, SessionError>>,
    ),
    Receipts(
        Vec<Uuid>,
        bool,
        oneshot::Sender<Result<ReceiptResponse, SessionError>>,
    ),
}
struct Pending {
    wire: SessionDelivery,
    receipt: SubscriptionReceipt,
    bytes: usize,
    received: Instant,
}
struct Actor {
    consumer: Box<dyn AcknowledgedSubscription>,
    config: BusSessionConfig,
    snapshot: watch::Sender<SessionResponse>,
    pending: VecDeque<Pending>,
    history: VecDeque<(Uuid, SubscriptionReceipt)>,
    stale: VecDeque<Uuid>,
    bytes: usize,
    created: Instant,
    last_activity: Instant,
    ack_timeout: Duration,
}
impl Actor {
    fn new(
        consumer: Box<dyn AcknowledgedSubscription>,
        config: BusSessionConfig,
        ack_timeout_ms: u64,
        snapshot: watch::Sender<SessionResponse>,
    ) -> Self {
        Self {
            consumer,
            config,
            snapshot,
            pending: VecDeque::new(),
            history: VecDeque::new(),
            stale: VecDeque::new(),
            bytes: 0,
            created: Instant::now(),
            last_activity: Instant::now(),
            ack_timeout: Duration::from_millis(ack_timeout_ms),
        }
    }
    fn expired(&self) -> Option<&'static str> {
        if self.created.elapsed() >= Duration::from_millis(self.config.lifetime_ms) {
            Some("absolute lifetime exceeded")
        } else if self.last_activity.elapsed() >= Duration::from_millis(self.config.idle_timeout_ms)
        {
            Some("idle timeout exceeded")
        } else if self
            .pending
            .front()
            .is_some_and(|p| p.received.elapsed() >= self.ack_timeout)
        {
            Some("receipt acknowledgement timeout exceeded")
        } else {
            None
        }
    }
    fn sync_ownership(&mut self) {
        let ownership = self.consumer.ownership_changes().borrow().clone();
        for p in &self.pending {
            if p.receipt.assignment_epoch() != ownership.epoch {
                self.stale.push_back(p.wire.receipt_id);
            }
        }
        for (id, r) in &self.history {
            if r.assignment_epoch() != ownership.epoch {
                self.stale.push_back(*id);
            }
        }
        while self.stale.len() > self.config.max_ack_history {
            self.stale.pop_front();
        }
        self.pending
            .retain(|p| p.receipt.assignment_epoch() == ownership.epoch);
        self.history
            .retain(|(_, r)| r.assignment_epoch() == ownership.epoch);
        self.bytes = self.pending.iter().map(|p| p.bytes).sum();
        self.snapshot.send_modify(|s| {
            s.assignment_epoch = ownership.epoch;
            s.partitions.clone_from(&ownership.partitions);
            s.pending = self.pending.len();
            s.buffered_bytes = self.bytes;
        });
    }
    async fn run(mut self, mut rx: mpsc::Receiver<Command>, handle: &SessionHandle) {
        self.snapshot.send_modify(|s| s.phase = "active".into());
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        loop {
            if let Some(reason) = self.expired() {
                self.snapshot
                    .send_modify(|s| s.closed_reason = Some(reason.into()));
                break;
            }
            self.sync_ownership();
            tokio::select! {
                biased;
                ()=handle.cancel.cancelled()=>break,
                _=tick.tick()=>{},
                delivery=self.consumer.recv(), if self.pending.len()<self.config.max_in_flight && self.bytes<self.config.max_buffer_bytes => {
                    self.sync_ownership();
                    match delivery.map_err(SessionError::from).and_then(|d|self.retain(d)) {
                        Ok(())=>{},
                        Err(e)=>{self.snapshot.send_modify(|s|s.closed_reason=Some(e.message));break;}
                    }
                },
                command=rx.recv()=> {
                    let Some(command)=command else {break};
                    self.last_activity=Instant::now();
                    match command {
                        Command::Receive(req,reply)=> {let result=self.receive(req,&handle.cancel).await;let _=reply.send(result);},
                        Command::Receipts(ids,commit,reply)=> {let result=self.receipts(ids,commit).await;let _=reply.send(result);}
                    }
                }
            }
        }
        self.snapshot.send_modify(|s| {
            s.phase = "closed".into();
            s.pending = 0;
            s.buffered_bytes = 0;
        });
        // Kafka consumer destruction can wait for broker leave-group work. Keep
        // it off the async executor and inside the tracked session lifecycle.
        self.pending.clear();
        self.history.clear();
        self.stale.clear();
        drop(rx);
        let consumer = self.consumer;
        let _ = tokio::task::spawn_blocking(move || drop(consumer)).await;
    }
    fn retain(&mut self, d: acteon_bus::SubscriptionDelivery) -> Result<(), SessionError> {
        if d.receipt.assignment_epoch() != self.snapshot.borrow().assignment_epoch {
            return Ok(());
        }
        let message = serde_json::to_value(&d.message)
            .map_err(|e| SessionError::new(StatusCode::BAD_GATEWAY, e.to_string()))?;
        let bytes = serde_json::to_vec(&message)
            .map_err(|e| SessionError::new(StatusCode::BAD_GATEWAY, e.to_string()))?
            .len();
        if bytes > self.config.max_buffer_bytes.saturating_sub(self.bytes) {
            return Err(SessionError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "session payload buffer limit exceeded; no offset acknowledged",
            ));
        }
        let position = d.receipt.position();
        self.pending.push_back(Pending {
            wire: SessionDelivery {
                receipt_id: Uuid::new_v4(),
                message,
                partition: position.partition,
                offset: position.offset,
                assignment_epoch: d.receipt.assignment_epoch(),
            },
            receipt: d.receipt,
            bytes,
            received: Instant::now(),
        });
        self.bytes += bytes;
        Ok(())
    }
    async fn receive(
        &mut self,
        req: ReceiveRequest,
        cancel: &CancellationToken,
    ) -> Result<ReceiveResponse, SessionError> {
        if req.max_messages == 0
            || req.max_messages > self.config.max_in_flight
            || req.wait_ms > 30_000
        {
            return Err(SessionError::new(
                StatusCode::BAD_REQUEST,
                "invalid receive limit or wait (maximum 30000ms)",
            ));
        }
        self.sync_ownership();
        // Replay cached receipts on a retry. No new poll hides an unacknowledged batch.
        if self.pending.is_empty() {
            let deadline = Instant::now() + Duration::from_millis(req.wait_ms);
            // Gather a bounded batch, stopping promptly after available records. The actor
            // retains each delivery before the next await, even if the HTTP client leaves.
            while self.pending.len() < req.max_messages {
                if self.expired().is_some() {
                    cancel.cancel();
                    return Err(SessionError::closed());
                }
                let wait = if self.pending.is_empty() {
                    deadline
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_millis(100))
                } else {
                    Duration::from_millis(10)
                };
                let delivery = tokio::select! {
                    ()=cancel.cancelled()=>return Err(SessionError::closed()),
                    r=tokio::time::timeout(wait,self.consumer.recv())=>r
                };
                self.sync_ownership();
                match delivery {
                    Ok(Ok(d)) => {
                        if let Err(e) = self.retain(d) {
                            cancel.cancel();
                            return Err(e);
                        }
                    }
                    Ok(Err(e)) => {
                        cancel.cancel();
                        return Err(e.into());
                    }
                    Err(_) => {
                        if !self.pending.is_empty() || Instant::now() >= deadline {
                            break;
                        }
                    }
                }
            }
        }
        self.sync_ownership();
        Ok(ReceiveResponse {
            deliveries: self
                .pending
                .iter()
                .take(req.max_messages)
                .map(|p| p.wire.clone())
                .collect(),
        })
    }
    async fn receipts(
        &mut self,
        ids: Vec<Uuid>,
        commit: bool,
    ) -> Result<ReceiptResponse, SessionError> {
        self.sync_ownership();
        if ids.is_empty()
            || ids.len()
                > self
                    .config
                    .max_in_flight
                    .saturating_add(self.config.max_ack_history)
        {
            return Err(SessionError::new(
                StatusCode::BAD_REQUEST,
                "empty or oversized receipt batch",
            ));
        }
        let mut receipts = Vec::new();
        let mut pending = Vec::new();
        for id in ids {
            if let Some(p) = self.pending.iter().find(|p| p.wire.receipt_id == id) {
                pending.push(p.receipt.clone());
                receipts.push(p.receipt.clone());
            } else if let Some((_, r)) = self.history.iter().find(|(i, _)| *i == id) {
                receipts.push(r.clone());
            } else {
                return Err(SessionError::new(
                    if self.stale.contains(&id) {
                        StatusCode::CONFLICT
                    } else {
                        StatusCode::NOT_FOUND
                    },
                    "unknown, expired, or revoked receipt",
                ));
            }
        }
        self.consumer.validate_receipts(&receipts)?;
        self.consumer.validate_checkpoint_receipts(&pending)?;
        let mut positions = BTreeMap::new();
        for r in &receipts {
            let p = r.position();
            positions
                .entry(p.partition)
                .and_modify(|offset: &mut i64| *offset = (*offset).max(p.offset))
                .or_insert(p.offset);
        }
        if commit && !pending.is_empty() {
            // Recheck the COMPLETE prefix at commit time. Arbitrary out-of-order
            // acknowledgements cannot advance past an uncheckpointed record.
            let ack = self.consumer.acknowledge(&pending).await?;
            let mut kept = VecDeque::new();
            for p in self.pending.drain(..) {
                let pos = p.receipt.position();
                if ack
                    .committed
                    .iter()
                    .any(|c| c.partition == pos.partition && c.offset >= pos.offset)
                {
                    self.history.push_back((p.wire.receipt_id, p.receipt));
                } else {
                    kept.push_back(p);
                }
            }
            self.pending = kept;
            while self.history.len() > self.config.max_ack_history {
                self.history.pop_front();
            }
        }
        self.sync_ownership();
        Ok(ReceiptResponse {
            consumer_group: self.snapshot.borrow().consumer_group.clone(),
            positions: positions
                .into_iter()
                .map(|(partition, offset)| ReceiptPosition { partition, offset })
                .collect(),
            remaining_in_flight: self.pending.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unsupported_backend_does_not_fall_back_and_closed_open_keys_do_not_resurrect() {
        let registry = BusSessionRegistry::default();
        let mut sub = Subscription::new("test", "ns.tenant.topic", "ns", "tenant");
        sub.receipt_required = true;
        let owner = CallerIdentity::anonymous();
        let request = Uuid::new_v4();
        let backend: SharedBackend = acteon_bus::MemoryBackend::new();
        let handle = registry
            .open(sub.clone(), &owner, request, backend.clone())
            .unwrap();
        assert_eq!(
            handle.ready().await.unwrap_err().status,
            StatusCode::NOT_IMPLEMENTED
        );
        let retry = registry.open(sub, &owner, request, backend).unwrap();
        assert_eq!(handle.snapshot().session_id, retry.snapshot().session_id);
        assert_eq!(
            retry.ready().await.unwrap_err().status,
            StatusCode::NOT_IMPLEMENTED
        );
        registry.shutdown().await;
    }

    #[tokio::test]
    async fn shutdown_rejects_new_registrations() {
        let registry = BusSessionRegistry::default();
        registry.shutdown().await;
        let mut sub = Subscription::new("test", "ns.tenant.topic", "ns", "tenant");
        sub.receipt_required = true;
        let result = registry.open(
            sub,
            &CallerIdentity::anonymous(),
            Uuid::new_v4(),
            acteon_bus::MemoryBackend::new(),
        );
        assert_eq!(result.err().unwrap().status, StatusCode::GONE);
    }

    #[test]
    fn invalid_capacity_is_rejected_before_starting_consumers() {
        let invalid = BusSessionConfig {
            max_in_flight: 0,
            ..Default::default()
        };
        assert!(BusSessionRegistry::new(invalid).is_err());
    }
}
