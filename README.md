<p align="center">
  <img src="branding/ferry-readme.png" alt="Ferry sends files straight from your computer" width="100%">
</p>

# Ferry

[![CI](https://github.com/Alexxfromgit/Ferry/actions/workflows/ci.yml/badge.svg)](https://github.com/Alexxfromgit/Ferry/actions/workflows/ci.yml)
[![License MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Send files and folders from your computer to another device, even across the internet. Ferry
creates a share ticket you can send as a link or a small `.ferry` file. The recipient opens
it in Ferry and downloads from your device. There is no account or cloud upload.

- Ferry connects directly when possible and can use a relay when the network blocks a direct connection.
- Transfers are end-to-end encrypted, verified against a BLAKE3 hash, and can resume after an interruption.
- Shared files stay in their original location. Keep Ferry running and the files in place until the recipient finishes.

## Get Ferry

Installers are published on the [Releases page](https://github.com/Alexxfromgit/Ferry/releases).
If there are no installers yet, you can [build Ferry from source](#build-from-source).

| Platform | Installer |
| --- | --- |
| Windows x64 | `.exe` or `.msi` |
| macOS | `.dmg` for Apple silicon (arm64) or Intel (x64) |
| Linux x64 | `.deb` or `.AppImage` built on Ubuntu 22.04 |

Ferry is an early desktop release. Windows has received manual testing. macOS and Linux
desktop testing is in progress. Initial Windows installers are unsigned and macOS builds
are unnotarized. The [release guide](docs/RELEASING.md#verifying-downloads) explains how
to verify a download.

## Send and receive

1. On the sender's device, open **Send** and drop in files or a folder. You can also use **Choose files** or **Choose a folder**.
2. Share the link or save a `.ferry` ticket file. The QR code carries the same ticket.
3. On the receiving device, open the link or ticket file, or paste the link into **Receive**. Preview the share, choose a destination, and select **Download**.
4. On the sender's device, select **Stop sharing** when you want to revoke access.

Treat tickets like passwords. Anyone with an active ticket can request the share, and the
ticket includes the sender's device ID and network addresses. Ferry also publishes device
address records through iroh's public discovery service so tickets can keep working when a
device changes networks. [How Ferry connects](docs/NETWORKING.md)

**Save ticket file** creates a binary `.ferry` file that opens directly in Ferry. Choose
**Save with password** to encrypt the ticket and its contents. Use a strong passphrase and
send it separately. A link, QR code, or ordinary ticket for the same share still grants
access on its own. Update Ferry on the receiving device to open the new files. Current Ferry
versions can also read older JSON tickets.

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
relay and data directory options.

To save an encrypted ticket, use `ferry share ./Photos --save-file protected-ticket.ferry --protect`.
Both `share` and `get` prompt for the password in the terminal without echoing it.

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
See [CONTRIBUTING.md](CONTRIBUTING.md) for development commands, a macOS smoke test,
and guidance on submitting changes.

## License

Ferry is available under the [MIT License](LICENSE).
[Third-party notices](THIRD_PARTY_NOTICES.md)
