//! End-to-end tests of the `ferry` binary: two processes on this machine, relays disabled.

use std::{
    io::{BufRead, BufReader},
    path::Path,
    process::{Child, Command, Output, Stdio},
    sync::mpsc,
    time::Duration,
};

fn ferry(data_dir: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ferry"));
    cmd.arg("--data-dir")
        .arg(data_dir)
        .args(["--relay", "disabled"]);
    cmd.env("RUST_LOG", "error");
    cmd
}

/// Kills the sharing process when the test ends, even on failure.
struct Sender(Child);

impl Drop for Sender {
    fn drop(&mut self) {
        self.0.kill().ok();
        self.0.wait().ok();
    }
}

/// Start `ferry share` and wait for the link it prints.
fn start_sharing(data_dir: &Path, path: &Path, ticket_file: &Path) -> (Sender, String) {
    start_sharing_with(data_dir, path, ticket_file, None)
}

/// Like [`start_sharing`], optionally protecting the share with `password`.
fn start_sharing_with(
    data_dir: &Path,
    path: &Path,
    ticket_file: &Path,
    password: Option<&str>,
) -> (Sender, String) {
    let mut cmd = ferry(data_dir);
    cmd.arg("share")
        .arg(path)
        .arg("--save-file")
        .arg(ticket_file);
    if let Some(password) = password {
        cmd.arg("--password").env("FERRY_SHARE_PASSWORD", password);
    }
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            tx.send(line).ok();
        }
    });
    let sender = Sender(child);
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        let line = rx
            .recv_timeout(left)
            .expect("`ferry share` printed no link");
        if line.starts_with("https://ferry.xenvaultinteractive.com/s#") {
            return (sender, line);
        }
    }
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn share_and_get_between_two_processes() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("Album");
    std::fs::create_dir_all(&src).unwrap();
    let data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(src.join("song.flac"), &data).unwrap();
    std::fs::write(src.join(".DS_Store"), b"junk").unwrap();
    let ticket = tmp.path().join("album.ferry");

    let (_sender, link) = start_sharing(&tmp.path().join("a"), &src, &ticket);
    assert!(ticket.exists(), "--save-file writes the ticket file");
    let dest = tmp.path().join("dl");

    // Without --yes and without a terminal, nothing is downloaded.
    let output = ferry(&tmp.path().join("b"))
        .args(["get", &link, "--dest"])
        .arg(&dest)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("song.flac"),
        "the preview lists the files"
    );
    assert!(!dest.join("Album").exists(), "EOF at the prompt means no");

    // With --yes, from the .ferry file.
    let output = ferry(&tmp.path().join("b"))
        .arg("get")
        .arg(&ticket)
        .arg("--dest")
        .arg(&dest)
        .arg("--yes")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output));
    assert!(text(&output).contains("saved to"), "{}", text(&output));
    assert_eq!(std::fs::read(dest.join("Album/song.flac")).unwrap(), data);
    assert!(
        !dest.join("Album/.DS_Store").exists(),
        "system files are not shared"
    );
}

#[test]
fn locked_shares_ask_for_the_password() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("plans.txt");
    std::fs::write(&file, b"top secret plans").unwrap();
    let ticket = tmp.path().join("plans.ferry");
    let (_sender, link) = start_sharing_with(
        &tmp.path().join("a"),
        &file,
        &ticket,
        Some("winter crossing"),
    );
    let bytes = std::fs::read(&ticket).unwrap();
    assert!(
        !String::from_utf8_lossy(&bytes).contains("plans"),
        "the ticket file shows nothing"
    );
    let dest = tmp.path().join("dl");
    let get = |source: &std::ffi::OsStr, password: Option<&str>| {
        let mut cmd = ferry(&tmp.path().join("b"));
        cmd.arg("get")
            .arg(source)
            .arg("--dest")
            .arg(&dest)
            .arg("--yes");
        if let Some(password) = password {
            cmd.env("FERRY_SHARE_PASSWORD", password);
        }
        cmd.stdin(Stdio::null()).output().unwrap()
    };

    let output = get(link.as_ref(), None);
    assert!(!output.status.success(), "{}", text(&output));
    assert!(text(&output).contains("password"), "{}", text(&output));
    let output = get(link.as_ref(), Some("summer"));
    assert!(!output.status.success());
    assert!(text(&output).contains("not right"), "{}", text(&output));
    assert!(!dest.join("plans.txt").exists());

    // The encrypted ticket file opens with the same password, which also unlocks the share.
    let output = get(ticket.as_os_str(), Some("winter crossing"));
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(
        std::fs::read(dest.join("plans.txt")).unwrap(),
        b"top secret plans"
    );
}

#[test]
fn share_reports_what_it_left_out() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("Docs");
    std::fs::create_dir_all(src.join("empty")).unwrap();
    std::fs::write(src.join("a.txt"), b"a").unwrap();
    std::fs::write(src.join("Thumbs.db"), b"junk").unwrap();
    let mut child = ferry(&tmp.path().join("a"))
        .arg("share")
        .arg(&src)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut lines = Vec::new();
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        let done = line.starts_with("https://ferry.xenvaultinteractive.com/s#");
        lines.push(line);
        if done {
            break;
        }
    }
    child.kill().ok();
    child.wait().ok();
    let out = lines.join("\n");
    assert!(out.contains("left out 2 item(s)"), "{out}");
    assert!(
        out.contains("empty folder") && out.contains("Docs/empty"),
        "{out}"
    );
    assert!(
        out.contains("system file") && out.contains("Docs/Thumbs.db"),
        "{out}"
    );
}

#[test]
fn bad_input_is_an_error_not_a_crash() {
    let tmp = tempfile::tempdir().unwrap();
    for (args, expected) in [
        (vec!["unshare", "zz"], "invalid id"),
        (vec!["unshare", &"0".repeat(64)], "unknown share"),
        (vec!["get", "not a link", "--yes"], "not a Ferry link"),
    ] {
        let output = ferry(tmp.path()).args(&args).output().unwrap();
        assert_eq!(output.status.code(), Some(1), "{args:?}: {}", text(&output));
        assert!(
            text(&output).contains(expected),
            "{args:?}: {}",
            text(&output)
        );
    }
    let output = Command::new(env!("CARGO_BIN_EXE_ferry"))
        .arg("--data-dir")
        .arg(tmp.path())
        .args(["--relay", "https://", "status"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).to_lowercase().contains("relay"),
        "{}",
        text(&output)
    );
}
