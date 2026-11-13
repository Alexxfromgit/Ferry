# How Ferry connects, and its limits

Ferry sends files directly from the sender's computer to the recipient's. There is no upload to
a cloud service. This page explains how the two computers find each other, what the optional
relay server does, and what to expect on difficult networks.

## The short version

- Every Ferry install has a permanent **device ID**: a public key, created on first start and
  stored in the app's data folder.
- A share link (`ferry://get?t=…`), a `.ferry` ticket file and the QR code all carry the same
  **ticket**: the sender's device ID, their last known addresses, and the BLAKE3 hash of the
  shared files.
- The recipient's Ferry connects to the sender's **device ID** using the addresses available.
  It can use hole punching through home routers or an **encrypted relay** when needed.
- Every byte is verified against the BLAKE3 hash while it arrives. Interrupted downloads resume
  where they stopped.

Ferry is built on [iroh](https://iroh.computer) (QUIC connections dialed by public key) and
`iroh-blobs` (content-addressed, verified, resumable transfers).

## Finding the sender

1. The ticket contains the sender's addresses at the time the link was made.
2. Addresses change, for example when a laptop moves to another Wi-Fi or a router gets a new IP.
   So Ferry also publishes `device ID → current addresses` to n0's DNS service (pkarr) and looks
   it up when dialing. Old links keep working as long as the sender's device ID stays the same.
3. The record includes the sender's **home relay** when available. The recipient can use it
   if both devices can reach the relay.

## Direct vs. relay

| Situation | Result |
| --- | --- |
| Same local network | Direct |
| Typical home routers on both sides | Usually direct, via hole punching |
| One side behind carrier-grade NAT (mobile data, some ISPs) | Often direct; otherwise relay |
| Strict corporate or hotel firewalls, UDP blocked | Relay, carried over HTTPS port 443 |

The Receive screen shows **Direct** or **Via relay** for each running download. A relay can
help when a direct connection fails. Public relays share bandwidth and apply rate limits.

**Relays can't read your files.** The connection is end-to-end encrypted between the two
devices (QUIC with TLS 1.3, keys bound to the device IDs). The relay only forwards encrypted
packets.

## Relay options (Settings → Connections)

| Option | When to use it |
| --- | --- |
| **Public relays** (default) | Free relays run by n0, the company behind iroh. No setup. No uptime guarantee, rate limited. Fine for personal use. |
| **My own relay** | Best reliability and speed through relays. Run [`iroh-relay`](https://github.com/n0-computer/iroh/tree/main/iroh-relay) on a small VPS (1 vCPU / 512 MB is plenty for personal use) with a domain and TLS, then enter its `https://` address. Both sides should use it for best results, but the sender's choice is what matters most. |
| **No relays** | Only direct connections. Useful on a LAN or for testing; many internet transfers will fail. |

Changing the relay setting takes effect after restarting Ferry.

### Self-hosting a relay (outline)

```bash
cargo install iroh-relay --features server
iroh-relay --config-path relay.toml   # see the iroh-relay README for a config example
```

Point a DNS name at the server, open the ports listed in the iroh-relay README (HTTPS 443, plus
HTTP 80 if it fetches Let's Encrypt certificates itself, plus its UDP address-discovery port), and
use `https://that-name` in Ferry. Use an `iroh-relay` version matching the iroh version in
`Cargo.lock`.

## Limitations

- **The sender must be online.** Recipients download from the sender's computer, so Ferry has to
  be running there. Closing the window keeps it running in the system tray; "Quit Ferry" stops
  sharing. Sleep or hibernation pauses sharing too.
- **Shared files are served in place, not copied.** Don't move, rename or edit them while
  shared. If a file changes, recipients get a verification error; share it again to make a new
  ticket.
- **A ticket works like a key.** Anyone who has the link can download while the share is
  active. Send links privately and use "Stop sharing" when done. Stopping takes effect
  immediately: new downloads are refused and downloads in progress are cut off. Your original
  files are never deleted.
- **A ticket contains your addresses.** Besides the device ID, a link includes the sender's
  current IP addresses (local network and public) and home relay, so whoever sees the link
  learns them. Your device ID and addresses are also published to n0's public DNS service so
  links keep working when your IP changes; the files themselves and what you share are not
  published anywhere.
- **Discovery uses n0's DNS service.** If it's unreachable, links still work while the sender's
  addresses in the ticket are current.
- **Downloads need free space twice, briefly.** Received data first goes into Ferry's own
  storage (in the local app data folder) and is then moved to the chosen folder. On the same
  drive this is a quick move; on another drive it is a copy, so the system drive needs room for
  the whole download until it finishes. Ferry's copy is cleaned up within about ten minutes.
- **Names that can't be saved everywhere are refused.** A share containing file names that are
  invalid on Windows (such as `NUL` or names ending in a dot) or names that differ only in case
  is rejected before anything is downloaded, so no file is silently lost or merged.
- **No LAN auto-discovery yet.** Two devices on the same network connect directly from the
  ticket's addresses, but there is no "nearby devices" list.
- **Windows firewall** may ask about network access on first start. Allowing it on private
  networks is enough for most cases.
- **Mobile:** Android is planned.

## Where data lives

| What | Where (Windows; macOS and Linux use the platform equivalents) |
| --- | --- |
| Device identity, shares, partial downloads | `%LOCALAPPDATA%\dev.ferry.desktop\node\` |
| Settings | `%APPDATA%\dev.ferry.desktop\settings.json` |
| Log (one previous log kept as `ferry.log.1`) | `%LOCALAPPDATA%\dev.ferry.desktop\logs\ferry.log` |

The node folder is in the local (not roaming) profile because partial downloads can be large.
Earlier versions kept it under `%APPDATA%`; Ferry moves it on first start.

Deleting `node\secret.key` gives the device a new ID and **breaks every link you have sent**.
