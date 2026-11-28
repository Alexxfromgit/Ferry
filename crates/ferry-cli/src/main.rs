//! `ferry` command-line client: the same engine as the desktop app, for headless testing.

use std::{
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::Context;
use clap::{Parser, Subcommand};
use ferry_core::{
    AccessError, Config, ConnectionKind, DownloadStatus, Event, Node, Preview, RelayChoice,
    SHARE_FILE_EXTENSION, ShareLink, SkipReason, check_password, format_code, read_ticket_file,
};

/// Scripts can give the share password here instead of typing it.
const PASSWORD_ENV: &str = "FERRY_SHARE_PASSWORD";

#[derive(Parser)]
#[command(version, about = "Share files peer-to-peer with Ferry")]
struct Cli {
    /// Where identity, shares and partial downloads are kept.
    /// Defaults to a per-user `ferry-cli` folder (separate from the desktop app).
    #[arg(long, global = true, env = "FERRY_DATA_DIR")]
    data_dir: Option<PathBuf>,

    /// Relay servers: "default" (n0 public relays), "disabled", or a relay URL.
    #[arg(long, global = true, default_value = "default")]
    relay: String,

    /// Share full links only, and don't open short ones. Ferry still asks its link service to
    /// remove short links it registered before, when their shares are removed.
    #[arg(long, global = true)]
    no_short_links: bool,

    /// Hide this device's IP address: devices elsewhere connect only through the relay.
    /// Devices on the same local network still connect directly. Needs relays.
    #[arg(long, global = true)]
    hide_ip: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Share files or folders and keep serving them until Ctrl+C.
    Share {
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Also write a .ferry share file to this path.
        #[arg(long)]
        save_file: Option<PathBuf>,
        /// Ask recipients for a password. Prompts for it, or reads FERRY_SHARE_PASSWORD.
        #[arg(long)]
        password: bool,
    },
    /// Keep serving all existing shares until Ctrl+C.
    Serve,
    /// Download a share from a share link, a short code, or a .ferry file. Prompts for the
    /// password if the share has one (or reads FERRY_SHARE_PASSWORD).
    Get {
        link: String,
        /// Destination folder (default: current directory).
        #[arg(long, short)]
        dest: Option<PathBuf>,
        /// Skip the confirmation after the preview.
        #[arg(long, short)]
        yes: bool,
    },
    /// List shares.
    Shares,
    /// Stop sharing (the original files are not touched).
    Unshare { id: String },
    /// Show this node's id and addresses.
    Status,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,ferry_core=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();

    let data_dir = match cli.data_dir {
        Some(dir) => dir,
        None => dirs::data_local_dir()
            .context("no per-user data directory on this system; pass --data-dir")?
            .join("ferry-cli"),
    };
    let relay = match cli.relay.as_str() {
        "default" => RelayChoice::Default,
        "disabled" => RelayChoice::Disabled,
        url => RelayChoice::Custom(url.to_string()),
    };
    // Reject a bad relay address here instead of silently using the default relays.
    relay.validate()?;
    let mut config = Config::new(data_dir);
    config.relay = relay.clone();
    config.short_links = !cli.no_short_links;
    config.hide_ip = cli.hide_ip;
    // A one-shot tool should not quietly continue old downloads in the background.
    config.resume_downloads = false;
    let node = Node::start(config).await?;
    let needs_network = matches!(
        cli.command,
        Command::Share { .. } | Command::Serve | Command::Get { .. }
    );
    if needs_network
        && relay != RelayChoice::Disabled
        && !node.wait_online(Duration::from_secs(15)).await
    {
        eprintln!("warning: no relay reachable yet; links may only work on this network");
    }

    let result = run(&node, cli.command).await;
    node.shutdown().await?;
    result
}

async fn run(node: &Node, command: Command) -> anyhow::Result<()> {
    match command {
        Command::Share {
            paths,
            save_file,
            password,
        } => {
            let password = if password {
                Some(match std::env::var(PASSWORD_ENV) {
                    Ok(password) => password,
                    Err(_) => {
                        let password = prompt_password()?;
                        let confirmation = rpassword::prompt_password("Confirm the password: ")?;
                        anyhow::ensure!(password == confirmation, "Passwords do not match");
                        password
                    }
                })
            } else {
                None
            };
            // Checked before anything is shared, so a bad password never leaves an open share.
            if let Some(password) = &password {
                check_password(password)?;
            }
            let mut events = node.subscribe();
            let progress = tokio::spawn(async move {
                while let Ok(event) = events.recv().await {
                    if let Event::ImportProgress {
                        done_bytes,
                        total_bytes,
                        ..
                    } = event
                    {
                        eprint!(
                            "\rhashing {} / {}   ",
                            human(done_bytes),
                            human(total_bytes)
                        );
                    }
                }
            });
            // The password is set as the share is made, so it is never served without it.
            let mut share = node.create_share_with_password(paths, password).await?;
            progress.abort();
            eprintln!();
            println!(
                "sharing \"{}\": {} file(s), {}",
                share.name,
                share.file_count,
                human(share.total_size)
            );
            if share.skipped_count > 0 {
                println!("left out {} item(s):", share.skipped_count);
                for item in share.skipped.iter().take(20) {
                    println!("  {:<14} {}", skip_reason(item.reason), item.name);
                }
                if share.skipped_count > 20 {
                    println!("  ... and {} more", share.skipped_count - 20);
                }
            }
            if let Some(path) = save_file {
                let path = if path.extension().is_none() {
                    path.with_extension(SHARE_FILE_EXTENSION)
                } else {
                    path
                };
                node.save_share_file(&share.id, &path).await?;
                println!("share file written to {}", path.display());
            }
            // The short link follows once the link service has it; give it a moment.
            if node.short_links_on() {
                let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
                while !share.link.contains("/s/") && tokio::time::Instant::now() < deadline {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    if let Some(current) = node.share(&share.id) {
                        share = current;
                    }
                }
                if !share.link.contains("/s/") {
                    eprintln!("note: the link service didn't answer; this is the full link");
                }
            }
            println!("\nsend this link to the recipient:\n\n{}\n", share.link);
            if share.link.contains("/s/") {
                println!("share code: {}", format_code(&share.code));
            }
            if share.locked {
                println!("recipients need the password; send it separately.");
            }
            serve(node).await
        }
        Command::Serve => {
            let shares = node.list_shares();
            anyhow::ensure!(!shares.is_empty(), "nothing is shared yet");
            for share in shares {
                println!("{}  {}\n  {}", share.id, share.name, share.link);
            }
            serve(node).await
        }
        Command::Get { link, dest, yes } => {
            let path = Path::new(&link);
            let mut password = std::env::var(PASSWORD_ENV).ok();
            let link = if path.is_file() {
                read_ticket_file(path)?
            } else {
                ShareLink::parse(&link)?
            };
            eprintln!("connecting to the sender...");
            let preview = preview_with_password(node, &link, &mut password).await?;
            println!(
                "\"{}\": {} file(s), {}{}",
                preview.name,
                preview.file_count,
                human(preview.total_size),
                describe(preview.connection)
            );
            for file in preview.files.iter().take(20) {
                println!("  {:>10}  {}", human(file.size), file.name);
            }
            if preview.file_count > 20 {
                println!("  ... and {} more", preview.file_count - 20);
            }
            if !yes && !confirm("download?")? {
                return Ok(());
            }
            let dest = match dest {
                Some(d) => d,
                None => std::env::current_dir()?,
            };
            let mut events = node.subscribe();
            let started = node.start_download(&link, password, dest).await?;
            loop {
                let download = match events.recv().await {
                    Ok(Event::DownloadUpdated { download }) if download.id == started.id => {
                        download
                    }
                    Ok(_) => continue,
                    Err(_) => node.download(&started.id).context("download vanished")?,
                };
                match download.status {
                    DownloadStatus::Completed => {
                        eprintln!();
                        println!(
                            "saved to {}",
                            download.saved_path.unwrap_or_default().display()
                        );
                        return Ok(());
                    }
                    DownloadStatus::Failed => {
                        eprintln!();
                        anyhow::bail!(download.error.unwrap_or_else(|| "failed".into()));
                    }
                    DownloadStatus::Downloading => eprint!(
                        "\r{} / {}  {}/s  eta {}{}      ",
                        human(download.done_bytes),
                        human(download.total_bytes),
                        human(download.bytes_per_sec),
                        download
                            .eta_secs
                            .map(|s| format!("{}m{:02}s", s / 60, s % 60))
                            .unwrap_or_else(|| "?".into()),
                        describe(download.connection),
                    ),
                    status => eprint!("\r{status:?}...      "),
                }
            }
        }
        Command::Shares => {
            for share in node.list_shares() {
                println!(
                    "{}  {}  ({} files, {}){}\n  {}",
                    share.id,
                    share.name,
                    share.file_count,
                    human(share.total_size),
                    if share.locked { ", password" } else { "" },
                    share.link
                );
            }
            Ok(())
        }
        Command::Unshare { id } => {
            node.remove_share(&id).await?;
            println!("stopped sharing {id}");
            Ok(())
        }
        Command::Status => {
            let status = node.status();
            println!("node id:  {}", status.node_id);
            println!(
                "relay:    {}",
                status.relay_url.as_deref().unwrap_or("(none)")
            );
            for addr in status.direct_addresses {
                println!("direct:   {addr}");
            }
            Ok(())
        }
    }
}

/// Ask for the share password, only when someone can type it.
fn prompt_password() -> anyhow::Result<String> {
    use std::io::IsTerminal;
    let hint = || format!("This share needs a password: type it, or set {PASSWORD_ENV}");
    anyhow::ensure!(std::io::stdin().is_terminal(), hint());
    rpassword::prompt_password("Share password: ").with_context(hint)
}

/// Preview a share, asking for its password (up to three times) if the sender wants one.
async fn preview_with_password(
    node: &Node,
    link: &ShareLink,
    password: &mut Option<String>,
) -> anyhow::Result<Preview> {
    let mut tries = 0;
    loop {
        let err = match node.preview(link, password.clone()).await {
            Ok(preview) => return Ok(preview),
            Err(err) => err,
        };
        let interactive = std::env::var(PASSWORD_ENV).is_err();
        match err.downcast_ref::<AccessError>() {
            Some(AccessError::PasswordRequired | AccessError::WrongPassword)
                if interactive && tries < 3 =>
            {
                if password.is_some() {
                    eprintln!("{err}");
                }
                tries += 1;
                *password = Some(prompt_password()?);
            }
            _ => return Err(err),
        }
    }
}

/// Serve until Ctrl+C, printing upload activity.
async fn serve(node: &Node) -> anyhow::Result<()> {
    println!("serving; keep this running while others download. Ctrl+C to stop.");
    let mut events = node.subscribe();
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => return Ok(()),
            event = events.recv() => {
                if let Ok(Event::ShareActivity {
                    share_id,
                    active_peers,
                    uploaded_bytes,
                    bytes_per_sec,
                    ..
                }) = event
                {
                    eprint!(
                        "\r{}: {} peer(s) downloading, {} sent, {}/s      ",
                        &share_id[..8],
                        active_peers,
                        human(uploaded_bytes),
                        human(bytes_per_sec)
                    );
                }
            }
        }
    }
}

fn describe(kind: Option<ConnectionKind>) -> &'static str {
    match kind {
        Some(ConnectionKind::Direct) => "  [direct]",
        Some(ConnectionKind::Relay) => "  [via relay]",
        None => "",
    }
}

fn confirm(question: &str) -> anyhow::Result<bool> {
    print!("{question} [Y/n] ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer)? == 0 {
        // No terminal input (EOF): don't download unasked. Use --yes in scripts.
        println!();
        return Ok(false);
    }
    Ok(matches!(
        answer.trim().to_lowercase().as_str(),
        "" | "y" | "yes"
    ))
}

fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn skip_reason(reason: SkipReason) -> &'static str {
    match reason {
        SkipReason::SystemFile => "system file",
        SkipReason::Link => "symbolic link",
        SkipReason::EmptyFolder => "empty folder",
        SkipReason::Unreadable => "unreadable",
        SkipReason::NotAFile => "not a file",
    }
}
