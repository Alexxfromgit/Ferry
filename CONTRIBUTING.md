# Contributing to Ferry

Use the issue forms for bugs and feature requests. Use sample files when reporting a transfer
problem and redact tickets, device IDs, IP addresses, and personal paths from public reports.
For vulnerabilities, follow the [security policy](SECURITY.md).

## Develop locally

Start with the [build prerequisites](README.md#build-from-source). On Ubuntu 22.04,
install the same native packages used by CI.

```sh
sudo apt-get update
sudo apt-get install -y build-essential libssl-dev libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev librsvg2-dev patchelf libxdo-dev libfuse2 file xdg-utils
```

Run these commands from the repository root. On Windows, use `npm.cmd` if PowerShell's
execution policy blocks the npm wrapper.

```sh
npm --prefix app ci
npm --prefix app run tauri dev
```

Use `npm --prefix app run dev` to work on the interface in a browser with sample data.
Run `./build.sh --check` on macOS or Linux or `./build.ps1 -Check` on Windows before opening a PR.
The check builds the web assets for the desktop crate, runs the frontend and Rust tests,
and checks formatting and lint. The core and CLI also have a Rust 1.91 CI job.

| Area | Location |
| --- | --- |
| Transfer engine and integration tests | `crates/ferry-core/` |
| CLI | `crates/ferry-cli/` |
| Desktop commands, tray, and link handling | `app/src-tauri/` |
| Svelte interface and tests | `app/src/` |

When changing a Rust type used by the UI, regenerate `app/src/lib/contract.generated.ts`
with `UPDATE_CONTRACT=1 cargo test --locked -p ferry-core --test contract`. In PowerShell,
set `$env:UPDATE_CONTRACT = '1'` before running that test and clear it afterward. Update
`app/src/lib/types.ts` as needed and run the frontend type check.

When changing the app or ticket icon, update its source under `app/src-tauri/icons/` and run
`pwsh -File scripts/make-icons.ps1` on Windows. After dependency updates, regenerate
`THIRD_PARTY_NOTICES.md` with `node scripts/third-party-notices.mjs` after installing the app dependencies.

## macOS smoke test

1. From a clean clone, run `./build.sh --check` and `./build.sh --bundle`. Open the DMG in
   `target/release/bundle/dmg/` and copy Ferry to Applications.
2. Launch Ferry. Confirm the menu bar item works, closing the window leaves the app running,
   and **Quit Ferry** exits.
3. With Ferry closed and then again with it running, open a sample `.ferry` file or a
   `ferry://` link from a second device. Confirm the Receive view opens with the correct share.
4. Transfer a sample file and folder in both directions with the second device. Compare the
   files, try pausing and resuming a download, and confirm **Stop sharing** blocks a fresh request.
5. Repeat a transfer with the devices on different networks to exercise internet discovery.
   Record the macOS version, chip, and whether Ferry reported **Direct** or **Via relay**.

## Pull requests

Use a Conventional Commit title such as `feat: add share expiry` or
`fix: restore the download queue`. GitHub uses the PR title as the squash commit subject.
Release Please uses that subject in the changelog, so describe the change in terms users will
recognize. Include what you tested and the platforms involved. [Conventional Commits](https://www.conventionalcommits.org/)

Maintainers can follow the [release guide](docs/RELEASING.md).
