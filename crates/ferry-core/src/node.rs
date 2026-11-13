//! The Ferry node: one per app instance. Owns the iroh endpoint, the blob store and the registry.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::{Path, PathBuf},
    str::FromStr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::Context;
use futures_buffered::BufferedStreamExt;
use iroh::{Endpoint, RelayMode, endpoint::presets, protocol::Router};
use iroh_blobs::{
    BlobFormat, BlobsProtocol, Hash, HashAndFormat,
    api::{
        blobs::{
            AddPathOptions, AddProgressItem, ExportMode, ExportOptions, ExportProgressItem,
            ImportMode,
        },
        remote::GetProgressItem,
    },
    format::collection::Collection,
    get::request::get_hash_seq_and_sizes,
    protocol::{ChunkRanges, GetRequest},
    provider::events::EventSender,
    store::{
        GcConfig,
        fs::{FsStore, options::Options},
    },
    ticket::BlobTicket,
};
use n0_future::{StreamExt, task::AbortOnDropHandle};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use walkdir::WalkDir;

use crate::{
    config::{Config, RelayChoice},
    events::{ConnectionKind, Event},
    identity,
    link::{FileEntry, ShareFile, ShareLink},
    provider::{self, ProviderState},
    registry::{
        DownloadInfo, DownloadStatus, FileLists, MAX_SKIPPED_LISTED, Registry, ShareInfo,
        SkipReason, SkippedItem,
    },
    util,
};

/// Largest collection index (list of file hashes) we accept from a peer.
const MAX_HASH_SEQ_SIZE: u64 = 32 * 1024 * 1024;
/// Largest file-name list we accept from a peer; it is loaded into memory.
const MAX_META_SIZE: u64 = 16 * 1024 * 1024;
/// How often progress events are emitted at most.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);
/// Garbage collection reclaims data of removed shares and finished downloads.
/// Files shared by reference are never deleted by GC, only Ferry's own copies.
const GC_INTERVAL: Duration = Duration::from_secs(600);
const IMPORT_PARALLELISM: usize = 4;
/// How many file names a preview carries; the UI shows a few, the CLI up to 20.
pub const PREVIEW_FILES: usize = 200;

fn share_tag(hash: &Hash) -> String {
    format!("ferry/share/{}", hash.to_hex())
}

fn download_tag(hash: &Hash) -> String {
    format!("ferry/download/{}", hash.to_hex())
}

/// Accept a hash in hex or base32 form; return it with its canonical (hex) id.
///
/// The shape is checked first: `Hash::from_str` can panic on some malformed input, and ids
/// come from users (CLI arguments) and from files on disk.
fn parse_id(id: &str) -> anyhow::Result<(Hash, String)> {
    let id = id.trim();
    let hex = id.len() == 64 && id.chars().all(|c| c.is_ascii_hexdigit());
    let base32 = id.len() == 52
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || ('2'..='7').contains(&c));
    anyhow::ensure!(hex || base32, "invalid id {id:?}");
    let hash = Hash::from_str(id).context("invalid id")?;
    Ok((hash, hash.to_hex().to_string()))
}

/// Network identity and reachability of this node.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeStatus {
    pub node_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_url: Option<String>,
    pub direct_addresses: Vec<String>,
}

/// What a recipient sees before downloading.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub id: String,
    pub name: String,
    /// The first [`PREVIEW_FILES`] files; `file_count` is the full number.
    pub files: Vec<FileEntry>,
    pub file_count: usize,
    pub total_size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<ConnectionKind>,
}

/// A running download task. The generation tells a task whether it is still the current one
/// for its download, so a task that was paused (aborted) or replaced can't overwrite the state.
struct Running {
    generation: u64,
    _task: AbortOnDropHandle<()>,
}

/// The validated index of a share: names, hashes and sizes, fetched before any file data.
struct Index {
    collection: Collection,
    /// File sizes in collection order.
    sizes: Vec<u64>,
    /// Bytes taken by the index itself (hash list and name list).
    index_bytes: u64,
}

/// What a new share left out, collected while scanning the selection.
#[derive(Default)]
struct Skipped {
    items: Vec<SkippedItem>,
    count: usize,
}

impl Skipped {
    fn add(&mut self, name: String, reason: SkipReason) {
        self.count += 1;
        if self.items.len() < MAX_SKIPPED_LISTED {
            self.items.push(SkippedItem { name, reason });
        }
    }
}

/// Cheap to clone; all clones share the same node.
#[derive(Clone)]
pub struct Node {
    inner: Arc<Inner>,
}

struct Inner {
    router: Router,
    store: FsStore,
    relay: RelayChoice,
    registry: Mutex<Registry>,
    registry_path: PathBuf,
    file_lists: FileLists,
    /// Serializes registry writes so the newest state always lands last.
    save_lock: Mutex<()>,
    provider: Arc<ProviderState>,
    events: broadcast::Sender<Event>,
    downloads: Mutex<HashMap<String, Running>>,
    next_op: AtomicU64,
    next_generation: AtomicU64,
    _tasks: Vec<AbortOnDropHandle<()>>,
}

