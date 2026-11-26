//! The Ferry node: one per app instance. Owns the iroh endpoint, the blob store and the registry.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::{Path, PathBuf},
    str::FromStr,
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::Context;
use futures_buffered::BufferedStreamExt;
use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMode, RelayUrl, Watcher, endpoint::presets,
    protocol::Router,
};
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
};
use n0_future::{StreamExt, task::AbortOnDropHandle};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use walkdir::WalkDir;

use crate::{
    access::{self, GuessLimiter},
    config::{Config, RelayChoice},
    events::{ConnectionKind, Event},
    identity,
    link::{FileEntry, ShareLink, ShareToken},
    links_service::{LINK_LIFETIME, LinkService},
    protocol::{self, LanAddr, MAX_LAN_ADDRS, OpenRequest, OpenResponse},
    provider::{self, ProviderState},
    registry::{
        DownloadInfo, DownloadStatus, FileLists, ListedFile, MAX_SKIPPED_LISTED, Registry,
        ShareAccess, ShareInfo, SkipReason, SkippedItem,
    },
    speed::SpeedMeter,
    ticket_file, util,
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
/// How often short links are registered again, so they never expire while a share exists.
const LINK_REFRESH: Duration = Duration::from_secs(12 * 60 * 60);
/// How soon a failed registration is tried again at first; the wait doubles up to an hour.
const LINK_RETRY: Duration = Duration::from_secs(120);
const LINK_RETRY_MAX: Duration = Duration::from_secs(60 * 60);
/// How long a direct connection on the local network may take before the relay is used.
const LAN_TIMEOUT: Duration = Duration::from_secs(2);
/// The pause between registrations (and removals), so a device with many shares stays under the
/// service's limit of 10 a minute per network, with room to spare for other devices there.
const LINK_PACE: Duration = Duration::from_secs(7);

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

/// A device id as stored in the registry (hex).
fn device_hex(device: &EndpointId) -> String {
    hex::encode(device.as_bytes())
}

fn parse_device(text: &str) -> Option<EndpointId> {
    let bytes: [u8; 32] = hex::decode(text).ok()?.try_into().ok()?;
    EndpointId::from_bytes(&bytes).ok()
}

/// Why the sender did not hand over a share. Callers can tell these apart (with
/// `anyhow::Error::downcast_ref`) to ask for a password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessError {
    /// The share is locked; ask for its password.
    PasswordRequired,
    WrongPassword,
    /// Too many wrong passwords were tried; the sender takes no more for a while.
    TooManyGuesses {
        retry_after_secs: u64,
    },
}

impl std::fmt::Display for AccessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PasswordRequired => f.write_str("this share is protected with a password"),
            Self::WrongPassword => f.write_str("that password is not right"),
            Self::TooManyGuesses { retry_after_secs } => write!(
                f,
                "too many wrong passwords; try again in {retry_after_secs} seconds"
            ),
        }
    }
}

impl std::error::Error for AccessError {}

/// Lets the `ferry/1` protocol reach the node that serves it (set once the node exists).
#[derive(Default)]
struct Handle(OnceLock<Weak<Inner>>);

impl protocol::Handler for Handle {
    async fn open(&self, remote: EndpointId, request: OpenRequest) -> OpenResponse {
        match self.0.get().and_then(Weak::upgrade) {
            Some(inner) => Node { inner }.handle_open(remote, request).await,
            None => OpenResponse::Unavailable,
        }
    }
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
    /// With [`Config::hide_ip`]: a second endpoint for direct connections on the local
    /// network. It has no relays and never learns a public address.
    lan_router: Option<Router>,
    hide_ip: bool,
    lan_loopback: bool,
    lan_direct: bool,
    store: FsStore,
    relay: RelayChoice,
    registry: Mutex<Registry>,
    registry_path: PathBuf,
    file_lists: FileLists,
    /// Serializes registry writes so the newest state always lands last.
    save_lock: Mutex<()>,
    /// Serializes changes to shares (creating, removing, turning on or off), from the registry
    /// change through the tag and file-list writes, saving and the event. Two changes to the
    /// same share (it is content-addressed, so creating the same files again gives the same
    /// id) can then never interleave: a removal can't delete the tag or file list of a share
    /// created again meanwhile, and events arrive in the order the changes were made.
    share_ops: tokio::sync::Mutex<()>,
    provider: Arc<ProviderState>,
    /// Limits password guesses per share.
    guesses: GuessLimiter,
    /// The short-link service, if short links are used.
    links: Option<LinkService>,
    /// Shares whose short code the link service knows, so their link can be the short one.
    short_links: Mutex<HashSet<String>>,
    /// Devices can be found without addresses in links (see [`Config::address_book`]).
    has_address_book: bool,
    /// Tasks that hold only a weak reference to the node.
    background: Mutex<Vec<AbortOnDropHandle<()>>>,
    /// Requests to the link service that are under way; shutting down lets them finish.
    link_jobs: Mutex<Vec<AbortOnDropHandle<()>>>,
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
        anyhow::ensure!(
            !(config.hide_ip && matches!(relay, RelayChoice::Disabled)),
            "Hide my IP address needs relays: turn relays on, or stop hiding your IP address"
        );
        let mut builder = if config.address_lookup {
            Endpoint::builder(presets::N0)
        } else {
            Endpoint::builder(presets::Minimal)
        };
        if let Some(book) = &config.address_book {
            builder = builder.address_lookup(book.clone());
        }
        if config.hide_ip {
            // Relay only: no direct addresses, so no hole punching that would tell peers
            // this device's IP address.
            builder = builder.clear_ip_transports();
        }
        let endpoint = builder
            .secret_key(secret)
            .relay_mode(relay_mode)
            .alpns(vec![iroh_blobs::ALPN.to_vec(), protocol::ALPN.to_vec()])
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
        for (id, files) in legacy_lists.iter().cloned() {
            let listed: Vec<ListedFile> = files.into_iter().map(ListedFile::from).collect();
            if let Err(e) = file_lists.write(&id, &listed) {
                tracing::warn!(%id, "could not move a share's file list: {e:#}");
            }
        }
        // Every share has its own token; the access of shares that are gone is dropped.
        let mut access_changed = false;
        for share in &registry.shares {
            if !registry.access.contains_key(&share.id) {
                registry
                    .access
                    .insert(share.id.clone(), ShareAccess::generate());
                access_changed = true;
            }
        }
        let provider = Arc::new(ProviderState::new(file_lists.clone()));
        registry.shares.retain(|share| match parse_id(&share.id) {
            Ok((hash, _)) => {
                if !share.paused {
                    // File sizes are read from the share's list when first needed.
                    provider.allow(hash, share.total_size, None);
                }
                true
            }
            Err(_) => {
                tracing::warn!(id = %share.id, "dropping a share with an invalid id");
                false
            }
        });
        let ids: HashSet<String> = registry.shares.iter().map(|s| s.id.clone()).collect();
        let before = registry.access.len();
        registry.access.retain(|id, _| ids.contains(id));
        access_changed |= registry.access.len() != before;
        for (id, access) in &registry.access {
            if let Ok((hash, _)) = parse_id(id) {
                provider.set_grants(hash, access.grants.iter().filter_map(|d| parse_device(d)));
            }
        }

