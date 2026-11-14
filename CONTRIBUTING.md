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
  libayatana-appindicator3-dev librsvg2-dev libdbus-1-dev pkg-config patchelf libxdo-dev libfuse2 file xdg-utils
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
`pwsh -File scripts/make-icons.ps1` on Windows. It also writes the iOS icons and the Android
adaptive icon, and copies the Android set into `app/src-tauri/gen/android`. When changing the mascot artwork, update its
source under `branding/art/` and run `python3 scripts/make-art.py` (needs Pillow with WebP
support). It writes the in-app images and the installer images. After dependency updates, regenerate
`THIRD_PARTY_NOTICES.md` with `node scripts/third-party-notices.mjs` after installing the app dependencies.
It lists the Android libraries through Gradle, so set `JAVA_HOME` to the JDK the Android build
uses (see below); without one it keeps the Android list already in the file.

## Android

The Android app is the same Tauri app (`app/src-tauri`) with its Android Studio project in
`app/src-tauri/gen/android`. Phones receive only for now. You need Android Studio's SDK with
an NDK, JDK 21 (this Gradle version doesn't run on newer JDKs), and the Rust Android targets,
which `npm --prefix app run tauri android init` installs. Set `JAVA_HOME` to the JDK 21 and
`NDK_HOME` to the NDK folder, then:

```sh
npm --prefix app run tauri android dev                                   # on a connected phone
npm --prefix app run tauri android build -- --debug --apk --target aarch64
```

Ferry ships two Android builds from one project, chosen with the Gradle property `ferryDist`
(set `ORG_GRADLE_PROJECT_ferryDist=play` or `direct`):

| | `play` (default) | `direct` |
| --- | --- | --- |
| Package | `com.xenvaultinteractive.ferry` | `com.xenvaultinteractive.ferry.direct` |
| Ships through | Google Play | GitHub releases and ferry.xenvaultinteractive.com |
| Updates | Play in-app updates | `latest.json` on downloads.xenvaultinteractive.com |
| Channel code | `gen/android/app/src/play` | `gen/android/app/src/direct` |

The Rust side reads the channel as `env!("FERRY_DISTRIBUTION")`. Phone-only Rust lives in
`app/src-tauri/src/android.rs` (saving to Downloads/Ferry, the transfer notification) and
`android_updates.rs`. Their Kotlin halves are the `ferry-mobile` plugin in
`app/src-tauri/plugins/ferry-mobile` and `UpdatePlugin` in each channel folder.
Release signing reads `gen/android/keystore.properties` (gitignored) or environment variables;
see [Releasing](docs/RELEASING.md#android). CI builds both channels as debug APKs and checks
them with `scripts/android-release.mjs`, whose tests run with `node --test scripts/`.

To preview the phone layout in a browser, run `npm --prefix app run dev` and open
`http://localhost:1420/?mobile` with the browser's device toolbar.

## macOS smoke test

1. From a clean clone, run `./build.sh --check` and `./build.sh --bundle`. Open the DMG in
   `target/release/bundle/dmg/` and copy Ferry to Applications, or open the installer package in
   `target/release/bundle/pkg/`, which installs Ferry into Applications.
2. Launch Ferry. Confirm the menu bar item works, closing the window leaves the app running,
   and **Quit Ferry** exits.
3. With Ferry closed and then again with it running, open a sample `.ferry` file or a
   share link from a second device (the web page hands off to `ferry://`). Confirm the Receive view opens with the correct share.
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
