//! "Hide my IP address": nodes on localhost behind a plain-HTTP relay, finding each other through
//! a shared in-memory address book instead of n0's DNS service.

use std::{path::Path, time::Duration};

use ferry_core::{Config, ConnectionKind, DownloadStatus, Event, Node, RelayChoice, ShareLink};
use iroh::address_lookup::MemoryLookup;
use iroh_relay::server::{RelayConfig, Server, ServerConfig};

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
}

/// A relay on localhost, without TLS.
async fn relay() -> (Server, String) {
    let mut config = ServerConfig::default();
    config.relay = Some(RelayConfig::new(([127, 0, 0, 1], 0)));
    let server = Server::spawn(config).await.unwrap();
    let addr = server.http_addr().expect("the relay serves plain HTTP");
    let url = format!("http://{addr}");
    (server, url)
}

struct Setup {
    relay: String,
    book: MemoryLookup,
}

impl Setup {
    async fn node(&self, dir: &Path, hide_ip: bool, lan_direct: bool) -> Node {
        let mut config = Config::local_only(dir);
        config.relay = RelayChoice::Custom(self.relay.clone());
        config.address_book = Some(self.book.clone());
        config.hide_ip = hide_ip;
        config.lan_direct = lan_direct;
        let node = Node::start(config).await.unwrap();
        assert!(
            node.wait_online(Duration::from_secs(30)).await,
            "the node never reached the test relay"
        );
        node
    }
}

