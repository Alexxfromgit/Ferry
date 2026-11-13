//! Sender side: decides which requests are served and tracks upload activity per share.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use iroh_blobs::{
    Hash,
    protocol::{ChunkRanges, ChunkRangesSeq},
    provider::events::{
        AbortReason, ConnectMode, EventMask, ObserveMode, ProviderMessage, RequestMode,
        RequestUpdate, ThrottleMode,
    },
};
use tokio::sync::{broadcast, mpsc, watch};

use crate::{
    events::{Event, ReceiverProgress},
    registry::FileLists,
    speed::SpeedMeter,
};

/// Which provider events we subscribe to.
///
/// - `get` is intercepted so that only active shares are served. Each served request is also
///   tracked; when its share is turned off or removed, the tracker drops the request's progress
///   channel, and iroh-blobs aborts the transfer at its next write. So turning a share off also
///   ends downloads that are already running.
/// - `get_many`, `push` and `observe` are not used by Ferry clients and are refused.
pub const EVENT_MASK: EventMask = EventMask {
    connected: ConnectMode::None,
    get: RequestMode::InterceptLog,
    get_many: RequestMode::Disabled,
    push: RequestMode::Disabled,
    throttle: ThrottleMode::None,
    observe: ObserveMode::Intercept,
};

/// How often activity snapshots are published.
const ACTIVITY_INTERVAL: Duration = Duration::from_millis(500);

/// In a request for a share (a hash sequence), offset 0 is the hash sequence itself, offset 1
/// the collection's name list, and the files follow in collection order.
const FIRST_FILE_OFFSET: u64 = 2;

/// What is known about a served share.
struct Served {
    /// Changes each time the share is turned on, so a transfer that started before it was
    /// turned off (and on again) can tell it is no longer welcome.
    generation: u64,
    total: u64,
    /// File sizes in collection order, read from the share's file list on first use (lists
    /// can be large, so they are not all read at start-up). Empty if the list is unreadable.
    sizes: Option<Arc<[u64]>>,
}

/// A request is identified by its connection and its id; request ids are QUIC stream
/// numbers, which repeat on every connection.
type RequestKey = (u64, u64);

struct RequestStats {
    /// Unique for the life of the node; what the UI uses to tell receivers apart.
    receiver_id: u64,
    /// File bytes this request asked for; zero for requests that only read the index.
    requested: u64,
    /// File bytes sent so far (a live estimate, see [`track_request`]).
    sent: u64,
    meter: SpeedMeter,
    bytes_per_sec: u64,
    eta_secs: Option<u64>,
}

impl RequestStats {
    /// Requests that only fetch the index (a preview, or the first step of a download) are
    /// short and carry no file data, so they are not shown as receivers.
    fn is_receiver(&self) -> bool {
        self.requested > 0
    }
}

#[derive(Default)]
struct ShareStats {
    uploaded: u64,
    /// Requests in flight.
    requests: BTreeMap<RequestKey, RequestStats>,
    dirty: bool,
}

impl ShareStats {
    fn active_peers(&self) -> usize {
        self.requests
            .iter()
            .filter(|(_, r)| r.is_receiver())
            .map(|((connection, _), _)| *connection)
            .collect::<HashSet<_>>()
            .len()
    }
}

/// One activity snapshot of a share.
pub(crate) struct Activity {
    hash: Hash,
    active_peers: usize,
    uploaded_bytes: u64,
    bytes_per_sec: u64,
    receivers: Vec<ReceiverProgress>,
}

pub struct ProviderState {
    shared: RwLock<HashMap<Hash, Served>>,
    stats: Mutex<HashMap<Hash, ShareStats>>,
    /// Bumped whenever a share stops being served, so running transfers re-check.
    revoked: watch::Sender<u64>,
    file_lists: FileLists,
    next_receiver: AtomicU64,
    next_generation: AtomicU64,
}

impl ProviderState {
    pub fn new(file_lists: FileLists) -> Self {
        Self {
            shared: RwLock::default(),
            stats: Mutex::default(),
            revoked: watch::Sender::new(0),
            file_lists,
            next_receiver: AtomicU64::new(1),
            next_generation: AtomicU64::new(1),
        }
    }