impl Node {
    /// Start the node: load identity, open the store, start serving active shares and resume
    /// downloads that were in progress when the app last stopped.
    ///
    /// Problems that would otherwise stop the node from ever starting are tolerated: an invalid
    /// custom relay falls back to the default relays, and a damaged registry is set aside.
    pub async fn start(config: Config) -> anyhow::Result<Self> {
        let data_dir = &config.data_dir;
        std::fs::create_dir_all(data_dir)
            .with_context(|| format!("creating {}", data_dir.display()))?;
        let secret = identity::load_or_create_secret(&data_dir.join("secret.key"))?;

        let (relay_mode, relay) = match config.relay.to_relay_mode() {
            Ok(mode) => (mode, config.relay.clone()),
            Err(e) => {
                tracing::warn!("ignoring the custom relay ({e:#}); using the default relays");
                (RelayMode::Default, RelayChoice::Default)
            }
        };
        let builder = if config.address_lookup {
            Endpoint::builder(presets::N0)
        } else {
            Endpoint::builder(presets::Minimal)
        };
        let endpoint = builder
            .secret_key(secret)
            .relay_mode(relay_mode)
            .alpns(vec![iroh_blobs::ALPN.to_vec()])
            .bind()
            .await
            .context("binding network endpoint")?;

        let store_dir = data_dir.join("store");
        std::fs::create_dir_all(&store_dir)?;
        let mut options = Options::new(&store_dir);
        options.gc = Some(GcConfig {
            interval: GC_INTERVAL,
            add_protected: None,
        });
        let store = FsStore::load_with_opts(store_dir.join("blobs.db"), options)
            .await
            .context("opening blob store")?;

        let registry_path = data_dir.join("registry.json");
        let (mut registry, legacy_lists) = Registry::load(&registry_path)?;
        let file_lists = FileLists::new(data_dir.join("shares"));
        // Registries from earlier versions kept every file list inline; move them out.
        let migrated = !legacy_lists.is_empty();
        for (id, files) in &legacy_lists {
            if let Err(e) = file_lists.write(id, files) {
                tracing::warn!(%id, "could not move a share's file list: {e:#}");
            }
        }
        let provider = Arc::new(ProviderState::default());
        registry.shares.retain(|share| match parse_id(&share.id) {
            Ok((hash, _)) => {
                provider.allow(hash);
                true
            }
            Err(_) => {
                tracing::warn!(id = %share.id, "dropping a share with an invalid id");
                false
            }
        });

        let (provider_tx, provider_rx) = tokio::sync::mpsc::channel(64);
        let blobs = BlobsProtocol::new(
            &store,
            Some(EventSender::new(provider_tx, provider::EVENT_MASK)),
        );
        let router = Router::builder(endpoint)
            .accept(iroh_blobs::ALPN, blobs)
            .spawn();

        let (events, _) = broadcast::channel(1024);
        let tasks = vec![
            AbortOnDropHandle::new(tokio::spawn(provider::run(provider.clone(), provider_rx))),
            AbortOnDropHandle::new(tokio::spawn(provider::emit_activity(
                provider.clone(),
                events.clone(),
            ))),
        ];

        let resume: Vec<String> = if config.resume_downloads {
            registry
                .downloads
                .iter()
                .filter(|d| d.status.is_active())
                .map(|d| d.id.clone())
                .collect()
        } else {
            Vec::new()
        };

        let node = Self {
            inner: Arc::new(Inner {
                router,
                store,
                relay,
                registry: Mutex::new(registry),
                registry_path,
                file_lists,
                save_lock: Mutex::new(()),
                provider,
                events,
                downloads: Mutex::new(HashMap::new()),
                next_op: AtomicU64::new(1),
                next_generation: AtomicU64::new(1),
                _tasks: tasks,
            }),
        };
        if migrated {
            node.persist();
        }
        for id in resume {
            tracing::info!(%id, "resuming download");
            node.spawn_download(id);
        }
        Ok(node)
    }

    pub fn endpoint(&self) -> &Endpoint {
        self.inner.router.endpoint()
    }

    /// The relay setting actually in use (an invalid custom relay is replaced by the default).
    pub fn relay(&self) -> &RelayChoice {
        &self.inner.relay
    }

    /// Wait until the node is reachable through its relay (or the timeout passes).
    /// Links created after this include the relay address.
    pub async fn wait_online(&self, timeout: Duration) -> bool {
        tokio::time::timeout(timeout, self.endpoint().online())
            .await
            .is_ok()
    }

