//! Phone-side helpers that don't need Android, kept here so they're tested on every platform.
//! `android.rs` uses them.

#![cfg_attr(not(target_os = "android"), allow(dead_code))]

use std::path::{Path, PathBuf};

use ferry_core::{DownloadInfo, DownloadStatus};

/// Where received shares go, relative to shared storage.
pub const DOWNLOADS_DIR: &str = "Download/Ferry";

/// Title, text and percentage for the transfer notification, or `None` when nothing is
/// transferring.
pub fn transfer_text<'a>(
    active: impl IntoIterator<Item = &'a DownloadInfo>,
) -> Option<(String, String, Option<u8>)> {
    let active: Vec<_> = active.into_iter().collect();
    let total: u64 = active.iter().map(|d| d.total_bytes).sum();
    let done: u64 = active.iter().map(|d| d.done_bytes.min(d.total_bytes)).sum();
    let progress = (total > 0).then(|| (done * 100 / total) as u8);
    let (title, text) = match active.as_slice() {
        [] => return None,
        [one] if one.status == DownloadStatus::Connecting => (
            format!("Receiving {}", name_of(one)),
            "Connecting to the sender…".to_owned(),
        ),
        [one] if one.status == DownloadStatus::Exporting => {
            (format!("Receiving {}", name_of(one)), "Saving…".to_owned())
        }
        [one] => (
            format!("Receiving {}", name_of(one)),
            format!("{} of {}", bytes(done), bytes(total)),
        ),
        many => (
            format!("Receiving {} downloads", many.len()),
            format!("{} of {}", bytes(done), bytes(total)),
        ),
    };
    Some((title, text, progress))
}

/// Title and text while completed downloads are copied into Downloads, given their names, or
/// `None` when nothing is being saved.
pub fn saving_text<'a>(names: impl IntoIterator<Item = &'a str>) -> Option<(String, String)> {
    let names: Vec<_> = names.into_iter().collect();
    let title = match names.as_slice() {
        [] => return None,
        [one] => format!(
            "Saving {} to Downloads",
            if one.is_empty() { "files" } else { one }
        ),
        many => format!("Saving {} downloads to Downloads", many.len()),
    };
    Some((title, "Copying files…".to_owned()))
}

/// Title and text while `count` shares are being prepared (copied into app storage and
/// hashed), or `None` when none is.
pub fn preparing_text(count: usize) -> Option<(String, String)> {
    let title = match count {
        0 => return None,
        1 => "Getting files ready to share".to_owned(),
        n => format!("Getting {n} shares ready"),
    };
    Some((title, "Ferry keeps going if you leave it".to_owned()))
}

/// A share that is on, as the notification describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sharing {
    pub name: String,
    /// Devices downloading right now.
    pub peers: usize,
    pub bytes_per_sec: u64,
}

/// Title and text for the notification while shares are on and nothing is being received.
pub fn sharing_text<'a>(shares: impl IntoIterator<Item = &'a Sharing>) -> Option<(String, String)> {
    let shares: Vec<_> = shares.into_iter().collect();
    let title = match shares.as_slice() {
        [] => return None,
        [one] => format!("Sharing {}", one.name),
        many => format!("Sharing {} items", many.len()),
    };
    let peers: usize = shares.iter().map(|s| s.peers).sum();
    let speed: u64 = shares.iter().map(|s| s.bytes_per_sec).sum();
    // The speed appears once it is measured, a moment after a download starts.
    let rate = if speed > 0 {
        format!(" · {}/s", bytes(speed))
    } else {
        String::new()
    };
    let text = match peers {
        0 if shares.len() == 1 => "Waiting for someone to open the link".to_owned(),
        0 => "Waiting for someone to open a link".to_owned(),
        1 => format!("Sending to 1 device{rate}"),
        n => format!("Sending to {n} devices{rate}"),
    };
    Some((title, text))
}

fn name_of(download: &DownloadInfo) -> &str {
    if download.name.is_empty() {
        "files"
    } else {
        &download.name
    }
}

/// The files and folders a completed download wrote into its folder (`dest_dir`): usually one,
/// but one per top-level item when the share had several. In that case `saved_path` is the
/// folder itself, which other downloads share, so the items come from `export_names`.
pub fn saved_items(download: &DownloadInfo) -> Vec<PathBuf> {
    match &download.export_names {
        Some(names) if !names.is_empty() => names
            .values()
            .map(|name| download.dest_dir.join(name))
            .collect(),
        _ => download.saved_path.iter().cloned().collect(),
    }
}

/// A size written the way the UI writes it (`bytes` in `app/src/lib/format.ts`), so the
/// notification and the app agree.
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    match unit {
        0 => format!("{n} B"),
        _ if value < 10.0 => format!("{value:.1} {}", UNITS[unit]),
        _ => format!("{value:.0} {}", UNITS[unit]),
    }
}