    /// Serve a share. `sizes` are its file sizes in collection order, if already at hand;
    /// otherwise they are read from its file list when first needed. Serving a share that is
    /// already served keeps its running transfers.
    pub fn allow(&self, hash: Hash, total: u64, sizes: Option<Arc<[u64]>>) {
        let mut shared = self.shared.write().unwrap();
        match shared.get_mut(&hash) {
            Some(served) => {
                served.total = total;
                if sizes.is_some() {
                    served.sizes = sizes;
                }
            }
            None => {
                let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
                shared.insert(
                    hash,
                    Served {
                        generation,
                        total,
                        sizes,
                    },
                );
            }
        }
    }

    /// Stop serving a share for good: running transfers end and its statistics are dropped.
    pub fn disallow(&self, hash: &Hash) {
        self.shared.write().unwrap().remove(hash);
        self.stats.lock().unwrap().remove(hash);
        self.revoked.send_modify(|n| *n += 1);
    }

    /// Stop serving a share for now: running transfers end, but the total sent is kept for
    /// when it is turned on again. A last snapshot (with nobody receiving) is published.
    pub fn pause(&self, hash: &Hash) {
        self.shared.write().unwrap().remove(hash);
        if let Some(stats) = self.stats.lock().unwrap().get_mut(hash) {
            stats.requests.clear();
            stats.dirty = true;
        }
        self.revoked.send_modify(|n| *n += 1);
    }

    /// Admit a request for `hash` if the share is served, noting the generation it is served
    /// in. This happens before permission is sent, so a request admitted before the share is
    /// turned off is ended even if the share is turned on again before it is tracked. The
    /// subscription is made first, so a share turned off right after this is noticed too.
    fn admit(&self, hash: &Hash) -> Option<Admission> {
        let revoked = self.revoked.subscribe();
        let generation = self.generation(hash)?;
        Some(Admission {
            generation,
            revoked,
        })
    }

    /// The current generation of a served share, if it is served.
    fn generation(&self, hash: &Hash) -> Option<u64> {
        self.shared.read().unwrap().get(hash).map(|s| s.generation)
    }

    /// File bytes a request asks for, or `None` if the share is not served (any more).
    async fn requested_bytes(&self, hash: &Hash, ranges: &ChunkRangesSeq) -> Option<u64> {
        let (total, sizes) = {
            let shared = self.shared.read().unwrap();
            let served = shared.get(hash)?;
            (served.total, served.sizes.clone())
        };
        let sizes = match sizes {
            Some(sizes) => sizes,
            None => {
                let lists = self.file_lists.clone();
                let id = hash.to_hex().to_string();
                let read = tokio::task::spawn_blocking(move || lists.read(&id)).await;
                let sizes: Arc<[u64]> = match read {
                    Ok(Ok(files)) => files.iter().map(|f| f.size).collect(),
                    Ok(Err(e)) => {
                        tracing::warn!(%hash, "no file list for a share: {e:#}");
                        Arc::from([])
                    }
                    Err(e) => {
                        tracing::warn!(%hash, "reading a share's file list failed: {e}");
                        Arc::from([])
                    }
                };
                let mut shared = self.shared.write().unwrap();
                let served = shared.get_mut(hash)?;
                served.sizes.get_or_insert(sizes).clone()
            }
        };
        Some(if sizes.is_empty() {
            // No file list: a request for any file data is assumed to ask for everything.
            let asks_for_files = ranges
                .iter_non_empty_infinite()
                .any(|(offset, _)| offset >= FIRST_FILE_OFFSET);
            if asks_for_files { total } else { 0 }
        } else {
            requested_file_bytes(ranges, &sizes)
        })
    }

    /// Update the stats of a share, only while it is served in `generation`. The check and the
    /// update happen under the `shared` read lock, so a share can't be turned off in between
    /// (`pause` and `disallow` need the write lock). Lock order: `shared`, then `stats`.
    /// Returns whether the update happened.
    fn with_stats(&self, hash: Hash, generation: u64, f: impl FnOnce(&mut ShareStats)) -> bool {
        let shared = self.shared.read().unwrap();
        if shared.get(&hash).map(|s| s.generation) != Some(generation) {
            return false;
        }
        let mut stats = self.stats.lock().unwrap();
        let entry = stats.entry(hash).or_default();
        f(entry);
        entry.dirty = true;
        true
    }