    pub fn status(&self) -> NodeStatus {
        let addr = self.endpoint().addr();
        NodeStatus {
            node_id: addr.id.to_string(),
            relay_url: addr.relay_urls().next().map(|u| u.to_string()),
            direct_addresses: addr.ip_addrs().map(|a| a.to_string()).collect(),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.inner.events.subscribe()
    }

    fn emit(&self, event: Event) {
        self.inner.events.send(event).ok();
    }

    /// Write the registry. The snapshot is taken after acquiring the save lock, so the newest
    /// state is always written last even when several tasks save at once.
    fn persist(&self) {
        let _guard = self.inner.save_lock.lock().unwrap();
        let json = self.inner.registry.lock().unwrap().to_json();
        let result = json.and_then(|json| util::write_atomic(&self.inner.registry_path, &json));
        if let Err(e) = result {
            tracing::warn!("could not save the registry: {e:#}");
        }
    }

    /// Commit pending store changes before handing off to an installer that exits the process.
    #[cfg(windows)]
    pub async fn flush_store(&self) -> anyhow::Result<()> {
        self.inner.store.sync_db().await?;
        Ok(())
    }

    /// Stop serving and close the store. Active downloads resume on the next start.
    pub async fn shutdown(&self) -> anyhow::Result<()> {
        self.inner.downloads.lock().unwrap().clear();
        // Shutting down the router also shuts down the blobs protocol and with it the store.
        let router_closed =
            tokio::time::timeout(Duration::from_secs(3), self.inner.router.shutdown())
                .await
                .is_ok_and(|r| r.is_ok());
        if !router_closed {
            // Make sure the store is flushed even if the router did not close cleanly;
            // an error here only means it was already closed.
            self.inner.store.shutdown().await.ok();
        }
        Ok(())
    }

    // ---------------------------------------------------------------- shares

    fn ticket_for(&self, hash: Hash) -> BlobTicket {
        BlobTicket::new(self.endpoint().addr(), hash, BlobFormat::HashSeq)
    }

    /// Fill in the link and ticket with the node's current addresses.
    fn with_link(&self, mut share: ShareInfo) -> ShareInfo {
        if let Ok((hash, _)) = parse_id(&share.id) {
            let ticket = self.ticket_for(hash);
            share.link = ShareLink::new(ticket.clone()).to_url();
            share.ticket = ticket.to_string();
        }
        share
    }

    /// Share files and/or folders. Files are served from where they are (not copied), so they
    /// must not be modified or moved while shared. Empty folders, symlinks and OS junk files
    /// (`.DS_Store`, `Thumbs.db`, …) are skipped.
    pub async fn create_share(&self, paths: Vec<PathBuf>) -> anyhow::Result<ShareInfo> {
        anyhow::ensure!(!paths.is_empty(), "nothing to share");
        let op_id = self.inner.next_op.fetch_add(1, Ordering::Relaxed);

        // Flatten the inputs into (collection name, absolute path, size).
        let mut sources = Vec::new();
        let mut source_paths = Vec::new();
        let mut top_names = Vec::new();
        let mut skipped = Skipped::default();
        for path in &paths {
            let path = dunce_canonicalize(path)?;
            let root = path
                .parent()
                .context("cannot share a filesystem root")?
                .to_path_buf();
            top_names.push(
                path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            );
            let shown = |p: &Path| {
                p.strip_prefix(&root)
                    .unwrap_or(p)
                    .to_string_lossy()
                    .replace('\\', "/")
            };
            let first_source = sources.len();
            let mut dirs = Vec::new();
            // Links are not followed, so nothing outside the selection is ever shared.
            for entry in WalkDir::new(&path).follow_links(false) {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(e) if e.depth() == 0 => {
                        return Err(e).with_context(|| format!("can't read {}", path.display()));
                    }
                    Err(e) => {
                        let name = e.path().map(shown).unwrap_or_else(|| "?".into());
                        skipped.add(name, SkipReason::Unreadable);
                        continue;
                    }
                };
                let file_type = entry.file_type();
                if file_type.is_dir() {
                    dirs.push(entry.path().to_path_buf());
                    continue;
                }
                let reason = if file_type.is_symlink() {
                    Some(SkipReason::Link)
                } else if !file_type.is_file() {
                    Some(SkipReason::NotAFile)
                } else if util::is_os_junk(&entry.file_name().to_string_lossy()) {
                    Some(SkipReason::SystemFile)
                } else {
                    None
                };
                if let Some(reason) = reason {
                    skipped.add(shown(entry.path()), reason);
                    continue;
                }
                let Ok(meta) = entry.metadata() else {
                    skipped.add(shown(entry.path()), SkipReason::Unreadable);
                    continue;
                };
                let name = util::relative_path_to_name(entry.path().strip_prefix(&root)?)?;
                sources.push((name, entry.into_path(), meta.len()));
            }
            // Folders with nothing shared inside; only the outermost one is listed.
            let mut with_files = HashSet::new();
            for (_, file, _) in &sources[first_source..] {
                let mut dir = file.parent();
                while let Some(d) = dir {
                    if !d.starts_with(&path) || !with_files.insert(d.to_path_buf()) {
                        break;
                    }
                    dir = d.parent();
                }
            }
            for dir in &dirs {
                let parent_also_empty = *dir != path
                    && dir
                        .parent()
                        .is_some_and(|p| p.starts_with(&path) && !with_files.contains(p));
                if !with_files.contains(dir) && !parent_also_empty {
                    skipped.add(shown(dir), SkipReason::EmptyFolder);
                }
            }
            source_paths.push(path);
        }
        anyhow::ensure!(
            !sources.is_empty(),
            "there is nothing to share: the selection has no files{}",
            if skipped.count > 0 {
                " (only system files, links or empty folders)"
            } else {
                ""
            }
        );
        sources.sort_by(|a, b| a.0.cmp(&b.0));
        // The recipient checks names with these rules too; fail here so the sender learns why.
        util::validate_names(sources.iter().map(|s| s.0.as_str()))?;
        let total_bytes: u64 = sources.iter().map(|s| s.2).sum();

        // Hash all files, reporting aggregated progress.
        let progress: Arc<Vec<AtomicU64>> =
            Arc::new(sources.iter().map(|_| AtomicU64::new(0)).collect());
        let current = Arc::new(Mutex::new(String::new()));
        let reporter = {
            let progress = progress.clone();
            let current = current.clone();
            let node = self.clone();
            AbortOnDropHandle::new(tokio::spawn(async move {
                let mut tick = tokio::time::interval(PROGRESS_INTERVAL);
                loop {
                    tick.tick().await;
                    let done_bytes = progress.iter().map(|p| p.load(Ordering::Relaxed)).sum();
                    node.emit(Event::ImportProgress {
                        op_id,
                        done_bytes,
                        total_bytes,
                        current_file: current.lock().unwrap().clone(),
                    });
                }
            }))
        };

        let store = self.inner.store.clone();
        let mut entries = n0_future::stream::iter(sources.into_iter().enumerate())
            .map(|(index, (name, path, size))| {
                let store = store.clone();
                let progress = progress.clone();
                let current = current.clone();
                async move {
                    *current.lock().unwrap() = name.clone();
                    let mut stream = store
                        .add_path_with_opts(AddPathOptions {
                            path,
                            mode: ImportMode::TryReference,
                            format: BlobFormat::Raw,
                        })
                        .stream()
                        .await;
                    loop {
                        match stream.next().await {
                            Some(AddProgressItem::OutboardProgress(offset)) => {
                                progress[index].store(offset, Ordering::Relaxed);
                            }
                            Some(AddProgressItem::Done(tag)) => {
                                progress[index].store(size, Ordering::Relaxed);
                                return anyhow::Ok((name, tag, size));
                            }
                            Some(AddProgressItem::Error(e)) => {
                                anyhow::bail!("could not read {name}: {e}")
                            }
                            Some(_) => {}
                            None => anyhow::bail!("import of {name} ended unexpectedly"),
                        }
                    }
                }
            })
            .buffered_unordered(IMPORT_PARALLELISM)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<anyhow::Result<Vec<_>>>()?;
        drop(reporter);

        entries.sort_by(|a, b| a.0.cmp(&b.0));
        let mut collection = Collection::default();
        let mut files = Vec::with_capacity(entries.len());
        for (name, tag, size) in &entries {
            collection.push(name.clone(), tag.hash());
            files.push(FileEntry {
                name: name.clone(),
                size: *size,
            });
        }
        let root = collection
            .store(&self.inner.store)
            .await
            .context("storing the share index")?;
        let hash = root.hash();
        self.inner
            .store
            .tags()
            .set(share_tag(&hash), HashAndFormat::hash_seq(hash))
            .await
            .context("saving the share tag")?;
        drop(root);
        drop(entries); // temp tags: data is now protected by the persistent share tag

        self.emit(Event::ImportProgress {
            op_id,
            done_bytes: total_bytes,
            total_bytes,
            current_file: String::new(),
        });

        let name = match top_names.as_slice() {
            [single] => single.clone(),
            many => format!("{} items", many.len()),
        };
        let share = ShareInfo {
            id: hash.to_hex().to_string(),
            name,
            source_paths,
            file_count: files.len(),
            total_size: total_bytes,
            created_at: util::now_unix(),
            link: String::new(),
            ticket: String::new(),
            skipped: skipped.items,
            skipped_count: skipped.count,
        };
        if share.skipped_count > 0 {
            tracing::info!(count = share.skipped_count, "items left out of the share");
        }
        // The full list goes to its own file once; the registry only keeps the count.
        self.inner
            .file_lists
            .write(&share.id, &files)
            .context("saving the share's file list")?;
        {
            let mut registry = self.inner.registry.lock().unwrap();
            registry.shares.retain(|s| s.id != share.id);
            registry.shares.push(share.clone());
        }
        self.persist();
        self.inner.provider.allow(hash);
        let share = self.with_link(share);
        self.emit(Event::ShareCreated {
            share: share.clone(),
        });
        Ok(share)
    }

