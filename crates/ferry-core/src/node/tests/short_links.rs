use iroh::address_lookup::MemoryLookup;

use super::*;
use crate::links_service::tests::MockService;

async fn node_with(dir: &Path, service: Option<&str>, book: &MemoryLookup) -> Node {
    let mut config = Config::local_only(dir);
    config.links_service = service.map(str::to_string);
    // No relays here: the nodes find each other through a shared address book instead.
    config.address_book = Some(book.clone());
    Node::start(config).await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn shares_get_short_links_that_recipients_can_open() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("notes.txt");
    std::fs::write(&file, b"short and sweet").unwrap();
    let mock = MockService::default();
    let base = mock.start().await;
    let book = MemoryLookup::new();
    let sender = node_with(&tmp.path().join("sender"), Some(&base), &book).await;
    let receiver = node_with(&tmp.path().join("receiver"), Some(&base), &book).await;

    let mut events = sender.subscribe();
    let share = sender.create_share(vec![file]).await.unwrap();
    // The first link is the full one; the short one follows once the service knows it.
    assert!(share.link.contains("/s#"), "{}", share.link);
    let short = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Ok(Event::ShareUpdated { share: updated }) = events.recv().await
                && updated.id == share.id
            {
                return updated.link;
            }
        }
    })
    .await
    .expect("the short link was never registered");
    assert_eq!(
        short,
        format!("https://ferry.xenvaultinteractive.com/s/{}", share.code)
    );
    assert_eq!(sender.share(&share.id).unwrap().link, short);
    assert!(mock.links.lock().unwrap().contains_key(&share.code));

    let link = ShareLink::parse(&short).unwrap();
    let preview = receiver.preview(&link, None).await.unwrap();
    assert_eq!(preview.name, "notes.txt");
    let started = receiver
        .start_download(&link, None, tmp.path().join("dl"))
        .await
        .unwrap();
    assert!(
        started.link.contains("/s#"),
        "the download keeps the full link, so resuming needs no lookup"
    );

    // Without the service, a short link can't be opened.
    let offline = node_with(&tmp.path().join("offline"), None, &book).await;
    let err = offline.preview(&link, None).await.unwrap_err().to_string();
    assert!(err.contains("turned off"), "{err}");

    // Removing the share removes its short link too.
    sender.remove_share(&share.id).await.unwrap();
    tokio::time::timeout(Duration::from_secs(30), async {
        while mock.links.lock().unwrap().contains_key(&share.code) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the short link was not removed");
    let err = receiver.preview(&link, None).await.unwrap_err().to_string();
    assert!(err.contains("doesn't exist"), "{err}");

    for node in [sender, receiver, offline] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn without_relays_or_another_way_to_find_devices_links_stay_full() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("notes.txt");
    std::fs::write(&file, b"x").unwrap();
    let mock = MockService::default();
    let mut config = Config::local_only(tmp.path().join("sender"));
    config.links_service = Some(mock.start().await);
    let sender = Node::start(config).await.unwrap();
    let share = sender.create_share(vec![file]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(mock.links.lock().unwrap().is_empty());
    assert!(sender.share(&share.id).unwrap().link.contains("/s#"));
    sender.shutdown().await.unwrap();
}
