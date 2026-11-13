# Releasing Ferry

Ferry uses Release Please and GitHub Actions. Feature and fix PRs are squash-merged
with Conventional Commit titles. Release Please opens a PR that updates the changelog,
Cargo workspace and lockfile, npm metadata and lockfile, and Tauri version. Merging it
creates a version tag and draft GitHub release.

The Release workflow builds installers for Windows x64, Apple silicon and Intel Macs,
and Linux x64. It runs CI against the tagged source and publishes the draft when all checks
and builds pass and the installers have SHA-256 checksums and GitHub build attestations.

## Configure automation

Add `RELEASE_TOKEN` as a repository Actions secret. Use a GitHub App installation token
or a repository-scoped fine-grained personal access token with **Contents** and
**Pull requests** read/write permissions. Using a token distinct from `GITHUB_TOKEN`
lets the generated release PR trigger its CI checks. Run **Release** on `main` after adding
the secret to start Release Please.
[Release Please Action](https://github.com/googleapis/release-please-action)

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