    pub fn list_shares(&self) -> Vec<ShareInfo> {
        let shares = self.inner.registry.lock().unwrap().shares.clone();
        shares.into_iter().map(|s| self.with_link(s)).collect()
    }

    /// One share, with a link built from the node's current addresses.
    pub fn share(&self, id: &str) -> Option<ShareInfo> {
        let (_, id) = parse_id(id).ok()?;
        let share = self.inner.registry.lock().unwrap().share(&id).cloned();
        share.map(|s| self.with_link(s))
    }

    /// The full file list of a share (read from disk; can be large).
    pub fn share_files(&self, id: &str) -> anyhow::Result<Vec<FileEntry>> {
        let share = self.share(id).context("unknown share")?;
        self.inner.file_lists.read(&share.id)
    }

    /// Contents for a `.ferry` file of the given share.
    pub fn share_file(&self, id: &str) -> anyhow::Result<ShareFile> {
        let share = self.share(id).context("unknown share")?;
        let ticket = BlobTicket::from_str(&share.ticket)?;
        let files = self.inner.file_lists.read(&share.id).unwrap_or_default();
        Ok(ShareFile::new(
            &ticket,
            share.name,
            share.total_size,
            &files,
        ))
    }

    /// Stop serving a share. New requests are refused and running transfers are ended.
    /// The original files are left untouched.
    pub async fn remove_share(&self, id: &str) -> anyhow::Result<()> {
        let (hash, id) = parse_id(id).context("invalid share id")?;
        anyhow::ensure!(
            self.inner.registry.lock().unwrap().share(&id).is_some(),
            "unknown share"
        );
        self.inner.provider.disallow(&hash);
        self.inner.store.tags().delete(share_tag(&hash)).await?;
        self.inner
            .registry
            .lock()
            .unwrap()
            .shares
            .retain(|s| s.id != id);
        self.persist();
        self.inner.file_lists.remove(&id);
        self.emit(Event::ShareRemoved { share_id: id });
        Ok(())
    }

