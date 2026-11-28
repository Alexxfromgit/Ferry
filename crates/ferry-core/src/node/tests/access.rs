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