        let (provider_tx, provider_rx) = tokio::sync::mpsc::channel(64);
        let blobs = BlobsProtocol::new(
            &store,
            Some(EventSender::new(provider_tx, provider::EVENT_MASK)),
        );
        let links = match &config.links_service {
            Some(base) => match LinkService::new(base, endpoint.tls_config().clone()) {
                Ok(service) => Some(service),
                Err(e) => {
                    tracing::warn!("short links are off: {e:#}");
                    None
                }
            },
            None => None,
        };
        let mut background = Vec::new();
        if let Some(book) = config.address_book.clone() {
            let mut addrs = endpoint.watch_addr().stream();
            background.push(AbortOnDropHandle::new(tokio::spawn(async move {
                while let Some(addr) = addrs.next().await {
                    book.add_endpoint_info(addr);
                }
            })));
        }
        let handle = Arc::new(Handle::default());
        let router = Router::builder(endpoint)
            .accept(iroh_blobs::ALPN, blobs)
            .accept(protocol::ALPN, protocol::Protocol::new(handle.clone()))
            .spawn();
        let mut lan_rx = None;
        let lan_router = if config.hide_ip {
            let lan_endpoint = Endpoint::builder(presets::Minimal)
                .secret_key(identity::load_or_create_secret(&data_dir.join("lan.key"))?)
                .relay_mode(RelayMode::Disabled)
                .clear_relay_transports()
                .clear_ip_transports()
                // IPv4 only: local IPv6 addresses are usually global ones.
                .bind_addr("0.0.0.0:0")
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .portmapper_config(iroh::endpoint::PortmapperConfig::Disabled)
                .alpns(vec![iroh_blobs::ALPN.to_vec(), protocol::ALPN.to_vec()])
                .bind()
                .await
                .context("binding the local network endpoint")?;
            let (tx, rx) = tokio::sync::mpsc::channel(64);
            lan_rx = Some(rx);
            let lan_blobs =
                BlobsProtocol::new(&store, Some(EventSender::new(tx, provider::EVENT_MASK)));
            Some(
                Router::builder(lan_endpoint)
                    .accept(iroh_blobs::ALPN, SharedStore(lan_blobs))
                    .accept(protocol::ALPN, protocol::Protocol::new(handle.clone()))
                    .spawn(),
            )
        } else {
            None
        };

        let (events, _) = broadcast::channel(1024);
        let mut tasks = vec![
            AbortOnDropHandle::new(tokio::spawn(provider::run(
                provider.clone(),
                0,
                provider_rx,
            ))),
            AbortOnDropHandle::new(tokio::spawn(provider::emit_activity(
                provider.clone(),
                events.clone(),
            ))),
        ];