    // ------------------------------------------------------------- downloads

    async fn connect(&self, ticket: &BlobTicket) -> anyhow::Result<iroh::endpoint::Connection> {
        self.endpoint()
            .connect(ticket.addr().clone(), iroh_blobs::ALPN)
            .await
            .context("could not reach the sender (is their Ferry running?)")
    }

    /// Fetch and check a share's index (file names and sizes) without any file data.
    /// Rejects oversized indexes and names that can't be saved safely on every platform.
    async fn fetch_index(
        &self,
        conn: &iroh::endpoint::Connection,
        hash: Hash,
    ) -> anyhow::Result<Index> {
        let (hash_seq, sizes) = get_hash_seq_and_sizes(conn, &hash, MAX_HASH_SEQ_SIZE, None)
            .await
            .context("the sender refused or no longer has this share")?;
        anyhow::ensure!(!sizes.is_empty(), "the share is empty or malformed");
        anyhow::ensure!(
            sizes[0] <= MAX_META_SIZE,
            "the share's file list is too large ({} bytes)",
            sizes[0]
        );
        let request = GetRequest::builder()
            .root(ChunkRanges::all())
            .child(0, ChunkRanges::all())
            .build(hash);
        self.inner
            .store
            .remote()
            .execute_get(conn.clone(), request)
            .complete()
            .await
            .context("could not fetch the file list")?;
        let collection = Collection::load(hash, self.inner.store.as_ref()).await?;
        anyhow::ensure!(
            collection.len() + 1 == sizes.len(),
            "the share's file list does not match its contents"
        );
        util::validate_names(collection.iter().map(|(n, _)| n.as_str()))?;
        Ok(Index {
            collection,
            sizes: sizes[1..].to_vec(),
            index_bytes: 32 * hash_seq.len() as u64 + sizes[0],
        })
    }

    /// Fetch the file list and sizes of a share without downloading the files.
    pub async fn preview(&self, link: &ShareLink) -> anyhow::Result<Preview> {
        let hash = link.ticket.hash();
        let conn = self.connect(&link.ticket).await?;
        let connection = selected_path(&conn);
        let index = self.fetch_index(&conn, hash).await;
        conn.close(0u32.into(), b"done");
        let index = index?;
        // Only a sample of names is sent on; a share can hold hundreds of thousands of files.
        let files: Vec<FileEntry> = index
            .collection
            .iter()
            .zip(index.sizes.iter())
            .take(PREVIEW_FILES)
            .map(|((name, _), size)| FileEntry {
                name: name.clone(),
                size: *size,
            })
            .collect();
        Ok(Preview {
            id: hash.to_hex().to_string(),
            name: display_name(index.collection.iter().map(|(n, _)| n.as_str())),
            files,
            file_count: index.collection.len(),
            total_size: index.sizes.iter().sum(),
            connection,
        })
    }

    pub fn list_downloads(&self) -> Vec<DownloadInfo> {
        self.inner.registry.lock().unwrap().downloads.clone()
    }

    fn is_running(&self, id: &str) -> bool {
        self.inner.downloads.lock().unwrap().contains_key(id)
    }

