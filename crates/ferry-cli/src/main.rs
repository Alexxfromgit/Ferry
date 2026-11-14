//! `ferry` command-line client: the same engine as the desktop app, for headless testing.

use std::{
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::Context;
use clap::{Parser, Subcommand};
use ferry_core::{
    Config, ConnectionKind, DownloadStatus, Event, Node, RelayChoice, SHARE_FILE_EXTENSION,
    ShareLink, SkipReason, TicketFile, TicketPassword,
};

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
        /// Prompt for a password to protect the saved ticket file.
        #[arg(long, requires = "save_file")]
        protect: bool,
    },
    /// Keep serving all existing shares until Ctrl+C.
    Serve,
    /// Download a share from a share link, a ticket, or a .ferry file.
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
            protect,
        } => {
            let password = if protect {
                let password = rpassword::prompt_password("Ticket password: ")?;
                anyhow::ensure!(!password.trim().is_empty(), "Enter a password");
                let confirmation = rpassword::prompt_password("Confirm ticket password: ")?;
                let matches = password == confirmation;
                let password = TicketPassword::from(password.into_boxed_str());
                drop(TicketPassword::from(confirmation.into_boxed_str()));
                anyhow::ensure!(matches, "Passwords do not match");
                Some(password)
            } else {
                None
            };
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
            let share = node.create_share(paths).await?;
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
                node.share_file(&share.id)?.save(&path, password)?;
                println!("share file written to {}", path.display());
            }
            if protect {
                println!("Send the ticket file and share its password separately.");
            } else {
                println!("\nsend this link to the recipient:\n\n{}\n", share.link);
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
            let link = if path.is_file() {
                match TicketFile::read(path)? {
                    TicketFile::Ready(link) => link,
                    TicketFile::Protected(ticket) => {
                        let password = rpassword::prompt_password("Ticket password: ").context(
                            "Opening this ticket requires an interactive password prompt",
                        )?;
                        ticket.unlock(TicketPassword::from(password.into_boxed_str()))?
                    }
                }
            } else {
                ShareLink::parse(&link)?
            };
            eprintln!("connecting to the sender...");
            let preview = node.preview(&link).await?;
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
            let started = node.start_download(&link, dest).await?;
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
                    "{}  {}  ({} files, {})\n  {}",
                    share.id,
                    share.name,
                    share.file_count,
                    human(share.total_size),
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