async fn wait_for_end(node: &Node, id: &str) -> ferry_core::DownloadInfo {
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

fn sample(dir: &Path) -> (std::path::PathBuf, Vec<u8>) {
    let file = dir.join("photo.raw");
    let data: Vec<u8> = (0..2_000_000u32).map(|i| (i % 253) as u8).collect();
    std::fs::write(&file, &data).unwrap();
    (file, data)
}

#[tokio::test(flavor = "multi_thread")]
async fn hidden_devices_never_show_their_addresses_but_connect_directly_on_the_lan() {
    init_tracing();
    let tmp = tempfile::tempdir().unwrap();
    let (_relay, url) = relay().await;
    let setup = Setup {
        relay: url,
        book: MemoryLookup::new(),
    };
    let sender = setup.node(&tmp.path().join("sender"), true, true).await;
    let receiver = setup.node(&tmp.path().join("receiver"), true, true).await;
    assert!(sender.hides_ip());

    // Neither main endpoint has a direct address, so neither can be told to a peer.
    for node in [&sender, &receiver] {
        assert_eq!(node.endpoint().addr().ip_addrs().count(), 0);
        assert!(node.status().direct_addresses.is_empty());
        assert!(node.status().relay_url.is_some());
    }

    let (file, data) = sample(tmp.path());
    let share = sender.create_share(vec![file]).await.unwrap();
    let link = ShareLink::parse(&share.link).unwrap();
    let ShareLink::Direct { addrs, .. } = &link else {
        panic!("a full link")
    };
    assert!(addrs.is_empty(), "the link carries no addresses");

    // On the same network, the transfer goes directly between the local endpoints.
    let preview = receiver.preview(&link, None).await.unwrap();
    assert_eq!(preview.connection, Some(ConnectionKind::Direct));
    let started = receiver
        .start_download(&link, None, tmp.path().join("dl"))
        .await
        .unwrap();
    let done = wait_for_end(&receiver, &started.id).await;
    assert_eq!(done.status, DownloadStatus::Completed, "{:?}", done.error);
    assert_eq!(std::fs::read(done.saved_path.unwrap()).unwrap(), data);

    sender.shutdown().await.unwrap();
    receiver.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn elsewhere_a_hidden_sender_is_reached_through_the_relay() {
    init_tracing();
    let tmp = tempfile::tempdir().unwrap();
    let (_relay, url) = relay().await;
    let setup = Setup {
        relay: url,
        book: MemoryLookup::new(),
    };
    let sender = setup.node(&tmp.path().join("sender"), true, true).await;
    // A receiver that isn't hiding, and can't reach the sender's local network.
    let receiver = setup.node(&tmp.path().join("receiver"), false, false).await;

    let (file, data) = sample(tmp.path());
    let share = sender.create_share(vec![file]).await.unwrap();
    let link = ShareLink::parse(&share.link).unwrap();
    let preview = receiver.preview(&link, None).await.unwrap();
    assert_eq!(preview.connection, Some(ConnectionKind::Relay));

    let mut events = receiver.subscribe();
    let started = receiver
        .start_download(&link, None, tmp.path().join("dl"))
        .await
        .unwrap();
    let done = wait_for_end(&receiver, &started.id).await;
    assert_eq!(done.status, DownloadStatus::Completed, "{:?}", done.error);
    assert_eq!(std::fs::read(done.saved_path.unwrap()).unwrap(), data);
    // The download never switched to a direct path: the sender has none to offer.
    while let Ok(event) = events.try_recv() {
        if let Event::DownloadUpdated { download } = event {
            assert_ne!(download.connection, Some(ConnectionKind::Direct));
        }
    }

    sender.shutdown().await.unwrap();
    receiver.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn hiding_needs_relays() {
    let tmp = tempfile::tempdir().unwrap();
    let mut config = Config::local_only(tmp.path());
    config.hide_ip = true;
    let err = Node::start(config)
        .await
        .err()
        .expect("refused")
        .to_string();
    assert!(err.contains("needs relays"), "{err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hidden_receiver_opens_a_link_made_without_relays_once_the_sender_uses_them() {
    init_tracing();
    let tmp = tempfile::tempdir().unwrap();
    let (_relay, url) = relay().await;
    let setup = Setup {
        relay: url,
        book: MemoryLookup::new(),
    };
    // A sender without relays: its links carry its addresses.
    let sender_dir = tmp.path().join("sender");
    let sender = Node::start(Config::local_only(&sender_dir)).await.unwrap();
    let receiver = setup.node(&tmp.path().join("receiver"), true, true).await;
    let (file, _) = sample(tmp.path());
    let share = sender.create_share(vec![file]).await.unwrap();
    let link = ShareLink::parse(&share.link).unwrap();
    let ShareLink::Direct { addrs, .. } = &link else {
        panic!("a full link")
    };
    assert!(!addrs.is_empty(), "the link carries addresses");

    // A hidden receiver can't use them, and says why.
    let err = receiver.preview(&link, None).await.unwrap_err().to_string();
    assert!(err.contains("used no relays"), "{err}");
    sender.shutdown().await.unwrap();
    drop(sender);

    // Once the sender uses relays, the same link opens, through the relay.
    let sender = setup.node(&sender_dir, false, true).await;
    let preview = receiver.preview(&link, None).await.unwrap();
    assert_eq!(preview.name, "photo.raw");

    sender.shutdown().await.unwrap();
    receiver.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_local_network_that_doesnt_answer_falls_back_to_the_relay() {
    init_tracing();
    let tmp = tempfile::tempdir().unwrap();
    let (_relay, url) = relay().await;
    let setup = Setup {
        relay: url,
        book: MemoryLookup::new(),
    };
    // The hidden sender offers a local address where nothing answers.
    let silent = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut config = Config::local_only(tmp.path().join("sender"));
    config.relay = RelayChoice::Custom(setup.relay.clone());
    config.address_book = Some(setup.book.clone());
    config.hide_ip = true;
    config.lan_offer = Some(vec![silent.local_addr().unwrap()]);
    let sender = Node::start(config).await.unwrap();
    assert!(sender.wait_online(Duration::from_secs(30)).await);
    // A receiver that does try the local network.
    let receiver = setup.node(&tmp.path().join("receiver"), false, true).await;

    let (file, data) = sample(tmp.path());
    let share = sender.create_share(vec![file]).await.unwrap();
    let link = ShareLink::parse(&share.link).unwrap();
    let preview = receiver.preview(&link, None).await.unwrap();
    assert_eq!(preview.connection, Some(ConnectionKind::Relay));
    let started = receiver
        .start_download(&link, None, tmp.path().join("dl"))
        .await
        .unwrap();
    let done = wait_for_end(&receiver, &started.id).await;
    assert_eq!(done.status, DownloadStatus::Completed, "{:?}", done.error);
    assert_eq!(std::fs::read(done.saved_path.unwrap()).unwrap(), data);

    sender.shutdown().await.unwrap();
    receiver.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_locked_download_started_without_a_preview_still_goes_directly() {
    init_tracing();
    let tmp = tempfile::tempdir().unwrap();
    let (_relay, url) = relay().await;
    let setup = Setup {
        relay: url,
        book: MemoryLookup::new(),
    };
    let sender = setup.node(&tmp.path().join("sender"), true, true).await;
    let receiver = setup.node(&tmp.path().join("receiver"), true, true).await;
    let (file, data) = sample(tmp.path());
    let share = sender
        .create_share_with_password(vec![file], Some("winter crossing".into()))
        .await
        .unwrap();
    let link = ShareLink::parse(&share.link).unwrap();

    // Straight to the download: the password is given once, and the transfer still runs
    // between the local endpoints, which the sender sees as another device.
    let mut events = receiver.subscribe();
    let started = receiver
        .start_download(&link, Some("winter crossing".into()), tmp.path().join("dl"))
        .await
        .unwrap();
    let done = wait_for_end(&receiver, &started.id).await;
    assert_eq!(done.status, DownloadStatus::Completed, "{:?}", done.error);
    assert_eq!(std::fs::read(done.saved_path.unwrap()).unwrap(), data);
    let mut direct = false;
    while let Ok(event) = events.try_recv() {
        if let Event::DownloadUpdated { download } = event {
            direct |= download.connection == Some(ConnectionKind::Direct);
        }
    }
    assert!(direct, "the download went through the local network");

    sender.shutdown().await.unwrap();
    receiver.shutdown().await.unwrap();
}