    /// Start (or restart) downloading a share into `dest_dir`.
    pub async fn start_download(
        &self,
        link: &ShareLink,
        dest_dir: PathBuf,
    ) -> anyhow::Result<DownloadInfo> {
        // Saving needs an absolute path; resolve it now, not after the whole transfer.
        let dest_dir = std::path::absolute(&dest_dir).context("invalid destination folder")?;
        let hash = link.ticket.hash();
        let id = hash.to_hex().to_string();
        if self.is_running(&id) {
            return self
                .download(&id)
                .context("download disappeared while starting");
        }
        std::fs::create_dir_all(&dest_dir)
            .with_context(|| format!("creating {}", dest_dir.display()))?;
        // Protect partial data from garbage collection until the download is finished.
        self.inner
            .store
            .tags()
            .set(download_tag(&hash), HashAndFormat::hash_seq(hash))
            .await?;
        let info = DownloadInfo {
            id: id.clone(),
            ticket: link.ticket.to_string(),
            dest_dir,
            name: String::new(),
            status: DownloadStatus::Connecting,
            done_bytes: 0,
            total_bytes: 0,
            created_at: util::now_unix(),
            saved_path: None,
            error: None,
            export_names: None,
            bytes_per_sec: 0,
            eta_secs: None,
            connection: None,
        };
        {
            let mut registry = self.inner.registry.lock().unwrap();
            registry.downloads.retain(|d| d.id != id);
            registry.downloads.push(info.clone());
        }
        self.persist();
        self.emit(Event::DownloadUpdated {
            download: info.clone(),
        });
        self.spawn_download(id);
        Ok(info)
    }

    /// Resume a paused or failed download. Already received data is not fetched again.
    pub async fn resume_download(&self, id: &str) -> anyhow::Result<()> {
        let (hash, id) = parse_id(id).context("invalid download id")?;
        let info = self.download(&id).context("unknown download")?;
        anyhow::ensure!(
            info.status != DownloadStatus::Completed,
            "this download has already finished"
        );
        if self.is_running(&id) {
            return Ok(());
        }
        self.inner
            .store
            .tags()
            .set(download_tag(&hash), HashAndFormat::hash_seq(hash))
            .await?;
        self.update_download(&id, true, |d| {
            d.status = DownloadStatus::Connecting;
            d.error = None;
        });
        self.spawn_download(id);
        Ok(())
    }

    /// Pause a download; received data is kept for resuming.
    pub fn pause_download(&self, id: &str) {
        let Ok((_, id)) = parse_id(id) else { return };
        let task = self.inner.downloads.lock().unwrap().remove(&id);
        if task.is_some() {
            self.update_download(&id, true, |d| {
                d.status = DownloadStatus::Paused;
                d.bytes_per_sec = 0;
                d.eta_secs = None;
                d.connection = None;
            });
        }
    }

    /// Forget a download and discard partial data. Saved files are left untouched.
    pub async fn remove_download(&self, id: &str) -> anyhow::Result<()> {
        let (hash, id) = parse_id(id).context("invalid download id")?;
        anyhow::ensure!(self.download(&id).is_some(), "unknown download");
        self.inner.downloads.lock().unwrap().remove(&id);
        self.inner.store.tags().delete(download_tag(&hash)).await?;
        self.inner
            .registry
            .lock()
            .unwrap()
            .downloads
            .retain(|d| d.id != id);
        self.persist();
        self.emit(Event::DownloadRemoved { download_id: id });
        Ok(())
    }

    pub fn download(&self, id: &str) -> Option<DownloadInfo> {
        let (_, id) = parse_id(id).ok()?;
        let mut registry = self.inner.registry.lock().unwrap();
        registry.download_mut(&id).map(|d| d.clone())
    }

    fn update_download(&self, id: &str, persist: bool, f: impl FnOnce(&mut DownloadInfo)) {
        let snapshot = {
            let mut registry = self.inner.registry.lock().unwrap();
            let Some(download) = registry.download_mut(id) else {
                return;
            };
            f(download);
            download.clone()
        };
        if persist {
            self.persist();
        }
        self.emit(Event::DownloadUpdated { download: snapshot });
    }

    /// Update a download only if the calling task is still its current task.
    fn update_if_current(
        &self,
        id: &str,
        generation: u64,
        persist: bool,
        f: impl FnOnce(&mut DownloadInfo),
    ) -> bool {
        let downloads = self.inner.downloads.lock().unwrap();
        if downloads.get(id).map(|r| r.generation) != Some(generation) {
            return false;
        }
        self.update_download(id, persist, f);
        true
    }

    fn spawn_download(&self, id: String) {
        let generation = self.inner.next_generation.fetch_add(1, Ordering::Relaxed);
        let node = self.clone();
        let task_id = id.clone();
        // Hold the lock while spawning: the task can't finish (and look itself up) before its
        // entry exists.
        let mut downloads = self.inner.downloads.lock().unwrap();
        let handle = tokio::spawn(async move {
            let result = node.run_download(&task_id, generation).await;
            node.finish_download(&task_id, generation, result);
        });
        downloads.insert(
            id,
            Running {
                generation,
                _task: AbortOnDropHandle::new(handle),
            },
        );
    }

    fn finish_download(&self, id: &str, generation: u64, result: anyhow::Result<PathBuf>) {
        let mut downloads = self.inner.downloads.lock().unwrap();
        if downloads.get(id).map(|r| r.generation) != Some(generation) {
            return; // paused, removed or replaced meanwhile
        }
        let _this_task = downloads.remove(id);
        match result {
            Ok(saved) => self.update_download(id, true, |d| {
                d.status = DownloadStatus::Completed;
                d.saved_path = Some(saved);
                d.done_bytes = d.total_bytes;
                d.bytes_per_sec = 0;
                d.eta_secs = None;
                d.error = None;
            }),
            Err(e) => {
                tracing::warn!(%id, "download failed: {e:#}");
                self.update_download(id, true, |d| {
                    d.status = DownloadStatus::Failed;
                    d.error = Some(format!("{e:#}"));
                    d.bytes_per_sec = 0;
                    d.eta_secs = None;
                })
            }
        }
    }

