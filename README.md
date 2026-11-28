<p align="center">
  <img src="branding/ferry-readme.png" alt="Ferry sends files straight from your computer" width="100%">
</p>

# <img src="app/src-tauri/icons/128x128.png" alt="" width="36" align="top"> Ferry

[![CI](https://github.com/Alexxfromgit/Ferry/actions/workflows/ci.yml/badge.svg)](https://github.com/Alexxfromgit/Ferry/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/Alexxfromgit/Ferry?sort=semver)](https://github.com/Alexxfromgit/Ferry/releases/latest)
[![License MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Ferry sends files and folders straight from one device to another, even across the internet.
There is no account and no cloud upload: the recipient downloads directly from you, on
Windows, macOS, Linux and Android.

## Download

<!-- x-release-please-start-version -->

Latest version: **0.4.0** · [Release notes](https://github.com/Alexxfromgit/Ferry/releases/tag/v0.2.0) · [All releases](https://github.com/Alexxfromgit/Ferry/releases)

| Platform | Download | Also available |
| --- | --- | --- |
| Windows x64 | [Installer (.exe)](https://github.com/Alexxfromgit/Ferry/releases/download/v0.4.0/Ferry-0.2.0-windows-x64.exe) | [.msi](https://github.com/Alexxfromgit/Ferry/releases/download/v0.2.0/Ferry-0.2.0-windows-x64.msi) |
| macOS, Apple silicon | [Disk image (.dmg)](https://github.com/Alexxfromgit/Ferry/releases/download/v0.4.0/Ferry-0.2.0-macos-arm64.dmg) | [.pkg](https://github.com/Alexxfromgit/Ferry/releases/download/v0.2.0/Ferry-0.2.0-macos-arm64.pkg) |
| macOS, Intel | [Disk image (.dmg)](https://github.com/Alexxfromgit/Ferry/releases/download/v0.4.0/Ferry-0.2.0-macos-x64.dmg) | [.pkg](https://github.com/Alexxfromgit/Ferry/releases/download/v0.2.0/Ferry-0.2.0-macos-x64.pkg) |
| Linux x64 | [AppImage](https://github.com/Alexxfromgit/Ferry/releases/download/v0.4.0/Ferry-0.2.0-linux-x64.AppImage) | [.deb](https://github.com/Alexxfromgit/Ferry/releases/download/v0.2.0/Ferry-0.2.0-linux-x64.deb) |
| Android 10+ | [APK from ferry.xenvaultinteractive.com](https://ferry.xenvaultinteractive.com/android) | |

<!-- x-release-please-end -->

Every release lists its checksums in `SHA256SUMS.txt` and carries GitHub build attestations.
[Verifying a download](docs/RELEASING.md#verifying-downloads) explains how to check them.

## Highlights

- **No account, no cloud.** Files go from your device to theirs and are never uploaded anywhere.
- **Private.** Transfers are end-to-end encrypted, and every byte is checked against a BLAKE3 hash.
- **Works across networks.** Ferry connects directly when it can; when a network blocks that, an
  encrypted relay passes the data along without being able to read it.
- **Resumes.** An interrupted download picks up where it stopped.
- **Share it your way.** A link, a QR code, or a `.ferry` ticket file. Links name your device
  and the share, never your IP address.
- **Passwords.** Lock a share with a password; your Ferry checks it before anyone gets in.
- **Phones too.** On Android, share to Ferry from any app's Share menu, scan a QR code to receive,
  and find received files in Downloads/Ferry.
- **Stays current.** Signed in-app updates on Windows, macOS, the Linux AppImage and Android.
- **Command line.** The `ferry` CLI uses the same transfer engine.

## How it works

1. **Share.** Pick files or a folder. Ferry gives you a ticket: a link, a QR code and an optional
   `.ferry` file. Your files stay where they are.
2. **Send the ticket.** Paste the link into any chat or email, or let someone scan the QR code.
3. **Receive.** The recipient opens it in Ferry, previews what's inside, and downloads straight
   from your device, directly or through a relay. Keep Ferry running with sharing on until
   they finish.

Treat links like keys. Anyone with an active link can download the share unless it has a
password. A link names the sender's device and a random token for the share; it carries no IP
addresses (unless relays are turned off). Ferry publishes each device's home relay, not its IP
addresses, through iroh's public discovery service so links keep working when a device changes
networks. Turn sharing off to stop access. [How Ferry connects](docs/NETWORKING.md)

## Platforms

| Platform | Updates | Status |
| --- | --- | --- |
| Windows x64 | In the app | Tested. Installers aren't code-signed yet, so SmartScreen asks you to confirm. |
| macOS (Apple silicon and Intel) | In the app | Tested. Signed ad hoc but not notarized, so open it with right-click → **Open** the first time. |
| Linux x64 | AppImage in the app; `.deb` through your package manager | Testing in progress. |
| Android 10+ | In the app (the APK checks for new versions) | Tested. |

Under **Settings → Updates**, Ferry shows its current version and can check for updates
automatically or on demand.

## Send and receive

1. On the sender's device, open **Send** and drop in files or a folder, or use **Choose files**
   or **Choose a folder**. On a phone, you can also pick **Ferry** in another app's Share menu.
2. Share the link, show the QR code, or save a `.ferry` ticket file.
3. On the receiving device, open the link or ticket file, scan the QR code, or paste the link
   into **Receive**. Preview the share and select **Download**.
4. On the sender's device, turn sharing off when you want to revoke access.

**Require a password** locks the share: whoever opens its link, QR code or ticket file has to
enter the password, and your Ferry checks it, so guesses can only be made online and slowly. Use
something hard to guess and send it separately from the link.

### Command line

The `ferry` CLI uses the same transfer engine. Build it with
`cargo build --locked --release -p ferry-cli` and find it in `target/release/`.
Run these examples with the binary on your PATH.

```sh
ferry share ./Photos --save-file photos.ferry
ferry get photos.ferry --dest ~/Downloads
ferry status
```

Keep the share command running until the transfer finishes. Run `ferry --help` for
relay and data directory options. Add `--password` to `share` to lock the share; `get` asks
for the password when the sender wants one. Both prompt without echoing it, or read
`FERRY_SHARE_PASSWORD` in scripts.

## Build from source

Install Rust stable with `rustfmt` and `clippy`, Node.js 24, and the native build tools
for your platform. On macOS, install the Xcode command line tools with `xcode-select --install`.
On Windows, use PowerShell 7.3 or later. [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/)

```sh
git clone https://github.com/Alexxfromgit/Ferry.git
cd Ferry
./build.sh
```

| Task | macOS and Linux | Windows |
| --- | --- | --- |
| Build the desktop app and CLI | `./build.sh` | `./build.ps1` |
| Run the CI checks locally | `./build.sh --check` | `./build.ps1 -Check` |
| Generate installers and build the CLI | `./build.sh --bundle` | `./build.ps1 -Bundle` |

Release binaries are written to `target/release/` and installers to `target/release/bundle/`.
The Android app builds from the same project; see [Android](CONTRIBUTING.md#android).
[CONTRIBUTING.md](CONTRIBUTING.md) covers development commands, a macOS smoke test, and
submitting changes.

## License

Ferry is available under the [MIT License](LICENSE). The artwork under `branding/` and the app
icons are the author's own and aren't covered by it; see the
[third-party notices](THIRD_PARTY_NOTICES.md).
