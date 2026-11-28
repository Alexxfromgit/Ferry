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

#[tokio::test(flavor = "multi_thread")]
async fn turning_short_links_off_removes_them() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = MockService::default();
    let base = mock.start().await;
    let book = MemoryLookup::new();
    let sender = node_with(&tmp.path().join("sender"), Some(&base), &book).await;
    for name in ["a.txt", "b.txt"] {
        let file = tmp.path().join(name);
        std::fs::write(&file, name).unwrap();
        sender.create_share(vec![file]).await.unwrap();
    }
    tokio::time::timeout(Duration::from_secs(30), async {
        while mock.links.lock().unwrap().len() < 2 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("both short links were registered");

    let wait_for = |count: usize| {
        let mock = mock.clone();
        async move {
            tokio::time::timeout(Duration::from_secs(60), async {
                while mock.links.lock().unwrap().len() != count {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            })
            .await
            .unwrap_or_else(|_| panic!("the service never had {count} links"));
        }
    };

    sender.set_short_links(false);
    assert!(!sender.uses_link_service());
    for share in sender.list_shares() {
        assert!(share.link.contains("/s#"), "back to the full link at once");
    }
    wait_for(0).await;

    // Nothing brings them back while short links are off, and short links aren't looked up.
    let shares = sender.list_shares();
    sender.set_share_paused(&shares[0].id, true).await.unwrap();
    sender.set_share_paused(&shares[0].id, false).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(mock.links.lock().unwrap().is_empty());
    let code = ShareLink::Code(shares[0].code.clone());
    let err = sender.preview(&code, None).await.unwrap_err().to_string();
    assert!(err.contains("turned off"), "{err}");

    // Turned on again, the shares get their short links back.
    sender.set_short_links(true);
    assert!(sender.uses_link_service());
    wait_for(2).await;
    tokio::time::timeout(Duration::from_secs(10), async {
        while !sender.list_shares().iter().all(|s| s.link.contains("/s/")) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the shares show their short links again");
    sender.shutdown().await.unwrap();
}

/// Wait until `done` holds (polling), or fail with `what`.
async fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(30), async {
        while !done() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out: {what}"));
}

fn sample(dir: &Path, name: &str) -> PathBuf {
    let file = dir.join(name);
    std::fs::write(&file, name).unwrap();
    file
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_registrations_are_tried_again() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = MockService::default();
    *mock.fail_writes.lock().unwrap() = 2;
    let base = mock.start().await;
    let book = MemoryLookup::new();
    let sender = node_with(&tmp.path().join("sender"), Some(&base), &book).await;
    let share = sender
        .create_share(vec![sample(tmp.path(), "a.txt")])
        .await
        .unwrap();
    wait_until("registered after two failures", || {
        mock.links.lock().unwrap().contains_key(&share.code)
    })
    .await;
    wait_until("the short link is shown", || {
        sender.share(&share.id).unwrap().link.contains("/s/")
    })
    .await;
    sender.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn removals_are_kept_until_the_service_has_them_even_across_restarts() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = MockService::default();
    let base = mock.start().await;
    let book = MemoryLookup::new();
    let dir = tmp.path().join("sender");
    let sender = node_with(&dir, Some(&base), &book).await;
    let share = sender
        .create_share(vec![sample(tmp.path(), "a.txt")])
        .await
        .unwrap();
    wait_until("registered", || {
        mock.links.lock().unwrap().contains_key(&share.code)
    })
    .await;

    // The service is down while the share is removed and Ferry quits.
    *mock.down.lock().unwrap() = true;
    sender.remove_share(&share.id).await.unwrap();
    sender.shutdown().await.unwrap();
    drop(sender);
    let saved = std::fs::read_to_string(dir.join("registry.json")).unwrap();
    assert!(saved.contains(&share.code), "the removal is saved");

    // The next start sends it, even with short links off.
    *mock.down.lock().unwrap() = false;
    let mut config = Config::local_only(&dir);
    config.links_service = Some(base.clone());
    config.address_book = Some(book.clone());
    config.short_links = false;
    let sender = Node::start(config).await.unwrap();
    wait_until("removed after the restart", || {
        mock.links.lock().unwrap().is_empty()
    })
    .await;
    wait_until("the removal is no longer saved", || {
        !std::fs::read_to_string(dir.join("registry.json"))
            .unwrap()
            .contains(&share.code)
    })
    .await;
    sender.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn shares_removed_without_relays_still_lose_their_codes() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = MockService::default();
    let base = mock.start().await;
    let book = MemoryLookup::new();
    let dir = tmp.path().join("sender");
    let sender = node_with(&dir, Some(&base), &book).await;
    let share = sender
        .create_share(vec![sample(tmp.path(), "a.txt")])
        .await
        .unwrap();
    wait_until("registered", || {
        mock.links.lock().unwrap().contains_key(&share.code)
    })
    .await;
    sender.shutdown().await.unwrap();
    drop(sender);

    // Started again without relays (nor another way to be found): no short links now, but
    // the code from before is still removed with its share.
    let mut config = Config::local_only(&dir);
    config.links_service = Some(base.clone());
    let sender = Node::start(config).await.unwrap();
    assert!(!sender.short_links_on());
    sender.remove_share(&share.id).await.unwrap();
    wait_until("removed", || mock.links.lock().unwrap().is_empty()).await;
    sender.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn shares_that_are_turned_off_keep_their_codes() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = MockService::default();
    let base = mock.start().await;
    let book = MemoryLookup::new();
    let dir = tmp.path().join("sender");
    let sender = node_with(&dir, Some(&base), &book).await;
    let share = sender
        .create_share(vec![sample(tmp.path(), "a.txt")])
        .await
        .unwrap();
    wait_until("registered", || {
        mock.links.lock().unwrap().contains_key(&share.code)
    })
    .await;
    sender.set_share_paused(&share.id, true).await.unwrap();
    sender.shutdown().await.unwrap();
    drop(sender);

    // As if the registration had run out meanwhile: the next start renews it, though the
    // share is still off.
    mock.links.lock().unwrap().clear();
    let sender = node_with(&dir, Some(&base), &book).await;
    wait_until("registered again", || {
        mock.links.lock().unwrap().contains_key(&share.code)
    })
    .await;
    assert!(sender.share(&share.id).unwrap().paused);
    sender.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn all_requests_to_the_service_are_paced() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = MockService::default();
    let base = mock.start().await;
    let book = MemoryLookup::new();
    let sender = node_with(&tmp.path().join("sender"), Some(&base), &book).await;
    let mut shares = Vec::new();
    for name in ["a.txt", "b.txt", "c.txt"] {
        shares.push(
            sender
                .create_share(vec![sample(tmp.path(), name)])
                .await
                .unwrap(),
        );
    }
    sender.remove_share(&shares[0].id).await.unwrap();
    wait_until("two registered, one removed", || {
        let links = mock.links.lock().unwrap();
        links.len() == 2 && !links.contains_key(&shares[0].code)
    })
    .await;
    let writes = mock.writes.lock().unwrap().clone();
    assert!(writes.len() >= 3, "{writes:?}");
    for pair in writes.windows(2) {
        let gap = pair[1] - pair[0];
        // A little slack for the request on the wire, which the pause is measured from.
        assert!(
            gap + Duration::from_millis(20) >= link_worker::LINK_PACE,
            "{gap:?}"
        );
    }
    sender.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_share_that_keeps_failing_doesnt_hold_back_the_others() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = MockService::default();
    *mock.down.lock().unwrap() = true;
    let base = mock.start().await;
    let book = MemoryLookup::new();
    let sender = node_with(&tmp.path().join("sender"), Some(&base), &book).await;
    let stuck = sender
        .create_share(vec![sample(tmp.path(), "stuck.txt")])
        .await
        .unwrap();
    // The service refuses this one's code for good, and is back for everything else.
    mock.refuse.lock().unwrap().insert(stuck.code.clone());
    *mock.down.lock().unwrap() = false;
    let healthy = sender
        .create_share(vec![sample(tmp.path(), "healthy.txt")])
        .await
        .unwrap();
    // Shown once the worker has handled the answer, in the same step as scheduling its refresh.
    wait_until("the healthy share shows its short link", || {
        sender.share(&healthy.id).unwrap().link.contains("/s/")
    })
    .await;
    {
        let state = sender.inner.link_state.lock().unwrap();
        let now = std::time::Instant::now();
        let refresh = state.due[&healthy.id].at - now;
        assert!(
            refresh > link_worker::LINK_REFRESH - Duration::from_secs(60),
            "refreshed on its own schedule: {refresh:?}"
        );
        assert!(
            state.due[&stuck.id].at < now + Duration::from_secs(60 * 60),
            "the failing one is tried again soon"
        );
        assert!(!state.registered.contains_key(&stuck.id));
    }
    sender.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_registration_about_to_run_out_falls_back_to_the_full_link() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = MockService::default();
    let base = mock.start().await;
    let book = MemoryLookup::new();
    let sender = node_with(&tmp.path().join("sender"), Some(&base), &book).await;
    let share = sender
        .create_share(vec![sample(tmp.path(), "a.txt")])
        .await
        .unwrap();
    wait_until("the short link is shown", || {
        sender.share(&share.id).unwrap().link.contains("/s/")
    })
    .await;

    // As if refreshing had failed for weeks: the registration runs out in the margin's time,
    // plus a second. The share then shows its full link, and the UI is told.
    let mut events = sender.subscribe();
    {
        let mut state = sender.inner.link_state.lock().unwrap();
        let soon = util::now_unix() + link_worker::EXPIRY_MARGIN.as_secs() + 1;
        state.registered.insert(share.id.clone(), soon);
        state.due.clear();
    }
    sender.inner.links_wake.notify_one();
    let updated = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(Event::ShareUpdated { share: updated }) = events.recv().await
                && updated.id == share.id
            {
                return updated;
            }
        }
    })
    .await
    .expect("the UI is told");
    assert!(updated.link.contains("/s#"), "{}", updated.link);
    sender.shutdown().await.unwrap();
}