    async fn run_download(&self, id: &str, generation: u64) -> anyhow::Result<PathBuf> {
        let info = self.download(id).context("unknown download")?;
        let ticket = BlobTicket::from_str(&info.ticket)?;
        let hash = ticket.hash();
        let content = ticket.hash_and_format();
        let store = &self.inner.store;

        let local = store.remote().local(content).await?;
        let collection = if local.is_complete() {
            let collection = Collection::load(hash, store.as_ref()).await?;
            util::validate_names(collection.iter().map(|(n, _)| n.as_str()))?;
            collection
        } else {
            self.update_if_current(id, generation, true, |d| {
                d.status = DownloadStatus::Connecting
            });
            let conn = self.connect(&ticket).await?;
            let _watcher = self.watch_connection(id, generation, conn.clone());
            // Names and sizes are checked before any file data is transferred.
            let index = self.fetch_index(&conn, hash).await?;
            let name = display_name(index.collection.iter().map(|(n, _)| n.as_str()));
            let total: u64 = index.sizes.iter().sum();
            let local = store.remote().local(content).await?;
            let already = local
                .local_bytes()
                .saturating_sub(index.index_bytes)
                .min(total);
            self.update_if_current(id, generation, true, |d| {
                d.status = DownloadStatus::Downloading;
                d.name = name;
                d.total_bytes = total;
                d.done_bytes = already;
            });

            let mut meter = SpeedMeter::new(already);
            let mut stream = store.remote().execute_get(conn, local.missing()).stream();
            while let Some(item) = stream.next().await {
                match item {
                    GetProgressItem::Progress(offset) => {
                        let done = (already + offset).min(total);
                        if let Some((speed, eta)) = meter.sample(done, total) {
                            self.update_if_current(id, generation, false, |d| {
                                d.done_bytes = done;
                                d.bytes_per_sec = speed;
                                d.eta_secs = eta;
                            });
                        }
                    }
                    GetProgressItem::Done(_) => break,
                    GetProgressItem::Error(e) => {
                        return Err(anyhow::Error::new(e).context("transfer interrupted"));
                    }
                }
            }
            index.collection
        };

        self.update_if_current(id, generation, true, |d| {
            d.status = DownloadStatus::Exporting;
            d.bytes_per_sec = 0;
            d.eta_secs = None;
            if d.name.is_empty() {
                d.name = display_name(collection.iter().map(|(n, _)| n.as_str()));
            }
        });
        let saved = self
            .export(id, generation, &collection, &info.dest_dir)
            .await?;
        // Our copy of the data is no longer needed; GC reclaims it.
        store.tags().delete(download_tag(&hash)).await?;
        Ok(saved)
    }

    /// Save a downloaded collection into `dest`. Existing files are never overwritten: a
    /// clashing top-level file or folder gets a ` (n)` suffix. The chosen names are stored, so
    /// a retried save writes to the same place. Data is moved out of the store when possible
    /// (same drive), otherwise copied.
    async fn export(
        &self,
        id: &str,
        generation: u64,
        collection: &Collection,
        dest: &Path,
    ) -> anyhow::Result<PathBuf> {
        let tops: BTreeSet<&str> = collection
            .iter()
            .map(|(n, _)| n.split('/').next().unwrap_or(n))
            .collect();
        let saved_plan = self.download(id).and_then(|d| d.export_names);
        let plan = match saved_plan {
            Some(plan) if tops.iter().all(|t| plan.contains_key(*t)) => plan,
            _ => {
                let mut plan = BTreeMap::new();
                let mut taken = HashSet::new();
                for top in &tops {
                    let target = util::non_conflicting(&util::safe_join(dest, top)?, &taken);
                    let chosen = target
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| top.to_string());
                    util::validate_name(&chosen)?;
                    taken.insert(chosen.to_lowercase());
                    plan.insert(top.to_string(), chosen);
                }
                self.update_if_current(id, generation, true, |d| {
                    d.export_names = Some(plan.clone())
                });
                plan
            }
        };
        for (name, hash) in collection.iter() {
            let (top, rest) = match name.split_once('/') {
                Some((top, rest)) => (top, Some(rest)),
                None => (name.as_str(), None),
            };
            let mapped = match rest {
                Some(rest) => format!("{}/{rest}", plan[top]),
                None => plan[top].clone(),
            };
            let target = util::safe_join(dest, &mapped)?;
            if target
                .symlink_metadata()
                .is_ok_and(|m| m.file_type().is_symlink())
            {
                anyhow::bail!("refusing to write through a link at {}", target.display());
            }
            if let Some(parent) = target.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            let mut stream = self
                .inner
                .store
                .export_with_opts(ExportOptions {
                    hash: *hash,
                    target,
                    mode: ExportMode::TryReference,
                })
                .stream()
                .await;
            while let Some(item) = stream.next().await {
                if let ExportProgressItem::Error(e) = item {
                    anyhow::bail!("could not save {name}: {e}");
                }
            }
        }
        Ok(match plan.values().collect::<Vec<_>>().as_slice() {
            [single] => dest.join(single),
            _ => dest.to_path_buf(),
        })
    }

    /// Report whether the transfer runs direct or through a relay, as it changes.
    fn watch_connection(
        &self,
        id: &str,
        generation: u64,
        conn: iroh::endpoint::Connection,
    ) -> AbortOnDropHandle<()> {
        let node = self.clone();
        let id = id.to_string();
        AbortOnDropHandle::new(tokio::spawn(async move {
            let mut paths = conn.paths_stream();
            let mut last = None;
            while let Some(list) = paths.next().await {
                let kind = (&list)
                    .into_iter()
                    .find(|p| p.is_selected())
                    .map(|p| connection_kind(p.is_relay()));
                if kind.is_some() && kind != last {
                    last = kind;
                    node.update_if_current(&id, generation, false, |d| d.connection = kind);
                }
            }
        }))
    }
}

