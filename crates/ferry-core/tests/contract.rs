//! Contract between the engine's JSON and the UI's hand-written TypeScript types.
//!
//! This test serializes one fully populated and one minimal value of every type the UI
//! receives, and writes them into `app/src/lib/contract.generated.ts` as
//! `… satisfies SomeType` expressions. `npm run check` then type-checks that file against
//! `app/src/lib/types.ts`, so a renamed, missing or extra field on either side fails the build.
//!
//! The test fails when the committed file is out of date; regenerate it with
//! `UPDATE_CONTRACT=1 cargo test -p ferry-core --test contract`.

use std::{collections::BTreeMap, path::PathBuf};

use ferry_core::{
    ConnectionKind, DownloadInfo, DownloadStatus, Event, FileEntry, NodeStatus, Preview,
    ReceiverProgress, RelayChoice, ShareInfo, SkipReason, SkippedItem,
};
use serde::Serialize;

fn share(full: bool) -> ShareInfo {
    ShareInfo {
        id: "a9cf".repeat(16),
        name: "Holiday".into(),
        source_paths: vec![PathBuf::from("/home/example/Holiday")],
        file_count: 1,
        total_size: 42,
        created_at: 1_794_500_000,
        link: if full {
            "https://ferry.xenvaultinteractive.com/get#blob".into()
        } else {
            String::new()
        },
        ticket: if full { "blob".into() } else { String::new() },
        skipped: if full {
            [
                SkipReason::SystemFile,
                SkipReason::Link,
                SkipReason::EmptyFolder,
                SkipReason::Unreadable,
                SkipReason::NotAFile,
            ]
            .into_iter()
            .map(|reason| SkippedItem {
                name: "Holiday/x".into(),
                reason,
            })
            .collect()
        } else {
            Vec::new()
        },
        skipped_count: if full { 5 } else { 0 },
        paused: full,
    }
}

fn download(status: DownloadStatus, full: bool) -> DownloadInfo {
    DownloadInfo {
        id: "b8de".repeat(16),
        ticket: "blob".into(),
        dest_dir: PathBuf::from("/home/example/Downloads"),
        name: "Holiday".into(),
        status,
        done_bytes: 10,
        total_bytes: 42,
        created_at: 1_794_500_000,
        saved_path: full.then(|| PathBuf::from("/home/example/Downloads/Holiday")),
        error: full.then(|| "could not reach the sender".into()),
        export_names: full.then(|| BTreeMap::from([("Holiday".into(), "Holiday (1)".into())])),
        bytes_per_sec: 1000,
        eta_secs: full.then_some(3),
        connection: full.then_some(ConnectionKind::Relay),
    }
}

fn ts<T: Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value).unwrap()
}

fn generate() -> String {
    let statuses = [
        DownloadStatus::Connecting,
        DownloadStatus::Downloading,
        DownloadStatus::Exporting,
        DownloadStatus::Completed,
        DownloadStatus::Paused,
        DownloadStatus::Failed,
    ];
    let downloads: Vec<DownloadInfo> = statuses
        .iter()
        .map(|s| download(*s, false))
        .chain([download(DownloadStatus::Completed, true)])
        .collect();
    let events = vec![
        Event::ImportProgress {
            op_id: 1,
            done_bytes: 1,
            total_bytes: 2,
            current_file: "Holiday/a.jpg".into(),
        },
        Event::ShareCreated { share: share(true) },
        Event::ShareRemoved {
            share_id: "a9cf".repeat(16),
        },
        Event::ShareUpdated {
            share: share(false),
        },
        Event::ShareActivity {
            share_id: "a9cf".repeat(16),
            active_peers: 1,
            uploaded_bytes: 10,
            bytes_per_sec: 5,
            receivers: vec![
                ReceiverProgress {
                    id: 3,
                    done_bytes: 10,
                    total_bytes: 42,
                    bytes_per_sec: 5,
                    eta_secs: Some(7),
                },
                ReceiverProgress {
                    id: 4,
                    done_bytes: 0,
                    total_bytes: 42,
                    bytes_per_sec: 0,
                    eta_secs: None,
                },
            ],
        },
        Event::DownloadUpdated {
            download: download(DownloadStatus::Downloading, true),
        },
        Event::DownloadRemoved {
            download_id: "b8de".repeat(16),
        },
    ];
    let previews = vec![
        Preview {
            id: "a9cf".repeat(16),
            name: "Holiday".into(),
            files: vec![FileEntry {
                name: "Holiday/a.jpg".into(),
                size: 42,
            }],
            file_count: 1,
            total_size: 42,
            connection: Some(ConnectionKind::Direct),
        },
        Preview {
            id: "a9cf".repeat(16),
            name: "Holiday".into(),
            files: Vec::new(),
            file_count: 0,
            total_size: 0,
            connection: None,
        },
    ];
    let statuses = vec![
        NodeStatus {
            node_id: "c7".repeat(32),
            relay_url: Some("https://relay.example.com/".into()),
            direct_addresses: vec!["192.168.1.2:4000".into()],
        },
        NodeStatus {
            node_id: "c7".repeat(32),
            relay_url: None,
            direct_addresses: Vec::new(),
        },
    ];
    let relays = vec![
        RelayChoice::Default,
        RelayChoice::Custom("https://relay.example.com".into()),
        RelayChoice::Disabled,
    ];

    format!(
        "// Generated by `crates/ferry-core/tests/contract.rs`; do not edit.\n\
         // Regenerate: UPDATE_CONTRACT=1 cargo test -p ferry-core --test contract\n\
         // `npm run check` type-checks these values (produced by the Rust code) against types.ts.\n\
         import type {{ DownloadInfo, FerryEvent, NodeStatus, Preview, RelayChoice, ShareInfo }} from \"./types\";\n\n\
         export const shares = [{}, {}] satisfies ShareInfo[];\n\n\
         export const downloads = {} satisfies DownloadInfo[];\n\n\
         export const events = {} satisfies FerryEvent[];\n\n\
         export const previews = {} satisfies Preview[];\n\n\
         export const nodeStatuses = {} satisfies NodeStatus[];\n\n\
         export const relays = {} satisfies RelayChoice[];\n",
        ts(&share(true)),
        ts(&share(false)),
        ts(&downloads),
        ts(&events),
        ts(&previews),
        ts(&statuses),
        ts(&relays),
    )
}

#[test]
fn typescript_contract_is_up_to_date() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../app/src/lib/contract.generated.ts");
    let generated = generate();
    if std::env::var_os("UPDATE_CONTRACT").is_some() {
        std::fs::write(&path, &generated).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        committed.replace("\r\n", "\n") == generated,
        "app/src/lib/contract.generated.ts is out of date with the Rust types.\n\
         Run `UPDATE_CONTRACT=1 cargo test -p ferry-core --test contract`, then `npm run check` in app/."
    );
}
