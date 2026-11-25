# Share links and the link service

Every Ferry share has a link in two forms:

| Form | Example | Needs the link service |
| --- | --- | --- |
| Short | `https://ferry.xenvaultinteractive.com/s/K7X2QP9A4MZT` | Yes |
| Full | `https://ferry.xenvaultinteractive.com/s#AXbq…` (about 106 characters) | No |

Both name the same thing: the sender's **device ID** (its public key) and the share's **token**
(16 random bytes made for that share). Neither contains IP addresses, file names or the files'
hash. The `ferry://s/<CODE>` and `ferry://s#<payload>` forms open an installed app directly.

Ferry shows the short link and its code once the link service has accepted it, and the full
link until then, or when short links are turned off (Settings → Connections → Short links, or
`--no-short-links`) or relays are off. `.ferry` files always hold the full link data.

## The full link

The part after `#` is the URL-safe base64 (no padding) of:

| Bytes | Content |
| --- | --- |
| 1 | Version, `1` |
| 32 | Device ID |
| 16 | Share token |
| 0–8 × (1 + 4 or 16 + 2) | Only when the sender uses no relays: its direct addresses, each a tag (`4` or `6`), the IP address and a big-endian port |

Browsers never send the fragment to the server, so the website can't see it.

## The short code

```
CODE = Crockford base32 of the first 60 bits of
       BLAKE3("ferry-link-v1" || device ID (32 bytes) || token (16 bytes))
```

Bits are taken most significant first, giving 12 characters from
`0123456789ABCDEFGHJKMNPQRSTVWXYZ`. Codes are read case-insensitively, `O` as `0` and `I`/`L`
as `1`, and Ferry shows them grouped as `K7X2-QP9A-4MZT`.

Test vector: device ID bytes `01 02 … 20`, token bytes `a0 a1 … af` → `6N76Y6BE0EQX`.

Because the code is derived from the device and token, a recipient's Ferry recomputes it from
the service's answer and refuses an answer that doesn't match. The service can't point a code
at another device. Finding a different device and token for the same 60-bit code takes about
2⁶⁰ hashes. The connection itself is authenticated by the device's key anyway.

## The API

All on `https://ferry.xenvaultinteractive.com`. Bodies and answers are JSON. Byte strings are
lower-case hex, and times are unix seconds. Integers in signed messages are big-endian.

### `PUT /api/links/<CODE>`: register or refresh

```json
{ "endpoint": "<64 hex>", "token": "<32 hex>", "relay": "https://…" | null,
  "expires": 1800000000, "sig": "<128 hex>" }
```

- `sig` is the device key's Ed25519 signature over
  `"ferry-link-v1" || CODE (ASCII) || endpoint (32) || token (16) || u16 relay length || relay (UTF-8) || u64 expires`.
- The service checks that `CODE` matches the derivation above and that the signature verifies.
  It caps `expires` at 31 days from now.
- It answers `204`, or `409` if a live code already belongs to another share.
- Ferry asks for 30 days and registers again every 12 hours while a share exists, so links
  don't expire while you're sharing.

### `GET /api/links/<CODE>`: look up

Answers `{ "endpoint", "token", "relay", "expires", "sig" }`, or `404` when the code is
unknown or expired. Lookups are limited per IP address, and over the limit the answer is `429`.

The recipient's Ferry checks `sig` over the registration message above before it uses `relay`,
so the service can't send recipients to a relay of its choosing; without a valid signature the
relay is ignored and the device is found through address lookup.

### `DELETE /api/links/<CODE>`: remove

```json
{ "endpoint": "<64 hex>", "timestamp": 1800000000, "sig": "<128 hex>" }
```

- `sig` signs `"ferry-link-del-v1" || CODE || endpoint (32) || u64 timestamp`.
- The timestamp must be within five minutes of the server's clock, and only the device that
  registered a code can remove it.
- Ferry removes a share's code when you remove the share. A share you only turn off keeps its
  code, so the link works again when you turn it back on.

## What the service sees

- **Stored:** the code, device ID, token, home relay, expiry time and the registration's
  signature. No IP addresses, no file names, no passwords. Because it holds the token, whoever
  runs the service could open a share that has no password, just like anyone the link was sent
  to; a password protects a share from the service too.
- **Seen in passing:** the IP address of each device that registers or looks up a code.
  Cloudflare, which runs the service, handles those requests like any web request.

If that matters to you, turn short links off and send full links instead. Turning them off in
Settings also removes your existing short links from the service. A share removed while short
links are off keeps its code until it expires (30 days at most).

A code works like the link it stands for: whoever has it can ask the sender for the share.
Locking the share with a password protects it in every form, as
[the networking guide](NETWORKING.md) explains.