fn connection_kind(is_relay: bool) -> ConnectionKind {
    if is_relay {
        ConnectionKind::Relay
    } else {
        ConnectionKind::Direct
    }
}

fn selected_path(conn: &iroh::endpoint::Connection) -> Option<ConnectionKind> {
    conn.paths()
        .into_iter()
        .find(|p| p.is_selected())
        .map(|p| connection_kind(p.is_relay()))
}

/// The top-level file or folder name, or "N items" when there are several.
fn display_name<'a>(names: impl Iterator<Item = &'a str>) -> String {
    let tops: BTreeSet<&str> = names.map(|n| n.split('/').next().unwrap_or(n)).collect();
    match tops.len() {
        0 => "empty share".to_string(),
        1 => tops.into_iter().next().unwrap_or_default().to_string(),
        n => format!("{n} items"),
    }
}

/// `std::fs::canonicalize` without the `\\?\` prefix on Windows, so names stay readable.
fn dunce_canonicalize(path: &Path) -> anyhow::Result<PathBuf> {
    let canonical = std::fs::canonicalize(path)
        .with_context(|| format!("{} does not exist", path.display()))?;
    #[cfg(windows)]
    {
        let text = canonical.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\")
            && !rest.starts_with("UNC\\")
        {
            return Ok(PathBuf::from(rest));
        }
    }
    Ok(canonical)
}

/// Smoothed transfer speed and ETA. The first sample is reported right away, later ones at
/// most every [`PROGRESS_INTERVAL`].
struct SpeedMeter {
    last_time: Instant,
    last_bytes: u64,
    speed: f64,
    reported: bool,
}

impl SpeedMeter {
    fn new(start_bytes: u64) -> Self {
        Self {
            last_time: Instant::now(),
            last_bytes: start_bytes,
            speed: 0.0,
            reported: false,
        }
    }

    fn sample(&mut self, done: u64, total: u64) -> Option<(u64, Option<u64>)> {
        let elapsed = self.last_time.elapsed();
        if self.reported && elapsed < PROGRESS_INTERVAL && done < total {
            return None;
        }
        if elapsed < Duration::from_millis(1) {
            return None; // too short to measure a speed
        }
        let instant = done.saturating_sub(self.last_bytes) as f64 / elapsed.as_secs_f64();
        self.speed = if self.speed == 0.0 {
            instant
        } else {
            0.7 * self.speed + 0.3 * instant
        };
        self.reported = true;
        self.last_time = Instant::now();
        self.last_bytes = done;
        let eta = (self.speed >= 1.0)
            .then(|| (total.saturating_sub(done) as f64 / self.speed).ceil() as u64);
        Some((self.speed as u64, eta))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod resume;

    #[test]
    fn display_name_counts_distinct_top_levels() {
        assert_eq!(display_name(["b/x", "a", "b/y"].into_iter()), "2 items");
        assert_eq!(
            display_name(["Photos/1.jpg", "Photos/2.jpg"].into_iter()),
            "Photos"
        );
        assert_eq!(display_name(std::iter::empty()), "empty share");
    }

    #[test]
    fn malformed_ids_are_errors_not_panics() {
        for bad in [
            "",
            "not a hash",
            "zz",
            &"g".repeat(64),
            &"A".repeat(52),
            "abc{e9}",
        ] {
            assert!(parse_id(bad).is_err(), "{bad:?}");
        }
        let hash = Hash::new(b"x");
        let (_, from_hex) = parse_id(&hash.to_hex()).unwrap();
        assert_eq!(from_hex, hash.to_hex().to_string());
    }

    #[test]
    fn speed_meter_never_reports_infinite_speed() {
        let mut meter = SpeedMeter::new(0);
        // Completion right away: elapsed is (nearly) zero.
        if let Some((speed, _)) = meter.sample(100, 100) {
            assert!(speed < u64::MAX);
        }
    }
}
