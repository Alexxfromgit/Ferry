use iroh::SecretKey;

use super::*;

/// A share's token, as its link names it.
fn token_of(node: &Node, id: &str) -> ShareToken {
    node.inner.registry.lock().unwrap().access[id].token
}

/// Sharing the same files again must keep the devices let in so far: an open share keeps its
/// grants only in memory, so replacing them with the saved ones would cut its recipients off.
/// Checked on the grants themselves, so it doesn't depend on how fast a transfer runs.
#[tokio::test(flavor = "multi_thread")]
async fn sharing_the_same_files_again_keeps_the_devices_let_in() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("notes.txt");
    std::fs::write(&file, b"open to anyone with the link").unwrap();
    let node = Node::start(Config::local_only(tmp.path().join("node")))
        .await
        .unwrap();
    let share = node.create_share(vec![file.clone()]).await.unwrap();
    let (hash, _) = parse_id(&share.id).unwrap();

    let device = SecretKey::generate().public();
    let request = OpenRequest {
        token: token_of(&node, &share.id),
        password: None,
    };
    assert!(matches!(
        node.handle_open(device, request).await,
        OpenResponse::Ok { .. }
    ));
    assert!(node.inner.provider.is_granted(&hash, &device));

    let again = node.create_share(vec![file]).await.unwrap();
    assert_eq!(again.id, share.id);
    assert!(
        node.inner.provider.is_granted(&hash, &device),
        "the device let in before can still fetch the share"
    );

    node.shutdown().await.unwrap();
}

/// Once a device has used up its guesses, it waits even with the right password, while another
/// device with it gets in. The guesses are used up directly, so nothing depends on how fast
/// passwords are checked or on time passing.
#[tokio::test(flavor = "multi_thread")]
async fn a_device_out_of_guesses_waits_even_with_the_right_password() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("secret.txt");
    std::fs::write(&file, b"locked").unwrap();
    let node = Node::start(Config::local_only(tmp.path().join("node")))
        .await
        .unwrap();
    let share = node
        .create_share_with_password(vec![file], Some("spring tide".into()))
        .await
        .unwrap();
    let token = token_of(&node, &share.id);
    let open = |password: &str| OpenRequest {
        token,
        password: Some(password.into()),
    };

    let guesser = SecretKey::generate().public();
    while node.inner.guesses.take(&share.id, guesser).is_ok() {}
    match node.handle_open(guesser, open("spring tide")).await {
        OpenResponse::WrongPassword { retry_after_secs } => assert!(retry_after_secs > 0),
        other => panic!("expected to wait, got {other:?}"),
    }

    let recipient = SecretKey::generate().public();
    assert!(matches!(
        node.handle_open(recipient, open("spring tide")).await,
        OpenResponse::Ok { .. }
    ));

    node.shutdown().await.unwrap();
}