    /// Forget a finished request, whatever the state of its share. Never creates stats.
    fn forget(&self, hash: &Hash, key: &RequestKey) {
        if let Some(s) = self.stats.lock().unwrap().get_mut(hash)
            && s.requests.remove(key).is_some()
        {
            s.dirty = true;
        }
    }

    /// Take a snapshot of every share whose activity changed since the last call. Shares with
    /// transfers in flight count as changed on every call, so speeds stay current.
    fn take_changes(&self) -> Vec<Activity> {
        let mut stats = self.stats.lock().unwrap();
        stats
            .iter_mut()
            .filter(|(_, s)| s.dirty || !s.requests.is_empty())
            .map(|(hash, s)| {
                s.dirty = false;
                let mut receivers = Vec::new();
                let mut bytes_per_sec = 0;
                for r in s.requests.values_mut() {
                    if !r.is_receiver() {
                        continue;
                    }
                    let done = r.sent.min(r.requested);
                    if let Some((speed, eta)) = r.meter.sample(done, r.requested) {
                        r.bytes_per_sec = speed;
                        r.eta_secs = eta;
                    }
                    bytes_per_sec += r.bytes_per_sec;
                    receivers.push(ReceiverProgress {
                        id: r.receiver_id,
                        done_bytes: done,
                        total_bytes: r.requested,
                        bytes_per_sec: r.bytes_per_sec,
                        eta_secs: r.eta_secs,
                    });
                }
                receivers.sort_by_key(|r| r.id);
                Activity {
                    hash: *hash,
                    active_peers: s.active_peers(),
                    uploaded_bytes: s.uploaded,
                    bytes_per_sec,
                    receivers,
                }
            })
            .collect()
    }
}

/// Bytes covered by `ranges` in a blob of `size` bytes that fall within `from..to` (chunks
/// are 1 KiB; the last one may be shorter). An open-ended range reaches the end of the blob.
fn range_bytes_within(ranges: &ChunkRanges, size: u64, from: u64, to: u64) -> u64 {
    let to = to.min(size);
    let to_bytes = |chunk: u64| chunk.saturating_mul(1024).min(size);
    ranges
        .boundaries()
        .chunks(2)
        .map(|pair| {
            let start = to_bytes(pair[0].0).max(from);
            let end = pair.get(1).map_or(size, |end| to_bytes(end.0)).min(to);
            end.saturating_sub(start)
        })
        .sum()
}

/// Bytes covered by `ranges` in a blob of `size` bytes.
fn range_bytes(ranges: &ChunkRanges, size: u64) -> u64 {
    range_bytes_within(ranges, size, 0, size)
}

/// Where the first requested range of a blob starts, in bytes.
fn first_range_start(ranges: &ChunkRanges) -> Option<u64> {
    ranges
        .boundaries()
        .first()
        .map(|c| c.0.saturating_mul(1024))
}

/// File bytes a request asks for, given the share's file sizes in collection order.
fn requested_file_bytes(ranges: &ChunkRangesSeq, sizes: &[u64]) -> u64 {
    sizes
        .iter()
        .zip(FIRST_FILE_OFFSET..)
        .map(|(&size, offset)| range_bytes(&ranges[offset], size))
        .sum()
}

