//! End-to-end tests: nodes on localhost, no relays, no address lookup.

use std::{path::Path, time::Duration};

use ferry_core::{Config, DownloadInfo, DownloadStatus, Event, Node, RelayChoice, ShareLink};
use iroh::{Endpoint, RelayMode, SecretKey, endpoint::presets, protocol::Router};
use iroh_blobs::{
    BlobFormat, BlobsProtocol, format::collection::Collection, store::mem::MemStore,
    ticket::BlobTicket,
};

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
}

async fn node(dir: &Path) -> Node {
    Node::start(Config::local_only(dir)).await.unwrap()
}

/// Deterministic pseudo-random content so the transfer spans many chunks.
fn content(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

async fn wait_for_end(node: &Node, id: &str) -> DownloadInfo {
    let mut events = node.subscribe();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        if let Some(d) = node.download(id)
            && matches!(d.status, DownloadStatus::Completed | DownloadStatus::Failed)
        {
            return d;
        }
        tokio::select! {
            _ = events.recv() => {}
            _ = tokio::time::sleep_until(deadline) => panic!("download did not finish in time"),
        }
    }
}

/// Wait for the first progress update that shows some, but not all, data received.
async fn wait_for_partial(events: &mut tokio::sync::broadcast::Receiver<Event>, id: &str) -> u64 {
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            if let Ok(Event::DownloadUpdated { download }) = events.recv().await
                && download.id == id
                && download.status == DownloadStatus::Downloading
                && download.done_bytes > 0
            {
                return download.done_bytes;
            }
        }
    })
    .await
    .expect("no progress reported")
}

/// A hand-made peer serving an arbitrary collection, to test what a malicious or foreign
/// sender could send.
async fn foreign_peer(names: Vec<String>) -> (Router, ShareLink) {
    let store = MemStore::new();
    let data = store.add_bytes(b"x".to_vec()).temp_tag().await.unwrap();
    let mut collection = Collection::default();
    for name in names {
        collection.push(name, data.hash());
    }
    let root = collection.store(&store).await.unwrap();
    let endpoint = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .alpns(vec![iroh_blobs::ALPN.to_vec()])
        .bind()
        .await
        .unwrap();
    let router = Router::builder(endpoint)
        .accept(iroh_blobs::ALPN, BlobsProtocol::new(&store, None))
        .spawn();
    let ticket = BlobTicket::new(router.endpoint().addr(), root.hash(), BlobFormat::HashSeq);
    // Keep the data alive for the lifetime of the router.
    std::mem::forget((data, root, store));
    (router, ShareLink::new(ticket))
}

