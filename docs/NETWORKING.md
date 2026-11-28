# How Ferry connects, and its limits

Ferry sends files directly from the sender's computer to the recipient's. There is no upload to
a cloud service. This page explains how the two computers find each other, what the optional
relay server does, and what to expect on difficult networks.

## The short version

- Every Ferry install has a permanent **device ID**: a public key, created on first start and
  stored in the app's data folder.
- A share link (short: `https://ferry.xenvaultinteractive.com/s/K7X2QP9A4MZT`, or full:
  `…/s#…`), a `.ferry` ticket file and the QR code all carry the same two things: the sender's **device ID** and the share's **token**, a
  random secret made for that share. They don't contain the sender's IP addresses or anything
  about the files. The link data sits after the `#`, which browsers never send to the web
  server; the page there only hands the link to Ferry (`ferry://s#…`) or offers to install it.
  A short link's code is looked up through Ferry's link service; [Share links](LINKS.md)
  explains the formats and what the service sees.
- The recipient's Ferry connects to the sender's device ID, through hole punching or an
  **encrypted relay**, and presents the token. The sender's Ferry checks it (and the share's
  password, if it has one) before it serves anything. Without the token, knowing what is shared
  gets nobody anything.
- Every byte is verified against the share's BLAKE3 hash while it arrives. Interrupted downloads
  resume where they stopped, without asking for the password again.

Ferry is built on [iroh](https://iroh.computer) (QUIC connections dialed by public key) and
`iroh-blobs` (content-addressed, verified, resumable transfers).

## Finding the sender

1. Ferry publishes `device ID → home relay` to n0's DNS service (pkarr), and the recipient's
   Ferry looks it up when dialing. The record holds only the relay's address, never the
   device's IP addresses.
2. The recipient reaches the sender through that relay, and the two devices then try to switch
   to a direct connection.
3. Links keep working as long as the sender's device ID stays the same, even after its IP
   address changes.
4. With **No relays**, there is nothing to look up, so links carry the sender's direct
   addresses instead and work only while those are current.

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
- **A link works like a key.** Anyone who has the link can download while the share is
  active, unless the share has a password. Send links privately and use "Stop sharing" when
  done. Stopping takes effect immediately: new downloads are refused and downloads in progress
  are cut off. Your original files are never deleted.
- **A password locks the share itself.** With **Require a password**, the link, the QR code
  and the ticket file all need the password, and your Ferry checks it. It only ever stores a
  hash of the password. Each device gets a few guesses and then one every ten seconds, and all
  devices together about 30 a minute; a right password doesn't count, so someone guessing
  doesn't lock out the real recipient for long. Ticket files hold only the link data, never
  anything that would let a password be tested offline. Devices that gave the password keep
  access, so their downloads can resume, until you change the password or remove the share.
  Use something that is hard to guess and send it separately from the link.
- **Discovery uses n0's DNS service.** It holds your device ID and home relay, not your IP
  addresses, the files or what you share. If it's unreachable, the recipient can't find you
  unless both devices use the same relay or relays are off.
- **Transfers are private, not anonymous.** While connecting, the two devices exchange their
  addresses to try a direct connection, so each side can learn the other's IP address, even
  when the download ends up going through a relay. Ferry doesn't show or log it, but network
  tools on that device can see it. The relay in use sees both devices' IP addresses and how
  much data passes, though not the data itself. To hide your IP address, use a VPN.
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
- **On Android, Ferry works with copies.** Phones download into Ferry's own storage, then copy
  the files to Downloads/Ferry. To send, Ferry first copies the picked files (or files shared to
  it from another app's Share menu) into its own storage, because Android only lends them to
  the app for a while; the copy is deleted when the share is removed. Sharing from a phone
  needs that much free space.
- **Phones stay reachable through a notification.** While a transfer runs or a share is on, a
  notification keeps Ferry connected with the screen off; its **Stop sharing** button turns all
  shares off. Android 15 allows this for about six hours a day. When the time is up, Ferry turns
  its shares off and says so; opening Ferry and turning them on again resets the limit. iOS
  comes later.

## Where data lives

| What | Where (Windows; macOS and Linux use the platform equivalents) |
| --- | --- |
| Device identity, shares, partial downloads | `%LOCALAPPDATA%\com.xenvaultinteractive.ferry\node\` |
| Settings | `%APPDATA%\com.xenvaultinteractive.ferry\settings.json` |
| Log (one previous log kept as `ferry.log.1`) | `%LOCALAPPDATA%\com.xenvaultinteractive.ferry\logs\ferry.log` |

The node folder is in the local (not roaming) profile because partial downloads can be large.
Earlier versions kept it under `%APPDATA%`, and versions up to 0.2 used folders named
`dev.ferry.desktop`; Ferry moves both on first start.

Deleting `node\secret.key` gives the device a new ID and **breaks every link you have sent**.