        if let Some(rx) = lan_rx {
            tasks.push(AbortOnDropHandle::new(tokio::spawn(provider::run(
                provider.clone(),
                1,
                rx,
            ))));
        }

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
                lan_router,
                hide_ip: config.hide_ip,
                lan_loopback: config.lan_loopback,
                lan_direct: config.lan_direct,
                store,
                relay,
                registry: Mutex::new(registry),
                registry_path,
                file_lists,
                save_lock: Mutex::new(()),
                share_ops: tokio::sync::Mutex::new(()),
                provider,
                guesses: GuessLimiter::default(),
                links,
                short_links: Mutex::new(HashSet::new()),
                has_address_book: config.address_book.is_some(),
                background: Mutex::new(background),
                link_jobs: Mutex::new(Vec::new()),
                events,
                downloads: Mutex::new(HashMap::new()),
                next_op: AtomicU64::new(1),
                next_generation: AtomicU64::new(1),
                _tasks: tasks,
            }),
        };
        handle.0.set(Arc::downgrade(&node.inner)).ok();
        if migrated || access_changed {
            node.persist();
        }
        if node.short_links_on() {
            let refresh = tokio::spawn(refresh_short_links(Arc::downgrade(&node.inner)));
            node.inner
                .background
                .lock()
                .unwrap()
                .push(AbortOnDropHandle::new(refresh));
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

    /// Whether this device hides its IP address (see [`Config::hide_ip`]).
    pub fn hides_ip(&self) -> bool {
        self.inner.hide_ip
    }

    /// The endpoint that makes direct connections on the local network: the separate one
    /// while hiding the IP address, the main one otherwise.
    fn lan_endpoint(&self) -> &Endpoint {
        self.inner
            .lan_router
            .as_ref()
            .map_or_else(|| self.endpoint(), |r| r.endpoint())
    }

    /// Where recipients on the same local network can reach this device directly.
    fn lan_offer(&self) -> Option<LanAddr> {
        let endpoint = self.lan_endpoint();
        let addrs: Vec<_> = endpoint
            .addr()
            .ip_addrs()
            .filter(|a| protocol::is_lan_ip(a.ip(), self.inner.lan_loopback))
            .copied()
            .take(MAX_LAN_ADDRS)
            .collect();
        (!addrs.is_empty()).then(|| LanAddr {
            id: endpoint.id(),
            addrs,
        })
    }

    /// Whether this node talks to the short-link service (to make short links and open them).
    pub fn uses_link_service(&self) -> bool {
        self.inner.links.is_some()
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

    /// Write the registry, logging a failure. See [`Self::try_persist`].
    fn persist(&self) {
        if let Err(e) = self.try_persist() {
            tracing::warn!("could not save the registry: {e:#}");
        }
    }

    /// Write the registry. The snapshot is taken after acquiring the save lock, so the newest
    /// state is always written last even when several tasks save at once.
    fn try_persist(&self) -> anyhow::Result<()> {
        let _guard = self.inner.save_lock.lock().unwrap();
        let json = self.inner.registry.lock().unwrap().to_json()?;
        // It holds the shares' tokens and password hashes.
        util::write_atomic_private(&self.inner.registry_path, &json)
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
        // A short link being removed (or registered) gets a moment to reach the service.
        let jobs = std::mem::take(&mut *self.inner.link_jobs.lock().unwrap());
        tokio::time::timeout(Duration::from_secs(5), futures_buffered::join_all(jobs))
            .await
            .ok();
        // Shutting down the router also shuts down the blobs protocol and with it the store.
        if let Some(lan) = &self.inner.lan_router {
            tokio::time::timeout(Duration::from_secs(3), lan.shutdown())
                .await
                .ok();
        }
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

    /// Without relays (and outside tests), a recipient can only find this device through
    /// addresses in the link.
    fn links_need_addresses(&self) -> bool {
        matches!(self.inner.relay, RelayChoice::Disabled) && !self.inner.has_address_book
    }

    /// Whether shares get short links (once the link service has accepted them).
    pub fn short_links_on(&self) -> bool {
        self.inner.links.is_some() && !self.links_need_addresses()
    }

    /// The full link of a share. It carries this device's addresses only when no relays are
    /// used, since a recipient could not find the device otherwise.
    fn link_for(&self, token: ShareToken) -> ShareLink {
        let id = self.endpoint().id();
        if self.links_need_addresses() {
            let addr = self.endpoint().addr();
            let addrs: Vec<_> = addr.ip_addrs().copied().collect();
            ShareLink::with_addrs(id, token, &addrs)
        } else {
            ShareLink::direct(id, token)
        }
    }

    /// Fill in the link, code and lock state of a share.
    fn with_link(&self, mut share: ShareInfo) -> ShareInfo {
        let access = self
            .inner
            .registry
            .lock()
            .unwrap()
            .access
            .get(&share.id)
            .cloned();
        if let Some(access) = access {
            let link = self.link_for(access.token);
            share.code = link.code();
            let short = self.inner.short_links.lock().unwrap().contains(&share.id);
            share.link = if short {
                ShareLink::Code(share.code.clone()).to_url()
            } else {
                link.to_url()
            };
            share.locked = access.password.is_some();
        }
        share
    }

    /// Register a share's short code with the link service, in the background. Once that
    /// worked, the share's link is the short one and [`Event::ShareUpdated`] says so.
    fn register_short_link(&self, id: String) {
        if !self.short_links_on() {
            return;
        }
        let node = Arc::downgrade(&self.inner);
        let task = tokio::spawn(async move {
            let Some(inner) = node.upgrade() else { return };
            let node = Node { inner };
            if let Err(e) = node.try_register_short_link(&id).await {
                tracing::warn!(%id, "could not register a short link: {e:#}");
            }
        });
        self.add_link_job(task);
    }

    async fn try_register_short_link(&self, id: &str) -> anyhow::Result<()> {
        let Some(service) = &self.inner.links else {
            return Ok(());
        };
        let token = {
            let registry = self.inner.registry.lock().unwrap();
            match (registry.share(id), registry.access.get(id)) {
                (Some(share), Some(access)) if !share.paused => access.token,
                _ => return Ok(()),
            }
        };
        // The home relay helps recipients find this device sooner; wait briefly for it.
        if !matches!(self.inner.relay, RelayChoice::Disabled) {
            self.wait_online(Duration::from_secs(10)).await;
        }
        let relay: Option<RelayUrl> = self.endpoint().addr().relay_urls().next().cloned();
        let expires = util::now_unix() + LINK_LIFETIME.as_secs();
        let secret = self.endpoint().secret_key();
        let code = service
            .register(secret, &token, relay.as_ref(), expires)
            .await?;
        // The share may have been removed while this was under way, and its code removed from
        // the service before it arrived there. Checked and marked under the registry lock, which
        // a removal also takes: either the removal comes after this and removes the code again,
        // or this sees it and does so.
        let newly = {
            let registry = self.inner.registry.lock().unwrap();
            let still_shared = registry.access.get(id).is_some_and(|a| a.token == token);
            if still_shared {
                Some(
                    self.inner
                        .short_links
                        .lock()
                        .unwrap()
                        .insert(id.to_string()),
                )
            } else {
                None
            }
        };
        match newly {
            None => service.delete(secret, &code, util::now_unix()).await?,
            Some(true) => {
                if let Some(share) = self.share(id) {
                    self.emit(Event::ShareUpdated { share });
                }
            }
            Some(false) => {}
        }
        Ok(())
    }

    /// Remove every short link this device registered, for when short links are turned off.
    /// Waits for the service (a few seconds at most per link).
    pub async fn remove_short_links(&self) {
        let Some(service) = self.inner.links.clone() else {
            return;
        };
        let tokens: Vec<ShareToken> = self
            .inner
            .registry
            .lock()
            .unwrap()
            .access
            .values()
            .map(|a| a.token)
            .collect();
        self.inner.short_links.lock().unwrap().clear();
        let secret = self.endpoint().secret_key().clone();
        for (i, token) in tokens.into_iter().enumerate() {
            if i > 0 {
                tokio::time::sleep(LINK_PACE).await;
            }
            let code = crate::link::share_code(&secret.public(), &token);
            if let Err(e) = service.delete(&secret, &code, util::now_unix()).await {
                tracing::warn!("could not remove a short link: {e:#}");
            }
        }
    }

    fn add_link_job(&self, task: tokio::task::JoinHandle<()>) {
        let mut jobs = self.inner.link_jobs.lock().unwrap();
        jobs.retain(|t| !t.is_finished());
        jobs.push(AbortOnDropHandle::new(task));
    }

    /// Remove a share's short code from the link service, in the background.
    /// The code may be registered from an earlier run even if it isn't marked yet, so it is
    /// removed whenever short links are on; removing an unknown code does no harm.
    fn forget_short_link(&self, id: &str, token: ShareToken) {
        self.inner.short_links.lock().unwrap().remove(id);
        if !self.short_links_on() {
            return;
        }
        let Some(service) = self.inner.links.clone() else {
            return;
        };
        let secret = self.endpoint().secret_key().clone();
        let code = crate::link::share_code(&secret.public(), &token);
        let task = tokio::spawn(async move {
            if let Err(e) = service.delete(&secret, &code, util::now_unix()).await {
                tracing::warn!("could not remove a short link: {e:#}");
            }
        });
        self.add_link_job(task);
    }

    /// Share files and/or folders. Files are served from where they are (not copied), so they
    /// must not be modified or moved while shared. Empty folders, symlinks and OS junk files
    /// (`.DS_Store`, `Thumbs.db`, …) are skipped.
    pub async fn create_share(&self, paths: Vec<PathBuf>) -> anyhow::Result<ShareInfo> {
        anyhow::ensure!(!paths.is_empty(), "nothing to share");
        let op_id = self.inner.next_op.fetch_add(1, Ordering::Relaxed);

        // Flatten the inputs into (collection name, absolute path, size, index of the item).
        let mut sources = Vec::new();
        let mut source_paths = Vec::new();
        let mut top_names = Vec::new();
        let mut skipped = Skipped::default();
        for (item, path) in paths.iter().enumerate() {
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
                sources.push((name, entry.into_path(), meta.len(), item));
            }
            // Folders with nothing shared inside; only the outermost one is listed.
            let mut with_files = HashSet::new();
            for (_, file, _, _) in &sources[first_source..] {
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
            .map(|(index, (name, path, size, item))| {
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
                                return anyhow::Ok((name, tag, size, item));
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
        for (name, tag, size, item) in &entries {
            collection.push(name.clone(), tag.hash());
            files.push(ListedFile {
                name: name.clone(),
                size: *size,
                // With one item there is only one place a file can be.
                source: (paths.len() > 1).then_some(*item),
            });
        }
        let root = collection
            .store(&self.inner.store)
            .await
            .context("storing the share index")?;
        let hash = root.hash();
        // From the tag on, nothing may interleave with a removal of the same share.
        let _ops = self.inner.share_ops.lock().await;
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
            code: String::new(),
            locked: false,
            skipped: skipped.items,
            skipped_count: skipped.count,
            paused: false,
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
            // Registry and serving change together, under the registry lock.
            let mut registry = self.inner.registry.lock().unwrap();
            registry.shares.retain(|s| s.id != share.id);
            registry.shares.push(share.clone());
            // Sharing the same files again keeps their link (and password).
            let access = registry
                .access
                .entry(share.id.clone())
                .or_insert_with(ShareAccess::generate);
            let grants: Vec<EndpointId> = access
                .grants
                .iter()
                .filter_map(|d| parse_device(d))
                .collect();
            self.inner.provider.set_grants(hash, grants);
            self.inner.provider.allow(
                hash,
                share.total_size,
                Some(files.iter().map(|f| f.size).collect()),
            );
        }
        self.persist();
        let share = self.with_link(share);
        self.emit(Event::ShareCreated {
            share: share.clone(),
        });
        self.register_short_link(share.id.clone());
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

    /// Save a `.ferry` file of a share. It holds only the link data; a locked share's file is
    /// no different, since its token gets nobody in without the password, and encrypting it
    /// with that password would let whoever holds the file guess the password offline.
    pub async fn save_share_file(&self, id: &str, path: &Path) -> anyhow::Result<()> {
        let (_, id) = parse_id(id).context("invalid share id")?;
        let access = self
            .inner
            .registry
            .lock()
            .unwrap()
            .access
            .get(&id)
            .cloned()
            .context("unknown share")?;
        let link = self.link_for(access.token);
        let path = path.to_path_buf();
        tokio::task::spawn_blocking(move || ticket_file::save_ticket_file(&path, &link)).await?
    }

    /// Protect a share with a password, change it, or remove it (`None`). Devices that
    /// opened the share before must open it again (with the new password); their running
    /// downloads stop.
    pub async fn set_share_password(
        &self,
        id: &str,
        password: Option<String>,
    ) -> anyhow::Result<ShareInfo> {
        let (hash, id) = parse_id(id).context("invalid share id")?;
        let hashed = match password {
            Some(password) => {
                Some(tokio::task::spawn_blocking(move || access::hash_password(&password)).await??)
            }
            None => None,
        };
        let _ops = self.inner.share_ops.lock().await;
        let updated = {
            let mut registry = self.inner.registry.lock().unwrap();
            let share = registry.share(&id).cloned().context("unknown share")?;
            let access = registry
                .access
                .entry(id.clone())
                .or_insert_with(ShareAccess::generate);
            access.password = hashed;
            access.grants.clear();
            self.inner.provider.set_grants(hash, []);
            share
        };
        self.inner.guesses.reset(&id);
        let saved = self.try_persist();
        let updated = self.with_link(updated);
        self.emit(Event::ShareUpdated {
            share: updated.clone(),
        });
        saved.context("the password is set, but Ferry couldn't save it; set it again")?;
        Ok(updated)
    }

    /// Answer a recipient asking for a share (the sender side of `ferry/1`).
    async fn handle_open(&self, remote: EndpointId, request: OpenRequest) -> OpenResponse {
        let remote_hex = device_hex(&remote);
        let found = {
            let registry = self.inner.registry.lock().unwrap();
            registry
                .access
                .iter()
                .find(|(_, access)| access.token == request.token)
                .and_then(|(id, access)| {
                    let share = registry.share(id)?;
                    Some((
                        id.clone(),
                        share.paused,
                        access.password.clone(),
                        access.grants.contains(&remote_hex),
                    ))
                })
        };
        let Some((id, paused, password_hash, granted)) = found else {
            return OpenResponse::Unavailable;
        };
        let Ok((hash, _)) = parse_id(&id) else {
            return OpenResponse::Unavailable;
        };
        if paused {
            return OpenResponse::Unavailable;
        }
        if let Some(stored) = password_hash.clone()
            && !granted
        {
            let Some(password) = request.password else {
                return OpenResponse::PasswordRequired;
            };
            if let Err(wait) = self.inner.guesses.take(&id, remote) {
                return OpenResponse::WrongPassword {
                    retry_after_secs: wait.as_secs().max(1),
                };
            }
            let verified =
                tokio::task::spawn_blocking(move || access::verify_password(&stored, &password))
                    .await;
            match verified {
                Ok(Ok(true)) => self.inner.guesses.give_back(&id, remote),
                Ok(Ok(false)) => {
                    return OpenResponse::WrongPassword {
                        retry_after_secs: 0,
                    };
                }
                Ok(Err(e)) => {
                    tracing::warn!(%id, "could not check a password: {e:#}");
                    return OpenResponse::Unavailable;
                }
                Err(e) => {
                    tracing::warn!(%id, "could not check a password: {e}");
                    return OpenResponse::Unavailable;
                }
            }
        }
        let changed = {
            let mut registry = self.inner.registry.lock().unwrap();
            // The share may have been removed, or got a new password, while this was checked.
            let Some(access) = registry.access.get_mut(&id) else {
                return OpenResponse::Unavailable;
            };
            if access.token != request.token || access.password != password_hash {
                return OpenResponse::Unavailable;
            }
            self.inner.provider.grant(hash, remote);
            // Only a locked share's grants are saved, so a device let in with the password can
            // resume after a restart without it. Anyone with an open share's link is simply
            // let in again.
            access.password.is_some() && access.grant(device_hex(&remote))
        };
        if changed {
            // Saving writes and syncs a file; keep it off the async threads.
            let node = self.clone();
            tokio::task::spawn_blocking(move || node.persist());
        }
        OpenResponse::Ok {
            hash,
            lan: self.lan_offer(),
        }
    }

    /// Stop serving a share. New requests are refused and running transfers are ended.
    /// The original files are left untouched.
    pub async fn remove_share(&self, id: &str) -> anyhow::Result<()> {
        let (hash, id) = parse_id(id).context("invalid share id")?;
        // Held to the end, so the same files shared again meanwhile can't lose their tag and
        // file list to this removal, or be followed by its `ShareRemoved`.
        let _ops = self.inner.share_ops.lock().await;
        let removed_access = {
            // Serving is stopped under the registry lock, together with the registry change.
            let mut registry = self.inner.registry.lock().unwrap();
            anyhow::ensure!(registry.share(&id).is_some(), "unknown share");
            registry.shares.retain(|s| s.id != id);
            let access = registry.access.remove(&id);
            self.inner.provider.disallow(&hash);
            access
        };
        if let Some(access) = removed_access {
            self.forget_short_link(&id, access.token);
        }
        self.inner.guesses.reset(&id);
        self.persist();
        self.inner.store.tags().delete(share_tag(&hash)).await?;
        self.inner.file_lists.remove(&id);
        self.emit(Event::ShareRemoved { share_id: id });
        Ok(())
    }

    /// Turn a share off or back on. While it is off, requests are refused and running
    /// transfers end, but the ticket stays valid: turning it on again serves the same link.
    ///
    /// Before a share is turned on, its files are checked (that each still exists with the
    /// size it had), so a ticket never quietly serves files that were moved or replaced.
    ///
    /// If the new state can't be saved, it still applies until Ferry stops (a share turned
    /// off stays off), [`Event::ShareUpdated`] is still sent, and an error says so.
    pub async fn set_share_paused(&self, id: &str, paused: bool) -> anyhow::Result<ShareInfo> {
        let (hash, id) = parse_id(id).context("invalid share id")?;
        let share = self
            .inner
            .registry
            .lock()
            .unwrap()
            .share(&id)
            .cloned()
            .context("unknown share")?;
        // Checking the files can take a while for big folders; it runs without holding locks.
        let sizes = if paused {
            None
        } else {
            let lists = self.inner.file_lists.clone();
            let sources = share.source_paths.clone();
            let list_id = id.clone();
            let files = tokio::task::spawn_blocking(move || {
                let files = lists.read_listed(&list_id)?;
                verify_sources(&sources, &files).map(|()| files)
            })
            .await??;
            Some(files.iter().map(|f| f.size).collect())
        };
        // Held until the event is sent, so concurrent calls save and announce their changes in
        // the order they made them.
        let _ops = self.inner.share_ops.lock().await;
        let updated = {
            // The share may have been removed (or removed and shared again) meanwhile;
            // serving changes only together with the registry, under its lock.
            let mut registry = self.inner.registry.lock().unwrap();
            let entry = registry
                .shares
                .iter_mut()
                .find(|s| s.id == id)
                .context("this ticket was removed")?;
            anyhow::ensure!(
                paused
                    || (entry.created_at == share.created_at
                        && entry.source_paths == share.source_paths),
                "this ticket changed while its files were being checked; try again"
            );
            entry.paused = paused;
            if paused {
                self.inner.provider.pause(&hash);
            } else {
                self.inner.provider.allow(hash, entry.total_size, sizes);
            }
            entry.clone()
        };
        let saved = self.try_persist();
        if !paused {
            self.register_short_link(id.clone());
        }
        let updated = self.with_link(updated);
        self.emit(Event::ShareUpdated {
            share: updated.clone(),
        });
        saved.with_context(|| {
            if paused {
                "sharing is off now, but Ferry couldn't save that, so the ticket may be \
                 shared again after Ferry restarts"
            } else {
                "sharing is on now, but Ferry couldn't save that, so the ticket will be off \
                 after Ferry restarts"
            }
        })?;
        Ok(updated)
    }

    // ------------------------------------------------------------- downloads

    async fn connect(
        &self,
        addr: EndpointAddr,
        alpn: &[u8],
    ) -> anyhow::Result<iroh::endpoint::Connection> {
        self.endpoint()
            .connect(addr, alpn)
            .await
            .context("could not reach the sender (is their Ferry running?)")
    }

    /// The device and token a link names, looking up a short code if needed, with the
    /// sender's home relay if the link service knows it.
    async fn resolve(&self, link: &ShareLink) -> anyhow::Result<(ShareLink, Option<RelayUrl>)> {
        match link {
            ShareLink::Direct { .. } => Ok((link.clone(), None)),
            ShareLink::Code(code) => {
                let service = self.inner.links.as_ref().context(
                    "short links are turned off in Settings; ask the sender for the full link",
                )?;
                let found = service.resolve(code).await?;
                Ok((ShareLink::direct(found.endpoint, found.token), found.relay))
            }
        }
    }

    /// Ask the sender named by a direct link for the share; on success this device may fetch
    /// it, and the share's root hash is returned with the sender's address.
    async fn open_share(
        &self,
        link: &ShareLink,
        relay: Option<RelayUrl>,
        password: Option<String>,
    ) -> anyhow::Result<(Hash, EndpointAddr, Option<LanAddr>)> {
        let ShareLink::Direct {
            endpoint,
            token,
            addrs,
        } = link
        else {
            anyhow::bail!("this link must be looked up first");
        };
        let addr = EndpointAddr::from_parts(
            *endpoint,
            addrs
                .iter()
                .copied()
                .map(iroh::TransportAddr::Ip)
                .chain(relay.map(iroh::TransportAddr::Relay)),
        );
        let conn = self.connect(addr.clone(), protocol::ALPN).await?;
        let answer = protocol::open(
            &conn,
            OpenRequest {
                token: *token,
                password,
            },
        )
        .await;
        conn.close(0u32.into(), b"done");
        match answer? {
            OpenResponse::Ok { hash, lan } => Ok((hash, addr, lan)),
            OpenResponse::PasswordRequired => Err(AccessError::PasswordRequired.into()),
            OpenResponse::WrongPassword {
                retry_after_secs: 0,
            } => Err(AccessError::WrongPassword.into()),
            OpenResponse::WrongPassword { retry_after_secs } => {
                Err(AccessError::TooManyGuesses { retry_after_secs }.into())
            }
            OpenResponse::Unavailable => anyhow::bail!(
                "the sender isn't sharing this right now: they turned sharing off or removed it"
            ),
        }
    }

    /// A connection for fetching the share: directly over the local network if the sender is
    /// there and offered it, otherwise to `sender` (through the relay while hiding the IP
    /// address). On the local network the share is asked for again, since the endpoints there
    /// are other devices to the sender.
    async fn transfer_connection(
        &self,
        sender: &EndpointAddr,
        hash: Hash,
        lan: Option<LanAddr>,
        token: ShareToken,
        password: Option<String>,
    ) -> anyhow::Result<iroh::endpoint::Connection> {
        if self.inner.lan_direct
            && let Some(lan) = lan
            // Without hiding, the main endpoints find a direct path to each other anyway.
            && (self.inner.lan_router.is_some() || lan.id != sender.id)
        {
            let attempt = tokio::time::timeout(
                LAN_TIMEOUT,
                self.lan_connection(&lan, hash, token, password),
            )
            .await;
            match attempt {
                Ok(Ok(conn)) => return Ok(conn),
                Ok(Err(e)) => tracing::debug!("no direct local connection: {e:#}"),
                Err(_) => tracing::debug!("the sender is not on this local network"),
            }
        }
        self.connect(sender.clone(), iroh_blobs::ALPN).await
    }

    async fn lan_connection(
        &self,
        lan: &LanAddr,
        hash: Hash,
        token: ShareToken,
        password: Option<String>,
    ) -> anyhow::Result<iroh::endpoint::Connection> {
        let dialer = self.lan_endpoint();
        let addrs = lan
            .addrs
            .iter()
            .filter(|a| protocol::is_lan_ip(a.ip(), self.inner.lan_loopback))
            .copied()
            .take(MAX_LAN_ADDRS)
            .map(iroh::TransportAddr::Ip);
        let addr = EndpointAddr::from_parts(lan.id, addrs);
        let conn = dialer.connect(addr.clone(), protocol::ALPN).await?;
        let answer = protocol::open(&conn, OpenRequest { token, password }).await;
        conn.close(0u32.into(), b"done");
        match answer? {
            OpenResponse::Ok { hash: served, .. } if served == hash => {}
            other => anyhow::bail!("the local endpoint answered {other:?}"),
        }
        Ok(dialer.connect(addr, iroh_blobs::ALPN).await?)
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

    /// Fetch the file list and sizes of a share without downloading the files. A locked
    /// share needs its `password` (see [`AccessError`]).
    pub async fn preview(
        &self,
        link: &ShareLink,
        password: Option<String>,
    ) -> anyhow::Result<Preview> {
        let (link, relay) = self.resolve(link).await?;
        let (hash, addr, lan) = self.open_share(&link, relay, password.clone()).await?;
        let token = link_token(&link)?;
        let conn = self
            .transfer_connection(&addr, hash, lan, token, password)
            .await?;
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

    /// Start (or restart) downloading a share into `dest_dir`. A locked share needs its
    /// `password` (see [`AccessError`]); resuming later doesn't.
    pub async fn start_download(
        &self,
        link: &ShareLink,
        password: Option<String>,
        dest_dir: PathBuf,
    ) -> anyhow::Result<DownloadInfo> {
        // Saving needs an absolute path; resolve it now, not after the whole transfer.
        let dest_dir = std::path::absolute(&dest_dir).context("invalid destination folder")?;
        // The download keeps the full link, so resuming needs no lookup.
        let (link, relay) = self.resolve(link).await?;
        let (hash, _, _) = self.open_share(&link, relay, password).await?;
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
            link: link.to_url(),
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
        let (hash, _) = parse_id(&info.id)?;
        let content = HashAndFormat::hash_seq(hash);
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
            // This device was let in when the download started, so no password is needed.
            anyhow::ensure!(
                !info.link.is_empty(),
                "this download was started by an older version of Ferry; open the link again"
            );
            let link = ShareLink::parse(&info.link)?;
            let (served, addr, lan) = self.open_share(&link, None, None).await.map_err(|e| {
                if e.downcast_ref::<AccessError>().is_some() {
                    e.context("the sender changed the share's password; open the link again")
                } else {
                    e
                }
            })?;
            anyhow::ensure!(
                served == hash,
                "the sender now shares different files under this link"
            );
            let conn = self
                .transfer_connection(&addr, hash, lan, link_token(&link)?, None)
                .await?;
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

            let mut meter = SpeedMeter::new(already, PROGRESS_INTERVAL);
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

/// Keep every short link registered: at start, then every [`LINK_REFRESH`]. Registrations are
/// paced, and only the ones that failed are tried again, sooner at first and then less often.
/// Stops when the node is gone.
async fn refresh_short_links(node: Weak<Inner>) {
    let mut retry: Option<(Vec<String>, Duration)> = None;
    loop {
        let Some(inner) = node.upgrade() else { return };
        let node_now = Node { inner };
        let ids: Vec<String> = match &retry {
            Some((failed, _)) => failed.clone(),
            None => node_now
                .inner
                .registry
                .lock()
                .unwrap()
                .shares
                .iter()
                .filter(|s| !s.paused)
                .map(|s| s.id.clone())
                .collect(),
        };
        let mut failed = Vec::new();
        for (i, id) in ids.into_iter().enumerate() {
            if i > 0 {
                tokio::time::sleep(LINK_PACE).await;
            }
            if let Err(e) = node_now.try_register_short_link(&id).await {
                tracing::warn!(%id, "could not refresh a short link: {e:#}");
                failed.push(id);
            }
        }
        drop(node_now);
        let wait = if failed.is_empty() {
            retry = None;
            LINK_REFRESH
        } else {
            let wait = retry.map_or(LINK_RETRY, |(_, last)| (last * 2).min(LINK_RETRY_MAX));
            retry = Some((failed, wait));
            wait
        };
        tokio::time::sleep(wait).await;
    }
}

/// Serves blobs from a store that another protocol owns: shutting down leaves the store to
/// its owner (the main endpoint's blobs protocol), which closes it once.
#[derive(Debug)]
struct SharedStore(BlobsProtocol);

impl iroh::protocol::ProtocolHandler for SharedStore {
    async fn accept(
        &self,
        connection: iroh::endpoint::Connection,
    ) -> Result<(), iroh::protocol::AcceptError> {
        self.0.accept(connection).await
    }
}

/// The token a direct link names.
fn link_token(link: &ShareLink) -> anyhow::Result<ShareToken> {
    match link {
        ShareLink::Direct { token, .. } => Ok(*token),
        ShareLink::Code(_) => anyhow::bail!("this link must be looked up first"),
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

/// Check that every file of a share is still where it was shared from, as a regular file of
/// the same size. Names are `/`-separated and start with the shared item's own name.
///
/// Each file is checked only in the item it was read from, as recorded in its file list. Two
/// shared folders can have the same name (their contents are merged), so a file found in the
/// other one is not the file the share serves. Lists written before the item was recorded
/// fall back to any shared item with the file's top-level name.
fn verify_sources(source_paths: &[PathBuf], files: &[ListedFile]) -> anyhow::Result<()> {
    // (name, parent) of each shared item, by index.
    let items: Vec<Option<(String, &Path)>> = source_paths
        .iter()
        .map(|path| {
            Some((
                path.file_name()?.to_string_lossy().into_owned(),
                path.parent()?,
            ))
        })
        .collect();
    let mut by_name: HashMap<&str, Vec<&Path>> = HashMap::new();
    for (name, parent) in items.iter().flatten() {
        by_name.entry(name.as_str()).or_default().push(parent);
    }
    for file in files {
        let top = file.name.split('/').next().unwrap_or_default();
        let candidates: Vec<&Path> = match file.source {
            // A recorded item that no longer matches the name (a damaged list) checks nothing,
            // so the file counts as changed.
            Some(index) => items
                .get(index)
                .and_then(Option::as_ref)
                .filter(|(name, _)| name == top)
                .map(|(_, parent)| *parent)
                .into_iter()
                .collect(),
            None => by_name.get(top).cloned().unwrap_or_default(),
        };
        let unchanged = candidates.iter().any(|root| {
            util::safe_join(root, &file.name)
                .and_then(|path| Ok(path.symlink_metadata()?))
                .is_ok_and(|meta| meta.is_file() && meta.len() == file.size)
        });
        anyhow::ensure!(
            unchanged,
            "{} was moved, deleted or changed since this ticket was made",
            file.name
        );
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    mod resume;
    mod short_links;

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
    fn sources_are_verified_before_a_share_is_turned_on() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Holiday");
        std::fs::create_dir_all(folder.join("day 1")).unwrap();
        std::fs::write(folder.join("day 1/a.txt"), b"hello").unwrap();
        std::fs::write(dir.path().join("b.txt"), b"abc").unwrap();
        let sources = vec![folder.clone(), dir.path().join("b.txt")];
        let files = vec![
            listed("Holiday/day 1/a.txt", 5, Some(0)),
            listed("b.txt", 3, Some(1)),
        ];
        verify_sources(&sources, &files).unwrap();

        std::fs::write(dir.path().join("b.txt"), b"abcd").unwrap();
        let err = verify_sources(&sources, &files).unwrap_err().to_string();
        assert!(err.starts_with("b.txt was moved"), "{err}");

        std::fs::write(dir.path().join("b.txt"), b"abc").unwrap();
        std::fs::remove_file(folder.join("day 1/a.txt")).unwrap();
        let err = verify_sources(&sources, &files).unwrap_err().to_string();
        assert!(err.starts_with("Holiday/day 1/a.txt was moved"), "{err}");
    }

    fn listed(name: &str, size: u64, source: Option<usize>) -> ListedFile {
        ListedFile {
            name: name.into(),
            size,
            source,
        }
    }

    #[test]
    fn folders_with_the_same_name_are_both_checked() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a/Photos"), dir.path().join("b/Photos"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("x.jpg"), b"xx").unwrap();
        std::fs::write(b.join("y.jpg"), b"yyy").unwrap();
        let sources = [a.clone(), b.clone()];
        let files = vec![
            listed("Photos/x.jpg", 2, Some(0)),
            listed("Photos/y.jpg", 3, Some(1)),
        ];
        verify_sources(&sources, &files).unwrap();
        std::fs::remove_file(a.join("x.jpg")).unwrap();
        assert!(verify_sources(&sources, &files).is_err());

        // A file of the same name and size in the other folder is not the one that was shared.
        std::fs::write(b.join("x.jpg"), b"xx").unwrap();
        let err = verify_sources(&sources, &files).unwrap_err().to_string();
        assert!(err.starts_with("Photos/x.jpg was moved"), "{err}");
        // Lists from before sources were recorded can only check by name.
        let legacy: Vec<ListedFile> = files
            .iter()
            .map(|f| listed(&f.name, f.size, None))
            .collect();
        verify_sources(&sources, &legacy).unwrap();
    }

    #[test]
    fn a_recorded_source_that_does_not_match_the_name_fails() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Photos");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("x.jpg"), b"xx").unwrap();
        let sources = [folder, dir.path().join("Other")];
        assert!(verify_sources(&sources, &[listed("Photos/x.jpg", 2, Some(1))]).is_err());
        assert!(verify_sources(&sources, &[listed("Photos/x.jpg", 2, Some(7))]).is_err());
        verify_sources(&sources, &[listed("Photos/x.jpg", 2, Some(0))]).unwrap();
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
}