#[tokio::test(flavor = "multi_thread")]
async fn share_folder_preview_and_download() {
    init_tracing();
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src/Holiday");
    std::fs::create_dir_all(src.join("day1")).unwrap();
    let big = content(3 * 1024 * 1024 + 17, 1);
    std::fs::write(src.join("day1/video.bin"), &big).unwrap();
    std::fs::write(src.join("notes.txt"), b"hello from ferry").unwrap();
    // Same content under another name: the blob is saved twice.
    std::fs::write(src.join("copy of notes.txt"), b"hello from ferry").unwrap();
    std::fs::write(src.join("empty.txt"), b"").unwrap();
    std::fs::write(src.join("Thumbs.db"), b"junk").unwrap();

    let sender = node(&tmp.path().join("sender")).await;
    let receiver = node(&tmp.path().join("receiver")).await;

    let share = sender.create_share(vec![src.clone()]).await.unwrap();
    assert_eq!(share.name, "Holiday");
    assert_eq!(share.file_count, 4, "OS junk files are not shared");
    assert_eq!(sender.share_files(&share.id).unwrap().len(), 4);
    assert_eq!(share.total_size, big.len() as u64 + 32);

    let link = ShareLink::parse(&share.link).unwrap();
    let preview = receiver.preview(&link).await.unwrap();
    assert_eq!(preview.name, "Holiday");
    assert_eq!(preview.total_size, share.total_size);
    let names: Vec<_> = preview.files.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Holiday/copy of notes.txt",
            "Holiday/day1/video.bin",
            "Holiday/empty.txt",
            "Holiday/notes.txt"
        ]
    );

    let dest = tmp.path().join("downloads");
    // A folder with the same name already exists: it must not be overwritten.
    std::fs::create_dir_all(dest.join("Holiday")).unwrap();
    let started = receiver.start_download(&link, dest.clone()).await.unwrap();
    let done = wait_for_end(&receiver, &started.id).await;
    assert_eq!(done.status, DownloadStatus::Completed, "{:?}", done.error);
    let saved = done.saved_path.unwrap();
    assert_eq!(saved, dest.join("Holiday (1)"));
    assert_eq!(std::fs::read(saved.join("day1/video.bin")).unwrap(), big);
    assert_eq!(
        std::fs::read(saved.join("notes.txt")).unwrap(),
        b"hello from ferry"
    );
    assert_eq!(
        std::fs::read(saved.join("copy of notes.txt")).unwrap(),
        b"hello from ferry"
    );
    assert_eq!(std::fs::read(saved.join("empty.txt")).unwrap(), b"");
    assert_eq!(done.total_bytes, share.total_size);

    // A finished download can't be "resumed".
    assert!(receiver.resume_download(&started.id).await.is_err());

    sender.shutdown().await.unwrap();
    receiver.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn relative_destination_is_resolved_up_front() {
    init_tracing();
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("doc.txt");
    std::fs::write(&file, b"relative").unwrap();
    let sender = node(&tmp.path().join("sender")).await;
    let receiver = node(&tmp.path().join("receiver")).await;
    let share = sender.create_share(vec![file]).await.unwrap();
    let link = ShareLink::parse(&share.link).unwrap();

    // Relative to the current directory, like `ferry get … -d downloads`.
    let cwd = std::env::current_dir().unwrap();
    let relative = Path::new("target-ferry-test-relative").join(&share.id[..8]);
    let started = receiver
        .start_download(&link, relative.clone())
        .await
        .unwrap();
    assert!(started.dest_dir.is_absolute());
    let done = wait_for_end(&receiver, &started.id).await;
    assert_eq!(done.status, DownloadStatus::Completed, "{:?}", done.error);
    assert_eq!(
        std::fs::read(cwd.join(&relative).join("doc.txt")).unwrap(),
        b"relative"
    );
    std::fs::remove_dir_all(cwd.join("target-ferry-test-relative")).ok();

    sender.shutdown().await.unwrap();
    receiver.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn removed_share_is_refused_but_files_stay() {
    init_tracing();
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("report.pdf");
    std::fs::write(&file, content(100_000, 7)).unwrap();

    let sender = node(&tmp.path().join("sender")).await;
    let receiver = node(&tmp.path().join("receiver")).await;
    let share = sender.create_share(vec![file.clone()]).await.unwrap();
    let link = ShareLink::parse(&share.link).unwrap();

    sender.remove_share(&share.id).await.unwrap();
    assert!(sender.list_shares().is_empty());
    assert!(
        file.exists(),
        "removing a share must not delete the original file"
    );
    assert!(
        sender.remove_share(&share.id).await.is_err(),
        "already removed"
    );

    assert!(receiver.preview(&link).await.is_err());
    let started = receiver
        .start_download(&link, tmp.path().join("dl"))
        .await
        .unwrap();
    let done = wait_for_end(&receiver, &started.id).await;
    assert_eq!(done.status, DownloadStatus::Failed);

    sender.shutdown().await.unwrap();
    receiver.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn removing_a_share_ends_a_running_transfer() {
    init_tracing();
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("movie.bin");
    let data = content(128 * 1024 * 1024, 4);
    std::fs::write(&file, &data).unwrap();
    let sender = node(&tmp.path().join("sender")).await;
    let receiver = node(&tmp.path().join("receiver")).await;
    let share = sender.create_share(vec![file]).await.unwrap();
    let link = ShareLink::parse(&share.link).unwrap();

    let mut events = receiver.subscribe();
    let started = receiver
        .start_download(&link, tmp.path().join("dl"))
        .await
        .unwrap();
    let partial = wait_for_partial(&mut events, &started.id).await;
    assert!(
        partial < data.len() as u64,
        "transfer finished too quickly to test"
    );
    sender.remove_share(&share.id).await.unwrap();

    let done = wait_for_end(&receiver, &started.id).await;
    assert_eq!(
        done.status,
        DownloadStatus::Failed,
        "the transfer must be cut off"
    );
    assert!(!tmp.path().join("dl/movie.bin").exists());

    sender.shutdown().await.unwrap();
    receiver.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_share_turned_off_is_refused_until_turned_on_again() {
    init_tracing();
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("notes.txt");
    let data = content(300_000, 5);
    std::fs::write(&file, &data).unwrap();
    let sender_dir = tmp.path().join("sender");
    let sender = node(&sender_dir).await;
    let receiver = node(&tmp.path().join("receiver")).await;
    let share = sender.create_share(vec![file.clone()]).await.unwrap();
    let link = ShareLink::parse(&share.link).unwrap();

    let off = sender.set_share_paused(&share.id, true).await.unwrap();
    assert!(off.paused);
    assert_eq!(sender.list_shares().len(), 1, "a share turned off is kept");
    assert!(receiver.preview(&link).await.is_err());

    // It stays off after a restart.
    sender.shutdown().await.unwrap();
    drop(sender);
    let sender = node(&sender_dir).await;
    assert!(sender.list_shares()[0].paused);
    // The restarted node listens on new addresses; take the link it gives now.
    let link = ShareLink::parse(&sender.list_shares()[0].link).unwrap();
    let err = receiver.preview(&link).await.unwrap_err().to_string();
    assert!(
        !err.contains("could not reach"),
        "the sender must answer and refuse: {err}"
    );

    // A file that changed size blocks turning it on.
    std::fs::write(&file, b"changed").unwrap();
    let err = sender
        .set_share_paused(&share.id, false)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("notes.txt was moved"), "{err}");
    assert!(sender.list_shares()[0].paused);

    std::fs::write(&file, &data).unwrap();
    let on = sender.set_share_paused(&share.id, false).await.unwrap();
    assert!(!on.paused);
    let started = receiver
        .start_download(&link, tmp.path().join("dl"))
        .await
        .unwrap();
    let done = wait_for_end(&receiver, &started.id).await;
    assert_eq!(done.status, DownloadStatus::Completed, "{:?}", done.error);
    assert_eq!(std::fs::read(done.saved_path.unwrap()).unwrap(), data);

    sender.shutdown().await.unwrap();
    receiver.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn shares_and_identity_survive_restart() {
    init_tracing();
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("a.bin");
    let data = content(500_000, 3);
    std::fs::write(&file, &data).unwrap();
    let sender_dir = tmp.path().join("sender");

    let sender = node(&sender_dir).await;
    let first = sender.create_share(vec![file]).await.unwrap();
    let node_id = sender.status().node_id;
    sender.shutdown().await.unwrap();
    drop(sender);

    let sender = node(&sender_dir).await;
    assert_eq!(sender.status().node_id, node_id, "node id must be stable");
    let shares = sender.list_shares();
    assert_eq!(shares.len(), 1);
    assert_eq!(shares[0].id, first.id);

    // Download using the link from the restarted node.
    let receiver = node(&tmp.path().join("receiver")).await;
    let link = ShareLink::parse(&shares[0].link).unwrap();
    let started = receiver
        .start_download(&link, tmp.path().join("dl"))
        .await
        .unwrap();
    let done = wait_for_end(&receiver, &started.id).await;
    assert_eq!(done.status, DownloadStatus::Completed, "{:?}", done.error);
    assert_eq!(std::fs::read(done.saved_path.unwrap()).unwrap(), data);

    sender.shutdown().await.unwrap();
    receiver.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn ids_are_normalized_and_unknown_ids_rejected() {
    init_tracing();
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("a.txt");
    std::fs::write(&file, b"a").unwrap();
    let sender_dir = tmp.path().join("sender");
    let sender = node(&sender_dir).await;
    let share = sender.create_share(vec![file]).await.unwrap();

    let unknown = "0".repeat(64);
    assert!(sender.remove_share(&unknown).await.is_err());
    assert!(sender.remove_share("not a hash").await.is_err());
    assert!(sender.remove_download(&unknown).await.is_err());

    // The base32 form of the same hash removes the share for good.
    let base32 = base32(share.id.parse::<iroh_blobs::Hash>().unwrap().as_bytes());
    assert_eq!(base32.len(), 52);
    assert_ne!(base32, share.id);
    sender.remove_share(&base32).await.unwrap();
    sender.shutdown().await.unwrap();
    drop(sender);
    let sender = node(&sender_dir).await;
    assert!(sender.list_shares().is_empty());
    sender.shutdown().await.unwrap();
}

/// Two people downloading at once use the same request numbers on their own connections;
/// the sender must still show them as two receivers, each with its own progress.
#[tokio::test(flavor = "multi_thread")]
async fn two_receivers_at_once_are_told_apart() {
    init_tracing();
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("movie.bin");
    let size = 128 * 1024 * 1024;
    std::fs::write(&file, content(size, 6)).unwrap();
    let sender = node(&tmp.path().join("sender")).await;
    let first = node(&tmp.path().join("first")).await;
    let second = node(&tmp.path().join("second")).await;
    let share = sender.create_share(vec![file]).await.unwrap();
    let link = ShareLink::parse(&share.link).unwrap();
    let mut events = sender.subscribe();

    let started = std::time::Instant::now();
    let (a, b) = tokio::join!(
        first.start_download(&link, tmp.path().join("dl1")),
        second.start_download(&link, tmp.path().join("dl2")),
    );
    let (a, b) = (a.unwrap(), b.unwrap());

    let mut most_at_once = 0;
    let mut last_done = std::collections::HashMap::new();
    let watch = async {
        loop {
            let Ok(Event::ShareActivity { receivers, .. }) = events.recv().await else {
                continue;
            };
            most_at_once = most_at_once.max(receivers.len());
            for r in &receivers {
                assert_eq!(r.total_bytes, size as u64, "{receivers:?}");
                assert!(r.done_bytes <= r.total_bytes);
                let before = last_done.insert(r.id, r.done_bytes).unwrap_or(0);
                assert!(r.done_bytes >= before, "progress went back: {receivers:?}");
            }
        }
    };
    let ended = |node: Node, id: String| async move {
        let done = wait_for_end(&node, &id).await;
        (done, started.elapsed())
    };
    let overlap = tokio::select! {
        _ = watch => unreachable!(),
        ((x, x_took), (y, y_took)) = async {
            tokio::join!(ended(first.clone(), a.id.clone()), ended(second.clone(), b.id.clone()))
        } => {
            assert_eq!(x.status, DownloadStatus::Completed, "{:?}", x.error);
            assert_eq!(y.status, DownloadStatus::Completed, "{:?}", y.error);
            x_took.min(y_took)
        }
    };
    // Both started together, so they ran side by side until the first one ended. Snapshots
    // come every 500 ms; after three of them both must have been shown at once. (Optimized
    // builds can be too quick for that, then only the per-receiver checks above apply.)
    if overlap >= Duration::from_millis(1500) {
        assert_eq!(most_at_once, 2, "both downloads must show at the same time");
    } else {
        eprintln!("transfers overlapped only {overlap:?}; not checking that both showed at once");
    }

    for node in [sender, first, second] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn upload_activity_is_reported() {
    init_tracing();
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("b.bin");
    std::fs::write(&file, content(2_000_000, 9)).unwrap();
    let sender = node(&tmp.path().join("sender")).await;
    let receiver = node(&tmp.path().join("receiver")).await;
    let share = sender.create_share(vec![file]).await.unwrap();
    let mut events = sender.subscribe();

    let link = ShareLink::parse(&share.link).unwrap();
    // A preview probes sizes by fetching only the last chunk of each file; that must not be
    // counted as a full upload.
    receiver.preview(&link).await.unwrap();
    let started = receiver
        .start_download(&link, tmp.path().join("dl"))
        .await
        .unwrap();
    assert_eq!(
        wait_for_end(&receiver, &started.id).await.status,
        DownloadStatus::Completed
    );

    // Wait for the snapshot taken after every request finished. Previews and index fetches
    // carry no file data, so the only receiver ever shown is the download, asking for the
    // whole file.
    let mut last = 0;
    let mut receivers_seen = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(Event::ShareActivity {
                share_id,
                uploaded_bytes,
                active_peers,
                receivers,
                ..
            }) = events.recv().await
                && share_id == share.id
            {
                last = uploaded_bytes;
                receivers_seen.extend(receivers.into_iter().map(|r| r.total_bytes));
                if active_peers == 0 && uploaded_bytes >= 2_000_000 {
                    break;
                }
            }
        }
    })
    .await;
    assert!(
        (2_000_000..2_100_000).contains(&last),
        "expected about 2 MB sent, got {last}"
    );
    assert!(
        receivers_seen.iter().all(|&total| total == 2_000_000),
        "unexpected receivers: {receivers_seen:?}"
    );

    sender.shutdown().await.unwrap();
    receiver.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn rapid_pause_resume_pause_stays_consistent() {
    init_tracing();
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("big.bin");
    let data = content(64 * 1024 * 1024, 8);
    std::fs::write(&file, &data).unwrap();
    let sender = node(&tmp.path().join("sender")).await;
    let receiver = node(&tmp.path().join("receiver")).await;
    let share = sender.create_share(vec![file]).await.unwrap();
    let link = ShareLink::parse(&share.link).unwrap();

    let started = receiver
        .start_download(&link, tmp.path().join("dl"))
        .await
        .unwrap();
    for _ in 0..5 {
        receiver.pause_download(&started.id);
        receiver.resume_download(&started.id).await.unwrap();
    }
    receiver.pause_download(&started.id);
    // Give any aborted task a chance to (wrongly) write its state.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        receiver.download(&started.id).unwrap().status,
        DownloadStatus::Paused
    );

    receiver.resume_download(&started.id).await.unwrap();
    let done = wait_for_end(&receiver, &started.id).await;
    assert_eq!(done.status, DownloadStatus::Completed, "{:?}", done.error);
    assert_eq!(std::fs::read(done.saved_path.unwrap()).unwrap(), data);

    sender.shutdown().await.unwrap();
    receiver.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_download_that_fails_at_once_can_be_retried() {
    init_tracing();
    let tmp = tempfile::tempdir().unwrap();
    let receiver = node(&tmp.path().join("receiver")).await;
    // Nobody is listening for this node id and there are no addresses or relays.
    let addr = iroh::EndpointAddr::new(SecretKey::generate().public());
    let ticket = BlobTicket::new(addr, iroh_blobs::Hash::new(b"x"), BlobFormat::HashSeq);
    let link = ShareLink::new(ticket);

    let started = receiver
        .start_download(&link, tmp.path().join("dl"))
        .await
        .unwrap();
    let done = wait_for_end(&receiver, &started.id).await;
    assert_eq!(done.status, DownloadStatus::Failed);

    // Starting again really restarts it (no stale task blocks it), and fails again.
    let mut events = receiver.subscribe();
    receiver
        .start_download(&link, tmp.path().join("dl"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Ok(Event::DownloadUpdated { download }) = events.recv().await
                && download.status == DownloadStatus::Failed
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    receiver.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn foreign_shares_with_unsafe_names_are_rejected_before_download() {
    init_tracing();
    let tmp = tempfile::tempdir().unwrap();
    let receiver = node(&tmp.path().join("receiver")).await;
    for names in [
        vec!["../escape.txt".to_string()],
        vec!["NUL".to_string()],
        vec!["notes.".to_string(), "notes".to_string()],
        vec!["Photo.jpg".to_string(), "photo.jpg".to_string()],
        vec!["a".to_string(), "a/b".to_string()],
        vec!["C:x".to_string()],
    ] {
        let (router, link) = foreign_peer(names.clone()).await;
        let err = receiver.preview(&link).await.unwrap_err();
        assert!(
            format!("{err:#}").contains("share"),
            "{names:?} gave an unexpected error: {err:#}"
        );
        let started = receiver
            .start_download(&link, tmp.path().join("dl"))
            .await
            .unwrap();
        let done = wait_for_end(&receiver, &started.id).await;
        assert_eq!(done.status, DownloadStatus::Failed, "{names:?}");
        assert_eq!(std::fs::read_dir(tmp.path().join("dl")).unwrap().count(), 0);
        router.shutdown().await.unwrap();
    }
    receiver.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn oversized_file_list_is_rejected() {
    init_tracing();
    let tmp = tempfile::tempdir().unwrap();
    let receiver = node(&tmp.path().join("receiver")).await;
    // About 20 MB of names (the limit is 16 MiB).
    let names = (0..100_000)
        .map(|i| format!("d{i:06}/{}", "x".repeat(190)))
        .collect();
    let (router, link) = foreign_peer(names).await;
    let err = receiver.preview(&link).await.unwrap_err();
    assert!(format!("{err:#}").contains("too large"), "{err:#}");
    router.shutdown().await.unwrap();
    receiver.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_settings_or_damaged_files_do_not_prevent_start() {
    init_tracing();
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("node");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("registry.json"), b"\0\0 truncated").unwrap();
    let mut config = Config::local_only(&dir);
    config.relay = RelayChoice::Custom("https://".into());
    let node = Node::start(config).await.unwrap();
    assert_eq!(
        node.relay(),
        &RelayChoice::Default,
        "invalid relay falls back"
    );
    assert!(node.list_shares().is_empty());
    node.shutdown().await.unwrap();
}

/// Lower-case RFC 4648 base32 without padding (the other text form `Hash` accepts).
fn base32(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut out = String::new();
    let (mut buffer, mut bits) = (0u32, 0);
    for &byte in bytes {
        buffer = (buffer << 8) | byte as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_starts_of_the_same_download_end_consistently() {
    init_tracing();
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("same.bin");
    let data = content(8 * 1024 * 1024, 2);
    std::fs::write(&file, &data).unwrap();
    let sender = node(&tmp.path().join("sender")).await;
    let receiver = node(&tmp.path().join("receiver")).await;
    let share = sender.create_share(vec![file]).await.unwrap();
    let link = ShareLink::parse(&share.link).unwrap();

    let dest = tmp.path().join("dl");
    let (a, b) = tokio::join!(
        receiver.start_download(&link, dest.clone()),
        receiver.start_download(&link, dest.clone())
    );
    let id = a.unwrap().id;
    assert_eq!(b.unwrap().id, id);
    let done = wait_for_end(&receiver, &id).await;
    assert_eq!(done.status, DownloadStatus::Completed, "{:?}", done.error);
    assert_eq!(receiver.list_downloads().len(), 1);
    assert_eq!(std::fs::read(done.saved_path.unwrap()).unwrap(), data);
    assert_eq!(
        std::fs::read_dir(&dest).unwrap().count(),
        1,
        "saved exactly once"
    );

    sender.shutdown().await.unwrap();
    receiver.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn skipped_items_are_reported_and_remembered() {
    use ferry_core::SkipReason;

    init_tracing();
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("Trip");
    std::fs::create_dir_all(src.join("empty/inner")).unwrap();
    std::fs::create_dir_all(src.join("photos")).unwrap();
    std::fs::write(src.join("photos/a.jpg"), b"a").unwrap();
    std::fs::write(src.join("photos/.DS_Store"), b"junk").unwrap();
    // Links are only possible where the OS allows creating them (Windows needs Developer Mode).
    #[cfg(unix)]
    let made_link =
        std::os::unix::fs::symlink(src.join("photos/a.jpg"), src.join("link.jpg")).is_ok();
    #[cfg(windows)]
    let made_link =
        std::os::windows::fs::symlink_file(src.join("photos/a.jpg"), src.join("link.jpg")).is_ok();

    let dir = tmp.path().join("sender");
    let sender = node(&dir).await;
    let share = sender.create_share(vec![src.clone()]).await.unwrap();
    assert_eq!(share.file_count, 1);
    let mut skipped: Vec<_> = share
        .skipped
        .iter()
        .map(|s| (s.name.as_str(), s.reason))
        .collect();
    skipped.sort();
    let mut expected = vec![
        ("Trip/empty", SkipReason::EmptyFolder),
        ("Trip/photos/.DS_Store", SkipReason::SystemFile),
    ];
    if made_link {
        expected.push(("Trip/link.jpg", SkipReason::Link));
    }
    expected.sort();
    assert_eq!(skipped, expected, "only the outer empty folder is listed");
    assert_eq!(share.skipped_count, expected.len());

    // Remembered across a restart, so the ticket can keep showing it.
    sender.shutdown().await.unwrap();
    drop(sender);
    let sender = node(&dir).await;
    assert_eq!(sender.list_shares()[0].skipped_count, expected.len());

    // A selection with nothing to share says why.
    let only_junk = tmp.path().join("junk");
    std::fs::create_dir_all(only_junk.join("nothing")).unwrap();
    std::fs::write(only_junk.join("Thumbs.db"), b"x").unwrap();
    let err = sender.create_share(vec![only_junk]).await.unwrap_err();
    assert!(format!("{err:#}").contains("nothing to share"), "{err:#}");
    sender.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn file_lists_live_outside_the_registry() {
    init_tracing();
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("node");
    std::fs::create_dir_all(&dir).unwrap();
    // A registry written by an earlier version, with the file list inline.
    let id = "ab".repeat(32);
    let old = format!(
        r#"{{"shares": [{{"id": "{id}", "name": "Old", "sourcePaths": ["/x/Old"], "totalSize": 3,
            "createdAt": 1, "files": [{{"name": "Old/a", "size": 1}}, {{"name": "Old/b", "size": 2}}]}}],
            "downloads": []}}"#
    );
    std::fs::write(dir.join("registry.json"), old).unwrap();

    let node = node(&dir).await;
    let shares = node.list_shares();
    assert_eq!(shares[0].file_count, 2);
    assert_eq!(node.share_files(&id).unwrap().len(), 2);
    let registry = std::fs::read_to_string(dir.join("registry.json")).unwrap();
    assert!(
        !registry.contains("Old/a"),
        "the list moved out of the registry"
    );
    assert!(dir.join(format!("shares/{id}.json")).exists());

    // A new share writes its list once; removing the share removes it.
    let src = tmp.path().join("Many");
    std::fs::create_dir_all(&src).unwrap();
    for i in 0..1200 {
        std::fs::write(src.join(format!("f{i:04}.txt")), i.to_string()).unwrap();
    }
    let share = node.create_share(vec![src]).await.unwrap();
    assert_eq!(share.file_count, 1200);
    let registry = std::fs::read_to_string(dir.join("registry.json")).unwrap();
    assert!(
        !registry.contains("f0001.txt"),
        "the registry only keeps the count"
    );

    // .ferry files carry a capped sample plus the full count.
    let file = node.share_file(&share.id).unwrap();
    assert_eq!(file.file_count, 1200);
    assert_eq!(file.files.len(), ferry_core::MAX_FILES_IN_SHARE_FILE);
    assert!(ShareLink::parse(&file.to_json()).is_ok());

    node.remove_share(&share.id).await.unwrap();
    assert!(!dir.join(format!("shares/{}.json", share.id)).exists());
    node.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn previews_carry_a_sample_of_names_and_the_full_count() {
    init_tracing();
    let tmp = tempfile::tempdir().unwrap();
    let receiver = node(&tmp.path().join("receiver")).await;
    let names = (0..1000).map(|i| format!("Big/{i:04}.txt")).collect();
    let (router, link) = foreign_peer(names).await;
    let preview = receiver.preview(&link).await.unwrap();
    assert_eq!(preview.file_count, 1000);
    assert_eq!(preview.files.len(), ferry_core::PREVIEW_FILES);
    assert_eq!(preview.total_size, 1000, "every file is the 1-byte blob");
    router.shutdown().await.unwrap();
    receiver.shutdown().await.unwrap();
}
