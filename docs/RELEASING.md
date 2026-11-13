# Releasing Ferry

Ferry uses Release Please and GitHub Actions. Feature and fix PRs are squash-merged
with Conventional Commit titles. Release Please opens a PR that updates the changelog,
Cargo workspace and lockfile, npm metadata and lockfile, and Tauri version. Merging it
creates a version tag and draft GitHub release.

The Release workflow builds installers for Windows x64, Apple silicon and Intel Macs,
and Linux x64. It runs CI against the tagged source and publishes the draft when all checks
and builds pass. It uploads SHA-256 checksums, GitHub build attestations, and a Tauri
update feed with signed packages for Windows MSI and NSIS, both Macs, and Linux AppImage.

## Configure automation

Add `RELEASE_TOKEN` as a repository Actions secret. Use a GitHub App installation token
or a repository-scoped fine-grained personal access token with **Contents** and
**Pull requests** read/write permissions. Using a token distinct from `GITHUB_TOKEN`
lets the generated release PR trigger its CI checks. Run **Release** on `main` after adding
the secret to start Release Please.
[Release Please Action](https://github.com/googleapis/release-please-action)

### Configure updater signing

Generate a dedicated Tauri updater key from the repository root after installing
the app dependencies. Use a path outside the repository and keep a secure backup of
the private key and its password.

```sh
npm --prefix app run tauri -- signer generate -w "$HOME/ferry-updater.key"
```

In the repository's **Settings → Secrets and variables → Actions**, set these values.

| Kind | Name | Value |
| --- | --- | --- |
| Variable | `FERRY_UPDATER_PUBLIC_KEY` | Contents of `ferry-updater.key.pub` |
| Secret | `TAURI_SIGNING_PRIVATE_KEY` | Contents of `ferry-updater.key` |
| Secret | `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | The password chosen when generating the key, if any |

Keep the same signing key for subsequent releases so installed copies can verify updates.
The release workflow embeds the public key in each build and checks that the generated
signatures identify that key and cover the tagged version before publishing `latest.json`.
Local builds can exercise signing by supplying the same environment variables with a test
key. Tauri updater signing is independent of OS code signing.
[Tauri updater](https://v2.tauri.app/plugin/updater/)

## Build a draft release

To ship the initial version from the current source or retry a draft, create or verify
a `vX.Y.Z` tag that matches the version in the source. Create a draft GitHub release for
that existing tag. From **Actions → Release → Run workflow**, select the tag in the
ref selector. This builds and publishes the draft after the checks pass. The same tag
can be dispatched again if a build fails while the release is still a draft.

For local packaging, run `./build.sh --bundle` or `./build.ps1 -Bundle`.
Use the [macOS smoke test](../CONTRIBUTING.md#macos-smoke-test) to check file
associations, links, the menu bar item, and transfers on a real Mac.

## Verifying downloads

Compare your installer hash with the matching entry in `SHA256SUMS.txt`.

```powershell
Get-FileHash -Algorithm SHA256 ./Ferry-0.1.0-windows-x64.exe
```

```sh
shasum -a 256 Ferry-0.1.0-macos-arm64.dmg
gh attestation verify Ferry-0.1.0-macos-arm64.dmg --repo Alexxfromgit/Ferry
```
