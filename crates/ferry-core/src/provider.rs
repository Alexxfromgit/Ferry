//! Sender side: decides which requests are served and tracks upload activity per share.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

use iroh_blobs::{
    Hash,
    provider::events::{
        AbortReason, ConnectMode, EventMask, ObserveMode, ProviderMessage, RequestMode,
        RequestUpdate, ThrottleMode,
    },
};
use tokio::sync::{broadcast, mpsc, watch};

use crate::events::Event;

/// Which provider events we subscribe to.
///
/// - `get` is intercepted so that only active shares are served. Each served request is also
///   tracked; when its share is removed, the tracker drops the request's progress channel, and
///   iroh-blobs aborts the transfer at its next write. So "Stop sharing" also ends downloads
///   that are already running.
/// - `get_many`, `push` and `observe` are not used by Ferry clients and are refused.
pub const EVENT_MASK: EventMask = EventMask {
    connected: ConnectMode::None,
    get: RequestMode::InterceptLog,
    get_many: RequestMode::Disabled,
    push: RequestMode::Disabled,
    throttle: ThrottleMode::None,
    observe: ObserveMode::Intercept,
};

#[derive(Default)]
struct ShareStats {
    uploaded: u64,
    /// request id -> connection id, for requests that are in flight.
    requests: HashMap<u64, u64>,
    dirty: bool,
}

impl ShareStats {
    fn active_peers(&self) -> usize {
        self.requests.values().collect::<HashSet<_>>().len()
    }
}

pub struct ProviderState {
    shared: RwLock<HashSet<Hash>>,
    stats: Mutex<HashMap<Hash, ShareStats>>,
    /// Bumped whenever a share is removed, so running transfers re-check their permission.
    revoked: watch::Sender<u64>,
}

impl Default for ProviderState {
    fn default() -> Self {
        Self {
            shared: RwLock::default(),
            stats: Mutex::default(),
            revoked: watch::Sender::new(0),
        }
    }
}

impl ProviderState {
    pub fn allow(&self, hash: Hash) {
        self.shared.write().unwrap().insert(hash);
    }

    pub fn disallow(&self, hash: &Hash) {
        self.shared.write().unwrap().remove(hash);
        self.stats.lock().unwrap().remove(hash);
        self.revoked.send_modify(|n| *n += 1);
    }

    fn is_shared(&self, hash: &Hash) -> bool {
        self.shared.read().unwrap().contains(hash)
    }

    /// Update the stats of a share; ignored once the share has been removed.
    fn with_stats(&self, hash: Hash, f: impl FnOnce(&mut ShareStats)) {
        if !self.is_shared(&hash) {
            return;
        }
        let mut stats = self.stats.lock().unwrap();
        let entry = stats.entry(hash).or_default();
        f(entry);
        entry.dirty = true;
    }

    /// Take a snapshot of every share whose activity changed since the last call.
    fn take_changes(&self) -> Vec<(Hash, usize, u64)> {
        let mut stats = self.stats.lock().unwrap();
        stats
            .iter_mut()
            .filter(|(_, s)| s.dirty)
            .map(|(hash, s)| {
                s.dirty = false;
                (*hash, s.active_peers(), s.uploaded)
            })
            .collect()
    }
}

/// Handle provider messages until the blobs protocol shuts down.
pub async fn run(state: Arc<ProviderState>, mut rx: mpsc::Receiver<ProviderMessage>) {
    while let Some(msg) = rx.recv().await {
        match msg {
            ProviderMessage::GetRequestReceived(msg) => {
                let hash = msg.inner.request.hash;
                let connection_id = msg.inner.connection_id;
                let request_id = msg.inner.request_id;
                if state.is_shared(&hash) {
                    msg.tx.send(Ok(())).await.ok();
                    let state = state.clone();
                    tokio::spawn(track_request(
                        state,
                        hash,
                        connection_id,
                        request_id,
                        msg.rx,
                    ));
                } else {
                    tracing::debug!(%hash, "refusing request for a hash that is not shared");
                    msg.tx.send(Err(AbortReason::Permission)).await.ok();
                }
            }
            ProviderMessage::ObserveRequestReceived(msg) => {
                msg.tx.send(Err(AbortReason::Permission)).await.ok();
            }
            ProviderMessage::ClientConnected(msg) => {
                msg.tx.send(Ok(())).await.ok();
            }
            ProviderMessage::Throttle(msg) => {
                msg.tx.send(Ok(())).await.ok();
            }
            _ => {}
        }
    }
}

async fn track_request(
    state: Arc<ProviderState>,
    hash: Hash,
    connection_id: u64,
    request_id: u64,
    mut rx: irpc::channel::mpsc::Receiver<RequestUpdate>,
) {
    let mut revoked = state.revoked.subscribe();
    state.with_stats(hash, |s| {
        s.requests.insert(request_id, connection_id);
    });
    // Progress events only carry the end offset of each write. Writes within a blob are
    // contiguous, so the difference between consecutive offsets is a good live estimate; the
    // first write of a blob (whose start is unknown) and jumps between ranges count as zero.
    // When the request ends, the exact payload byte count replaces the estimate.
    const MAX_WRITE: u64 = 4 * 1024 * 1024;
    let mut last_end: Option<u64> = None;
    let mut counted = 0u64;
    loop {
        let update = tokio::select! {
            update = rx.recv() => update,
            changed = revoked.changed() => {
                if changed.is_err() || !state.is_shared(&hash) {
                    tracing::info!(%hash, request_id, "share removed; ending a running transfer");
                    // Dropping `rx` makes the provider's next progress write fail, which
                    // aborts the transfer.
                    return;
                }
                continue;
            }
        };
        let Ok(Some(update)) = update else { break };
        match update {
            RequestUpdate::Started(_) => last_end = None,
            RequestUpdate::Progress(p) => {
                let delta = match last_end {
                    Some(prev) if p.end_offset > prev && p.end_offset - prev <= MAX_WRITE => {
                        p.end_offset - prev
                    }
                    _ => 0,
                };
                last_end = Some(p.end_offset);
                if delta > 0 {
                    counted += delta;
                    state.with_stats(hash, |s| s.uploaded += delta);
                }
            }
            RequestUpdate::Completed(done) => {
                reconcile(&state, hash, counted, done.stats.payload_bytes_sent);
                break;
            }
            RequestUpdate::Aborted(aborted) => {
                reconcile(&state, hash, counted, aborted.stats.payload_bytes_sent);
                break;
            }
        }
    }
    state.with_stats(hash, |s| {
        s.requests.remove(&request_id);
    });
}

/// Replace the live estimate for one request with the exact number of bytes sent.
fn reconcile(state: &ProviderState, hash: Hash, estimated: u64, exact: u64) {
    state.with_stats(hash, |s| {
        s.uploaded = s.uploaded.saturating_sub(estimated) + exact;
    });
}

/// Periodically publish [`Event::ShareActivity`] for shares whose activity changed.
pub async fn emit_activity(state: Arc<ProviderState>, events: broadcast::Sender<Event>) {
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    loop {
        tick.tick().await;
        for (hash, active_peers, uploaded_bytes) in state.take_changes() {
            events
                .send(Event::ShareActivity {
                    share_id: hash.to_hex().to_string(),
                    active_peers,
                    uploaded_bytes,
                })
                .ok();
        }
    }
}
