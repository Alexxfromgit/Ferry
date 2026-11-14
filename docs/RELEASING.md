# Releasing Ferry

Ferry uses Release Please and GitHub Actions. Feature and fix PRs are squash-merged
with Conventional Commit titles. Release Please opens a PR that updates the changelog,
Cargo workspace and lockfile, npm metadata and lockfile, and Tauri version. Merging it
creates a version tag and draft GitHub release.

The Release workflow builds installers for Windows x64, Apple silicon and Intel Macs,
and Linux x64. It runs CI against the tagged source and publishes the draft when all checks
and builds pass. It uploads SHA-256 checksums, GitHub build attestations, and a Tauri
update feed with signed packages for Windows MSI and NSIS, both Macs, and Linux AppImage.
Once the [Android setup](#android) is done, the same run also publishes the Android app.

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
that existing tag. From **Actions → Release → Run workflow**, keep `main` in the ref
selector and enter the tag in the **tag** field, or run
`gh workflow run release.yml --ref main -f tag=vX.Y.Z`. The workflow from `main` builds
the tagged source and publishes the draft after the checks pass. Run it again with the
same tag if a build fails while the release is still a draft.

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

The Android APK isn't on the GitHub release; its SHA-256 is in
`https://downloads.xenvaultinteractive.com/ferry/android/latest.json`, and the attestation
check works the same way. To check who signed it, compare the SHA-256 digest that
`apksigner verify --print-certs Ferry-0.3.0-android.apk` prints with the Direct certificate
fingerprint published on ferry.xenvaultinteractive.com.

```sh
gh attestation verify Ferry-0.3.0-android.apk --repo Alexxfromgit/Ferry
```

## Android

The Android app ships in two channels built from the same project (see
[CONTRIBUTING](../CONTRIBUTING.md#android)). Neither goes on the GitHub release.

| | Direct | Google Play |
| --- | --- | --- |
| Package | `com.xenvaultinteractive.ferry.direct` | `com.xenvaultinteractive.ferry` |
| Published as | APK on `downloads.xenvaultinteractive.com` (Cloudflare R2) | Bundle on the internal testing track |
| Signed with | The Direct key, which you keep | Your upload key; Google re-signs with the app signing key |
| Updates | Installed copies read `ferry/android/latest.json` | Google Play |

The Release workflow's Android jobs run only when the variable `FERRY_DIRECT_CERT_SHA256` is
set. Until then, releases ship the desktop app alone. With it set:

1. **Android** builds the Direct APK (arm64 and armv7) and the Play bundle (arm64, armv7 and
   x86_64) from the tag. It checks the APK's package, version, signer and that it isn't
   debuggable.
2. After the GitHub release is published, **Publish Android (Direct)** uploads the APK to
   `ferry/android/releases/<versionCode>/Ferry-X.Y.Z-android.apk`. It checks the APK downloads
   intact from the public host and attests it, and only then replaces `ferry/android/latest.json`.
   It never replaces the feed with an older version, and does nothing if this version is already
   live.
3. **Publish Android (Google Play)** uploads the bundle and its R8 mapping file to the internal
   track. Promote it to closed testing or production in Play Console. Google Play refuses a
   version code it already has, so after a partial failure use **Re-run failed jobs** rather than
   re-running everything.

### Create the keys and accounts

Keep the keys in a folder outside the repository. Create every password in your password manager
and save it there straight away. The commands are for PowerShell 7. They use full paths because
.NET methods such as `[IO.File]::ReadAllBytes` don't follow `cd`. `keytool` comes with the JDK
but often isn't on `PATH`, so they call it from `JAVA_HOME`; any JDK works for this.

```powershell
$keys = "$HOME\keys\ferry"
New-Item -ItemType Directory -Force $keys | Out-Null
$keytool = Join-Path $env:JAVA_HOME 'bin\keytool.exe'
```

**Direct signing key.** Create it, then note the `SHA256:` fingerprint the second command prints.

```powershell
& $keytool -genkeypair -v -storetype PKCS12 -keystore "$keys\ferry-direct.p12" -alias ferry-direct -keyalg RSA -keysize 4096 -validity 10000 -dname "CN=Ferry, O=Xenvault Interactive"
& $keytool -list -v -keystore "$keys\ferry-direct.p12" -alias ferry-direct | Select-String 'Alias name|SHA256:'
```

PKCS12 keystores use one password for the store and the key. The `-dname` text is public: it's
inside every APK. Back up `ferry-direct.p12` and its password offline as well. **Nothing can
replace this key**: without it Direct users can't update, and have to uninstall and reinstall.

**Play upload key.** Repeat with `-keystore "$keys\ferry-upload.p12" -alias upload`. Play
support can reset a lost or leaked upload key.

**Google Play Console.**

1. Create the app: name Ferry, App, Free.
2. Build the first bundle locally (below) and upload it under **Testing → Internal testing →
   Create release**, keeping the Google-generated app signing key. This first upload sets the
   package name.
3. Under **Test and release → App integrity → App signing**, copy the app signing key's
   SHA-256. The website's `assetlinks.json` needs it along with the Direct fingerprint.
4. Under Android developer verification, register `com.xenvaultinteractive.ferry.direct` with
   the Direct fingerprint, so the APK keeps installing on certified devices.

**Play service account**, for the workflow's uploads.

1. In the Google Cloud console, create a project and enable the **Google Play Android
   Developer API**.
2. **IAM & Admin → Service accounts → Create** an account with no Cloud roles. Under **Keys →
   Add key**, download a JSON key.
3. In Play Console, **Users and permissions → Invite new users**: add the service account's
   email with access to Ferry and the **Release apps to testing tracks** and **View app
   information** permissions. It can take a day to start working.

**Cloudflare R2.**

1. Create a bucket, for example `xenvault-downloads`.
2. Under the bucket's **Settings → Custom Domains**, connect `downloads.xenvaultinteractive.com`.
   Leave the public `r2.dev` URL disabled.
3. Add a CORS policy allowing `GET` and `HEAD` from `https://ferry.xenvaultinteractive.com`,
   for the website's download page.
4. Under **R2 → Manage API tokens**, create an account API token with **Object Read & Write**
   for that bucket only. Copy the access key ID and secret access key.

### Configure GitHub

Create an environment named `android-release` under **Settings → Environments**, and limit its
deployment branches to `main`. The values below go in that environment, so only the Android
release jobs receive them.

Set them with the GitHub CLI. Without `--body`, `gh secret set` prompts for the value, which
keeps passwords out of your shell history.

```powershell
$repo = 'Alexxfromgit/Ferry'
$direct = [Convert]::ToBase64String([IO.File]::ReadAllBytes("$keys\ferry-direct.p12"))
gh secret set ANDROID_DIRECT_KEYSTORE_B64 --env android-release --repo $repo --body $direct
$upload = [Convert]::ToBase64String([IO.File]::ReadAllBytes("$keys\ferry-upload.p12"))
gh secret set ANDROID_UPLOAD_KEYSTORE_B64 --env android-release --repo $repo --body $upload
gh secret set ANDROID_DIRECT_KEYSTORE_PASSWORD --env android-release --repo $repo
gh secret set PLAY_SERVICE_ACCOUNT_JSON --env android-release --repo $repo --body (Get-Content "$keys\play-service-account.json" -Raw)
gh variable set FERRY_DIRECT_CERT_SHA256 --env android-release --repo $repo --body 'AB:CD:…'
```

| Kind | Name | Value |
| --- | --- | --- |
| Secret | `ANDROID_DIRECT_KEYSTORE_B64` | The Direct keystore, base64 |
| Secret | `ANDROID_DIRECT_KEYSTORE_PASSWORD` | Its password |
| Secret | `ANDROID_UPLOAD_KEYSTORE_B64` | The upload keystore, base64 |
| Secret | `ANDROID_UPLOAD_KEYSTORE_PASSWORD` | Its password |
| Secret | `PLAY_SERVICE_ACCOUNT_JSON` | The service account's JSON key |
| Secret | `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY` | The R2 API token's keys |
| Variable | `R2_ACCOUNT_ID` | Your Cloudflare account ID: 32 hex characters, also the first part of the bucket's S3 API URL |
| Variable | `R2_BUCKET` | The bucket name |
| Variable | `FERRY_DIRECT_CERT_SHA256` | The Direct fingerprint; setting it turns the Android jobs on |
| Variable | `PLAY_RELEASE_STATUS` | Optional. `completed` once the app's first review has passed, so internal testers get each build at once. Until then Google Play only accepts `draft` releases, the default |

`ANDROID_DIRECT_KEY_ALIAS` and `ANDROID_UPLOAD_KEY_ALIAS` variables override the default key
aliases, `ferry-direct` and `upload`. Without `PLAY_SERVICE_ACCOUNT_JSON`, the Play job leaves
the bundle in the run's `android-play` artifact for a manual upload. Keep the keystores and the
JSON key backed up in your password manager.

### Build a signed bundle or APK locally

The Gradle build reads signing keys from environment variables, or from
`app/src-tauri/gen/android/keystore.properties` (gitignored) with keys such as
`uploadStoreFile`, `uploadStorePassword`, `uploadKeyAlias` and `uploadKeyPassword`. For the
first Play upload, in PowerShell 7 from the repository root, with the JDK 21 and NDK the
[Android build](../CONTRIBUTING.md#android) uses (adjust both paths to your installs):

```powershell
$env:JAVA_HOME = 'C:\Program Files\Java\jdk-21.0.12'
$env:NDK_HOME = Get-ChildItem "$env:LOCALAPPDATA\Android\Sdk\ndk" | Sort-Object Name | Select-Object -Last 1 -ExpandProperty FullName
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
$env:ORG_GRADLE_PROJECT_ferryDist = 'play'
$env:ANDROID_UPLOAD_KEYSTORE = "$HOME\keys\ferry\ferry-upload.p12"
$env:ANDROID_UPLOAD_KEY_ALIAS = 'upload'
$env:ANDROID_UPLOAD_KEYSTORE_PASSWORD = Read-Host -MaskInput 'Upload key password'
$env:ANDROID_UPLOAD_KEY_PASSWORD = $env:ANDROID_UPLOAD_KEYSTORE_PASSWORD
npm.cmd --prefix app run tauri android build -- --aab --target aarch64 armv7 x86_64
```

The variables last until you close the PowerShell window.

The bundle is `app/src-tauri/gen/android/app/build/outputs/bundle/universalRelease/app-universal-release.aab`.
For a Direct APK, use `ferryDist = 'direct'`, the `ANDROID_DIRECT_*` variables and
`--apk --target aarch64 armv7`.
