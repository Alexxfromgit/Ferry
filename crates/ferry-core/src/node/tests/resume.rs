use iroh_blobs::{
    provider::events::{EventMask, ProviderMessage, ThrottleMode},
    store::mem::MemStore,
};
use tokio::sync::oneshot;

use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn interrupted_download_resumes_after_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let data: Vec<u8> = (0..8 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
    let store = MemStore::new();
    let blob = store.add_bytes(data.clone()).temp_tag().await.unwrap();
    let mut collection = Collection::default();
    collection.push("large.bin".into(), blob.hash());
    let root = collection.store(&store).await.unwrap();

    // Let enough data through for the receiver to store verified chunks, then hold the
    // sender at a known point until the receiver has shut down. Loopback speed is irrelevant.
    let (release, wait_for_restart) = oneshot::channel();
    let mask = EventMask {
        throttle: ThrottleMode::Intercept,
        ..EventMask::DEFAULT
    };
    let (event_sender, mut requests) = EventSender::channel(16, mask);
    let throttle = AbortOnDropHandle::new(tokio::spawn(async move {
        let mut wait_for_restart = Some(wait_for_restart);
        let mut sent = 0;
        while let Some(message) = requests.recv().await {
            let ProviderMessage::Throttle(request) = message else {
                continue;
            };
            sent += request.inner.size;
            if sent >= 1024 * 1024
                && let Some(wait_for_restart) = wait_for_restart.take()
                && wait_for_restart.await.is_err()
            {
                return;
            }
            // The original request may have been canceled during shutdown.
            request.tx.send(Ok(())).await.ok();
        }
    }));
    let endpoint = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .alpns(vec![iroh_blobs::ALPN.to_vec()])
        .bind()
        .await
        .unwrap();
    let sender = Router::builder(endpoint)
        .accept(
            iroh_blobs::ALPN,
            BlobsProtocol::new(&store, Some(event_sender)),
        )
        .spawn();
    let link = ShareLink::new(BlobTicket::new(
        sender.endpoint().addr(),
        root.hash(),
        BlobFormat::HashSeq,
    ));

    let mut config = Config::local_only(tmp.path().join("receiver"));
    config.resume_downloads = false;
    let receiver = Node::start(config.clone()).await.unwrap();
    let started = receiver
        .start_download(&link, tmp.path().join("dl"))
        .await
        .unwrap();
    // Network progress can precede the store write. Watch the actual file's verified ranges.
    let stored_bytes = tokio::time::timeout(Duration::from_secs(30), async {
        let mut updates = receiver
            .inner
            .store
            .observe(blob.hash())
            .stream()
            .await
            .unwrap();
        while let Some(bitfield) = updates.next().await {
            assert!(
                !bitfield.is_complete(),
                "sender must stop before the whole file arrives"
            );
            if bitfield.total_bytes() > 0 {
                return bitfield.total_bytes();
            }
        }
        panic!("store closed before observing verified file data");
    })
    .await
    .expect("receiver did not store any verified file data");
    receiver.shutdown().await.unwrap();
    drop(receiver);
    release.send(()).unwrap();

    let receiver = Node::start(config).await.unwrap();
    let mut events = receiver.subscribe();
    receiver.resume_download(&started.id).await.unwrap();
    let (resumed_from, done) = tokio::time::timeout(Duration::from_secs(30), async {
        let mut resumed_from = None;
        loop {
            let Event::DownloadUpdated { download } = events.recv().await.unwrap() else {
                continue;
            };
            if download.id != started.id {
                continue;
            }
            match download.status {
                DownloadStatus::Downloading => {
                    resumed_from.get_or_insert(download.done_bytes);
                }
                DownloadStatus::Completed => return (resumed_from, download),
                DownloadStatus::Failed => panic!("resumed download failed: {:?}", download.error),
                _ => {}
            }
        }
    })
    .await
    .expect("resumed download did not finish");
    let resumed_from = resumed_from.expect("missing initial progress of the resumed download");
    assert!(
        (stored_bytes..data.len() as u64).contains(&resumed_from),
        "expected to reuse at least {stored_bytes} bytes of the partial file, reused {resumed_from}"
    );
    assert_eq!(std::fs::read(done.saved_path.unwrap()).unwrap(), data);

    receiver.shutdown().await.unwrap();
    sender.shutdown().await.unwrap();
    drop(throttle);
}