/// MediaStore's `RELATIVE_PATH` for `file`: its folders below `root`, under Download/Ferry.
pub fn relative_dir(root: &Path, file: &Path) -> Result<String, String> {
    let parent = file.parent().ok_or("A file has no folder.")?;
    let below = parent
        .strip_prefix(root)
        .map_err(|_| "A file is outside the download.".to_owned())?;
    let mut dir = DOWNLOADS_DIR.to_owned();
    for part in below.components() {
        dir.push('/');
        dir.push_str(&part.as_os_str().to_string_lossy());
    }
    dir.push('/');
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn relative_dirs_keep_folders_below_the_download() {
        let root = Path::new("/data/incoming");
        assert_eq!(
            relative_dir(root, Path::new("/data/incoming/Trip/Day 1/a.jpg")).unwrap(),
            "Download/Ferry/Trip/Day 1/"
        );
        assert_eq!(
            relative_dir(root, Path::new("/data/incoming/notes.txt")).unwrap(),
            "Download/Ferry/"
        );
        assert!(relative_dir(root, Path::new("/elsewhere/x")).is_err());
    }

    fn download(done: u64, total: u64, status: DownloadStatus) -> DownloadInfo {
        DownloadInfo {
            id: String::new(),
            link: String::new(),
            relay: None,
            dest_dir: PathBuf::new(),
            name: "Trip".into(),
            status,
            done_bytes: done,
            total_bytes: total,
            created_at: 0,
            saved_path: None,
            error: None,
            export_names: None,
            bytes_per_sec: 0,
            eta_secs: None,
            connection: None,
        }
    }

    #[test]
    fn saving_counts_as_work_in_the_notification() {
        assert!(saving_text([]).is_none());
        assert_eq!(saving_text(["Trip"]).unwrap().0, "Saving Trip to Downloads");
        assert_eq!(saving_text([""]).unwrap().0, "Saving files to Downloads");
        assert_eq!(
            saving_text(["Trip", "Notes"]).unwrap().0,
            "Saving 2 downloads to Downloads"
        );
    }

    #[test]
    fn notification_sums_active_downloads() {
        assert!(transfer_text([]).is_none());
        let a = download(250, 1000, DownloadStatus::Downloading);
        let (title, text, progress) = transfer_text([&a]).unwrap();
        assert_eq!(
            (title.as_str(), text.as_str(), progress),
            ("Receiving Trip", "250 B of 1000 B", Some(25))
        );
        let b = download(750, 1000, DownloadStatus::Downloading);
        let (title, _, progress) = transfer_text([&a, &b]).unwrap();
        assert_eq!(
            (title.as_str(), progress),
            ("Receiving 2 downloads", Some(50))
        );
        let connecting = download(0, 0, DownloadStatus::Connecting);
        let (_, text, progress) = transfer_text([&connecting]).unwrap();
        assert_eq!(
            (text.as_str(), progress),
            ("Connecting to the sender…", None)
        );
    }

    #[test]
    fn saved_items_are_the_downloads_own() {
        let mut one = download(1, 1, DownloadStatus::Completed);
        one.dest_dir = PathBuf::from("/data/incoming");
        one.saved_path = Some(PathBuf::from("/data/incoming/Trip"));
        one.export_names = Some([("Trip".to_owned(), "Trip".to_owned())].into());
        assert_eq!(saved_items(&one), [PathBuf::from("/data/incoming/Trip")]);

        // Several top-level items: saved_path is the shared folder, never deleted as a whole.
        let mut two = one.clone();
        two.saved_path = Some(PathBuf::from("/data/incoming"));
        two.export_names = Some(
            [
                ("A".to_owned(), "A".to_owned()),
                ("b.txt".to_owned(), "b (1).txt".to_owned()),
            ]
            .into(),
        );
        assert_eq!(
            saved_items(&two),
            [
                PathBuf::from("/data/incoming/A"),
                PathBuf::from("/data/incoming/b (1).txt")
            ]
        );

        // Downloads recorded before export_names existed fall back to saved_path.
        let mut old = one.clone();
        old.export_names = None;
        assert_eq!(saved_items(&old), [PathBuf::from("/data/incoming/Trip")]);
    }

    #[test]
    fn notification_describes_preparing_shares() {
        assert!(preparing_text(0).is_none());
        assert_eq!(preparing_text(1).unwrap().0, "Getting files ready to share");
        assert_eq!(preparing_text(2).unwrap().0, "Getting 2 shares ready");
    }

    #[test]
    fn notification_describes_shares() {
        let share = |name: &str, peers, bytes_per_sec| Sharing {
            name: name.to_owned(),
            peers,
            bytes_per_sec,
        };
        assert!(sharing_text([]).is_none());
        let waiting = share("Holiday", 0, 0);
        assert_eq!(
            sharing_text([&waiting]),
            Some((
                "Sharing Holiday".into(),
                "Waiting for someone to open the link".into()
            ))
        );
        let sending = share("Report.pdf", 1, 3_145_728);
        assert_eq!(
            sharing_text([&waiting, &sending]),
            Some((
                "Sharing 2 items".into(),
                "Sending to 1 device · 3.0 MB/s".into()
            ))
        );
        let busy = share("Clips", 2, 1024);
        assert_eq!(
            sharing_text([&busy]).unwrap().1,
            "Sending to 2 devices · 1.0 KB/s"
        );
        // Before the first speed is measured.
        let starting = share("Clips", 1, 0);
        assert_eq!(sharing_text([&starting]).unwrap().1, "Sending to 1 device");
    }

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(bytes(999), "999 B");
        assert_eq!(bytes(20_971_520), "20 MB");
        assert_eq!(bytes(1_577_058_304), "1.5 GB");
    }
}