/// Handle provider messages until the blobs protocol shuts down.
pub async fn run(state: Arc<ProviderState>, mut rx: mpsc::Receiver<ProviderMessage>) {
    while let Some(msg) = rx.recv().await {
        match msg {
            ProviderMessage::GetRequestReceived(msg) => {
                let hash = msg.inner.request.hash;
                if let Some(admission) = state.admit(&hash) {
                    msg.tx.send(Ok(())).await.ok();
                    let request = Request {
                        key: (msg.inner.connection_id, msg.inner.request_id),
                        ranges: msg.inner.request.ranges.clone(),
                    };
                    tokio::spawn(track_request(
                        state.clone(),
                        hash,
                        admission,
                        request,
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

struct Request {
    key: RequestKey,
    ranges: ChunkRangesSeq,
}

/// A request let through: the generation of the share it was let through in, and a
/// subscription to revocations made before that generation was read.
struct Admission {
    generation: u64,
    revoked: watch::Receiver<u64>,
}

async fn track_request(
    state: Arc<ProviderState>,
    hash: Hash,
    admission: Admission,
    request: Request,
    mut rx: irpc::channel::mpsc::Receiver<RequestUpdate>,
) {
    let Request { key, ranges } = request;
    let Admission {
        generation,
        mut revoked,
    } = admission;
    let stop = || tracing::info!(%hash, request = ?key, "share turned off; ending a transfer");
    // Everything below belongs to the share as served in the generation the request was
    // admitted in. If it is turned off (even if it is turned on again before this task runs or
    // notices), the transfer ends.
    let Some(requested) = state.requested_bytes(&hash, &ranges).await else {
        return stop();
    };
    // Registered only if still served in the same generation, checked under the lock that
    // turning off needs, so an entry can't be left behind by a share turned off meanwhile.
    let registered = state.with_stats(hash, generation, |s| {
        s.requests.insert(
            key,
            RequestStats {
                receiver_id: state.next_receiver.fetch_add(1, Ordering::Relaxed),
                requested,
                sent: 0,
                meter: SpeedMeter::new(0, Duration::ZERO),
                bytes_per_sec: 0,
                eta_secs: None,
            },
        );
    });
    if !registered {
        return stop();
    }
    // Progress events carry the end offset of each write. Each blob starts at its first
    // requested range, and only bytes inside the requested ranges count, so gaps between
    // ranges are skipped. Events can be dropped under load, which only makes the live
    // estimate low; when the request ends, the exact payload byte count replaces it in
    // `uploaded`.
    let mut blob: Option<(ChunkRanges, u64)> = None;
    let mut last_end = 0u64;
    let mut is_file = false;
    let mut counted = 0u64;
    loop {
        let update = tokio::select! {
            update = rx.recv() => update,
            changed = revoked.changed() => {
                if changed.is_err() || state.generation(&hash) != Some(generation) {
                    stop();
                    // Dropping `rx` makes the provider's next progress write fail, which
                    // aborts the transfer.
                    state.forget(&hash, &key);
                    return;
                }
                continue;
            }
        };
        let Ok(Some(update)) = update else { break };
        match update {
            RequestUpdate::Started(started) => {
                let ranges = ranges[started.index].clone();
                last_end = first_range_start(&ranges).unwrap_or(0).min(started.size);
                blob = Some((ranges, started.size));
                is_file = started.index >= FIRST_FILE_OFFSET;
            }
            RequestUpdate::Progress(p) => {
                let delta = match &blob {
                    Some((ranges, size)) if p.end_offset > last_end => {
                        range_bytes_within(ranges, *size, last_end, p.end_offset)
                    }
                    _ => 0,
                };
                last_end = last_end.max(p.end_offset);
                if delta > 0 {
                    counted += delta;
                    state.with_stats(hash, generation, |s| {
                        s.uploaded += delta;
                        if is_file && let Some(r) = s.requests.get_mut(&key) {
                            r.sent += delta;
                        }
                    });
                }
            }
            RequestUpdate::Completed(done) => {
                reconcile(
                    &state,
                    hash,
                    generation,
                    counted,
                    done.stats.payload_bytes_sent,
                );
                break;
            }
            RequestUpdate::Aborted(aborted) => {
                reconcile(
                    &state,
                    hash,
                    generation,
                    counted,
                    aborted.stats.payload_bytes_sent,
                );
                break;
            }
        }
    }
    state.forget(&hash, &key);
}

/// Replace the live estimate for one request with the exact number of bytes sent.
fn reconcile(state: &ProviderState, hash: Hash, generation: u64, estimated: u64, exact: u64) {
    state.with_stats(hash, generation, |s| {
        s.uploaded = s.uploaded.saturating_sub(estimated) + exact;
    });
}

/// Periodically publish [`Event::ShareActivity`] for shares whose activity changed.
pub async fn emit_activity(state: Arc<ProviderState>, events: broadcast::Sender<Event>) {
    let mut tick = tokio::time::interval(ACTIVITY_INTERVAL);
    loop {
        tick.tick().await;
        for activity in state.take_changes() {
            events
                .send(Event::ShareActivity {
                    share_id: activity.hash.to_hex().to_string(),
                    active_peers: activity.active_peers,
                    uploaded_bytes: activity.uploaded_bytes,
                    bytes_per_sec: activity.bytes_per_sec,
                    receivers: activity.receivers,
                })
                .ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use iroh_blobs::protocol::ChunkRangesExt;

    use super::*;

    fn chunks(range: std::ops::Range<u64>) -> ChunkRanges {
        <ChunkRanges as ChunkRangesExt>::chunks(range)
    }

    #[test]
    fn range_bytes_handles_partial_and_open_ranges() {
        // 2.5 KiB blob: three chunks, the last one half full.
        let size = 2560;
        assert_eq!(range_bytes(&ChunkRanges::all(), size), size);
        assert_eq!(range_bytes(&ChunkRanges::empty(), size), 0);
        assert_eq!(range_bytes(&chunks(0..1), size), 1024);
        assert_eq!(range_bytes(&chunks(2..3), size), 512);
        assert_eq!(range_bytes(&chunks(1..100), size), 1536);
        assert_eq!(range_bytes(&chunks(5..9), size), 0);
    }

    #[test]
    fn gaps_between_ranges_are_not_counted() {
        // Chunks 0..2 and 10.. of a 20 KiB blob; a write from 2 KiB to 11 KiB crosses the gap.
        let ranges = &chunks(0..2) | &(ChunkRanges::all() - chunks(0..10));
        let size = 20 * 1024;
        assert_eq!(range_bytes(&ranges, size), 2048 + 10 * 1024);
        assert_eq!(range_bytes_within(&ranges, size, 2048, 11 * 1024), 1024);
        assert_eq!(range_bytes_within(&ranges, size, 0, 1024), 1024);
    }

    #[test]
    fn requested_bytes_of_full_resumed_and_index_requests() {
        let sizes = [3000, 10, 5000];
        let all = ChunkRangesSeq::from_ranges_infinite([
            ChunkRanges::all(),
            ChunkRanges::all(),
            ChunkRanges::all(),
        ]);
        assert_eq!(requested_file_bytes(&all, &sizes), 8010);

        // Index only: the hash sequence and the name list.
        let index = ChunkRangesSeq::from_ranges([ChunkRanges::all(), ChunkRanges::all()]);
        assert_eq!(requested_file_bytes(&index, &sizes), 0);

        // Size probe: the last chunk of everything.
        let probe =
            ChunkRangesSeq::from_ranges_infinite([ChunkRanges::all(), ChunkRanges::last_chunk()]);
        assert_eq!(requested_file_bytes(&probe, &sizes), 0);

        // Resumed: the first file is complete, the second missing, the third from chunk 2 on.
        let resumed = ChunkRangesSeq::from_ranges([
            ChunkRanges::empty(),
            ChunkRanges::empty(),
            ChunkRanges::empty(),
            ChunkRanges::all(),
            ChunkRanges::all() - chunks(0..2),
        ]);
        assert_eq!(requested_file_bytes(&resumed, &sizes), 10 + (5000 - 2048));
    }

    #[tokio::test]
    async fn without_a_file_list_any_file_data_counts_as_everything() {
        let dir = tempfile::tempdir().unwrap();
        let state = ProviderState::new(FileLists::new(dir.path().to_path_buf()));
        let hash = Hash::new(b"share");
        state.allow(hash, 1000, None);
        // The first file is complete; a later one is asked for.
        let resumed = ChunkRangesSeq::from_ranges([
            ChunkRanges::empty(),
            ChunkRanges::empty(),
            ChunkRanges::empty(),
            ChunkRanges::all(),
        ]);
        assert_eq!(state.requested_bytes(&hash, &resumed).await, Some(1000));
        let index = ChunkRangesSeq::from_ranges([ChunkRanges::all(), ChunkRanges::all()]);
        assert_eq!(state.requested_bytes(&hash, &index).await, Some(0));
        state.pause(&hash);
        assert_eq!(state.requested_bytes(&hash, &index).await, None);
    }

    #[test]
    fn pausing_keeps_the_total_sent_and_removing_drops_it() {
        let dir = tempfile::tempdir().unwrap();
        let state = ProviderState::new(FileLists::new(dir.path().to_path_buf()));
        let hash = Hash::new(b"share");
        state.allow(hash, 1000, Some(Arc::from([1000u64])));
        let generation = state.generation(&hash).unwrap();
        state.with_stats(hash, generation, |s| {
            s.uploaded = 500;
            s.requests.insert(
                (1, 2),
                RequestStats {
                    receiver_id: 1,
                    requested: 1000,
                    sent: 500,
                    meter: SpeedMeter::new(0, Duration::ZERO),
                    bytes_per_sec: 0,
                    eta_secs: None,
                },
            );
        });
        state.take_changes();

        state.pause(&hash);
        let after = state.take_changes();
        assert_eq!(after.len(), 1, "a last snapshot is published");
        assert_eq!(after[0].uploaded_bytes, 500);
        assert!(after[0].receivers.is_empty());

        // Updates for the old generation are ignored once it was turned off.
        assert!(!state.with_stats(hash, generation, |s| s.uploaded += 1000));
        state.allow(hash, 1000, None);
        let again = state.generation(&hash).unwrap();
        assert_ne!(again, generation, "turning on starts a new generation");
        assert!(!state.with_stats(hash, generation, |s| s.uploaded += 1000));
        state.with_stats(hash, again, |s| s.uploaded += 1);
        assert_eq!(state.take_changes()[0].uploaded_bytes, 501);
        // Serving it again while served keeps the generation (running transfers continue).
        state.allow(hash, 1000, None);
        assert_eq!(state.generation(&hash), Some(again));

        state.disallow(&hash);
        assert!(state.take_changes().is_empty());
        // A request finishing late never brings the stats of a removed share back.
        state.forget(&hash, &(1, 2));
        assert!(!state.with_stats(hash, again, |s| s.uploaded += 1));
        assert!(state.take_changes().is_empty());
    }

    #[tokio::test]
    async fn a_request_admitted_before_turning_off_and_on_is_ended() {
        let dir = tempfile::tempdir().unwrap();
        let state = Arc::new(ProviderState::new(FileLists::new(dir.path().to_path_buf())));
        let hash = Hash::new(b"share");
        state.allow(hash, 1000, Some(Arc::from([1000u64])));
        let all = || {
            ChunkRangesSeq::from_ranges_infinite([
                ChunkRanges::all(),
                ChunkRanges::all(),
                ChunkRanges::all(),
            ])
        };
        let receivers = |state: &ProviderState| {
            state
                .stats
                .lock()
                .unwrap()
                .get(&hash)
                .map_or(0, |s| s.requests.len())
        };

        // Admitted (and so given permission), then the share is turned off and on again
        // before the tracker runs.
        let stale = state.admit(&hash).unwrap();
        state.pause(&hash);
        state.allow(hash, 1000, None);
        let (tx, rx) = irpc::channel::mpsc::channel::<RequestUpdate>(8);
        let request = Request {
            key: (1, 1),
            ranges: all(),
        };
        tokio::time::timeout(
            Duration::from_secs(5),
            track_request(state.clone(), hash, stale, request, rx),
        )
        .await
        .expect("the stale request must end at once");
        assert_eq!(receivers(&state), 0, "a stale request is never registered");
        drop(tx);

        // A request admitted in the current generation is tracked until it ends.
        let current = state.admit(&hash).unwrap();
        let (tx, rx) = irpc::channel::mpsc::channel::<RequestUpdate>(8);
        let request = Request {
            key: (2, 1),
            ranges: all(),
        };
        let task = tokio::spawn(track_request(state.clone(), hash, current, request, rx));
        while receivers(&state) == 0 {
            tokio::task::yield_now().await;
        }
        state.pause(&hash);
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("turning off ends a tracked request")
            .unwrap();
        drop(tx);
    }

    #[test]
    fn first_range_start_is_in_bytes() {
        assert_eq!(first_range_start(&chunks(3..5)), Some(3072));
        assert_eq!(first_range_start(&ChunkRanges::empty()), None);
    }
}
