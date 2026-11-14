// Checks Ferry's Android builds and prepares the Direct update feed.
//
//   node scripts/android-release.mjs check-channels <play.apk> <direct.apk>
//       CI: each channel has its own package, only Direct may install APKs, and only Play
//       carries Google Play's in-app update library.
//   node scripts/android-release.mjs verify <direct.apk> --tag vX.Y.Z --cert <sha256>
//       Release: the APK is the Direct build of that tag, not debuggable, signed with that key.
//   node scripts/android-release.mjs prepare <direct.apk> --tag vX.Y.Z --cert <sha256>
//       --repo owner/name --out <dir> [--current <latest.json>]
//       Release: verify, then lay out <dir>/ferry/android/{releases/<code>/<apk>,latest.json} as
//       the bucket stores them. With --current (the feed now live), refuses to go backwards and
//       reports publish=false when this version is already live.
//
// Needs aapt2 and apksigner from the Android SDK's build-tools (ANDROID_HOME).
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  appendFileSync,
  closeSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  openSync,
  readdirSync,
  readFileSync,
  readSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { basename, dirname, join } from "node:path";
import { pathToFileURL } from "node:url";
import { inflateRawSync } from "node:zlib";

export const PLAY_PACKAGE = "com.xenvaultinteractive.ferry";
export const DIRECT_PACKAGE = `${PLAY_PACKAGE}.direct`;
// Must match FEED_HOST and FEED_URL in gen/android/app/src/direct/.../UpdatePlugin.kt.
export const DOWNLOAD_HOST = "downloads.xenvaultinteractive.com";
export const FEED_PREFIX = "ferry/android";
const INSTALL_PERMISSION = "android.permission.REQUEST_INSTALL_PACKAGES";
// Google Play's in-app update library, as dex type descriptors name it.
const PLAY_UPDATES_CLASS = Buffer.from("Lcom/google/android/play/core/appupdate/");

/** Tauri's Android versionCode for a semver version: major * 1000000 + minor * 1000 + patch. */
export function versionCodeOf(version) {
  const m = /^(\d+)\.(\d+)\.(\d+)$/.exec(version);
  if (!m) throw new Error(`"${version}" is not a plain X.Y.Z version`);
  const [major, minor, patch] = m.slice(1).map(Number);
  if (minor > 999 || patch > 999) throw new Error(`${version}: minor and patch must stay below 1000`);
  return major * 1_000_000 + minor * 1000 + patch;
}

export function versionOfTag(tag) {
  const m = /^v(\d+\.\d+\.\d+)$/.exec(tag ?? "");
  if (!m) throw new Error(`Use a vX.Y.Z tag, not "${tag}"`);
  return m[1];
}

/** A SHA-256 fingerprint as lowercase hex, from keytool's AB:CD:… or apksigner's abcd… form. */
export function normalizeDigest(value) {
  const hex = String(value ?? "").replace(/[\s:]/g, "").toLowerCase();
  if (!/^[0-9a-f]{64}$/.test(hex)) throw new Error("A certificate SHA-256 must be 64 hex digits");
  return hex;
}

/** The parts of `aapt2 dump badging` Ferry checks. */
export function parseBadging(text) {
  const pkg = /^package: name='([^']+)' versionCode='(\d+)' versionName='([^']*)'/m.exec(text);
  if (!pkg) throw new Error("aapt2 printed no package line");
  return {
    package: pkg[1],
    versionCode: Number(pkg[2]),
    versionName: pkg[3],
    debuggable: /^application-debuggable$/m.test(text),
    permissions: new Set([...text.matchAll(/^uses-permission: name='([^']+)'/gm)].map((m) => m[1])),
  };
}

/** Signer certificate digests from `apksigner verify --print-certs`. */
export function parseSigners(text) {
  return [...new Set([...text.matchAll(/certificate SHA-256 digest: ([0-9a-fA-F]{64})/g)].map((m) => m[1].toLowerCase()))];
}

/**
 * What to do given the feed now live and the build being released: "publish", or "published"
 * when this version is already live (a re-run). Never moves the feed back to an older build.
 */
export function feedAction(current, versionCode) {
  if (!current) return "publish";
  const live = Number(current.versionCode);
  if (!Number.isSafeInteger(live)) throw new Error("The live latest.json has no valid versionCode");
  if (live > versionCode) throw new Error(`The live feed already offers versionCode ${live}, newer than ${versionCode}`);
  return live === versionCode ? "published" : "publish";
}

export function feedFor({ version, versionCode, sha256, sizeBytes, repo, tag, publishedAt }) {
  const apkKey = `${FEED_PREFIX}/releases/${versionCode}/Ferry-${version}-android.apk`;
  return {
    apkKey,
    feed: {
      versionCode,
      versionName: version,
      apkUrl: `https://${DOWNLOAD_HOST}/${apkKey}`,
      sha256,
      sizeBytes,
      releaseNotes: `https://github.com/${repo}/releases/tag/${tag}`,
      publishedAt,
    },
  };
}

// --------------------------------------------------------------------------------- tools

function buildTool(name) {
  const sdk = process.env.ANDROID_HOME || process.env.ANDROID_SDK_ROOT;
  if (!sdk) throw new Error("Set ANDROID_HOME to the Android SDK");
  const root = join(sdk, "build-tools");
  const file = process.platform === "win32" ? (name === "apksigner" ? `${name}.bat` : `${name}.exe`) : name;
  const found = (existsSync(root) ? readdirSync(root) : [])
    .sort((a, b) => b.localeCompare(a, undefined, { numeric: true }))
    .map((version) => join(root, version, file))
    .find(existsSync);
  if (!found) throw new Error(`No ${name} under ${root}`);
  return found;
}

function run(tool, args) {
  // .bat files only run through cmd.exe.
  const [command, commandArgs] = tool.endsWith(".bat") ? ["cmd.exe", ["/d", "/c", tool, ...args]] : [tool, args];
  return execFileSync(command, commandArgs, { encoding: "utf8", maxBuffer: 1 << 26, stdio: ["ignore", "pipe", "pipe"] });
}

const badging = (apk) => parseBadging(run(buildTool("aapt2"), ["dump", "badging", apk]));
const signers = (apk) => parseSigners(run(buildTool("apksigner"), ["verify", "--print-certs", apk]));

/** Whether any dex file in the APK or bundle mentions `needle`. Reads entries one by one. */
export function dexContains(archive, needle) {
  const fd = openSync(archive, "r");
  try {
    const read = (position, length) => {
      const buf = Buffer.alloc(length);
      const n = readSync(fd, buf, 0, length, position);
      return buf.subarray(0, n);
    };
    const size = statSync(archive).size;
    const tail = read(Math.max(0, size - 65_557), Math.min(size, 65_557));
    const eocd = tail.lastIndexOf(Buffer.from([0x50, 0x4b, 0x05, 0x06]));
    if (eocd < 0) throw new Error(`${basename(archive)} is not a zip archive`);
    const entries = tail.readUInt16LE(eocd + 10);
    const cdSize = tail.readUInt32LE(eocd + 12);
    const cdOffset = tail.readUInt32LE(eocd + 16);
    if (cdOffset === 0xffffffff || entries === 0xffff) throw new Error("ZIP64 archives aren't supported");
    const cd = read(cdOffset, cdSize);
    for (let p = 0, i = 0; i < entries; i++) {
      if (cd.readUInt32LE(p) !== 0x02014b50) throw new Error("Corrupt zip central directory");
      const method = cd.readUInt16LE(p + 10);
      const compressed = cd.readUInt32LE(p + 20);
      const nameLength = cd.readUInt16LE(p + 28);
      const extraLength = cd.readUInt16LE(p + 30);
      const commentLength = cd.readUInt16LE(p + 32);
      const local = cd.readUInt32LE(p + 42);
      const name = cd.toString("utf8", p + 46, p + 46 + nameLength);
      p += 46 + nameLength + extraLength + commentLength;
      if (!/^(base\/dex\/)?classes\d*\.dex$/.test(name)) continue;
      const header = read(local, 30);
      const start = local + 30 + header.readUInt16LE(26) + header.readUInt16LE(28);
      const raw = read(start, compressed);
      const dex = method === 0 ? raw : method === 8 ? inflateRawSync(raw) : null;
      if (!dex) throw new Error(`${name} uses unsupported zip method ${method}`);
      if (dex.includes(needle)) return true;
    }
    return false;
  } finally {
    closeSync(fd);
  }
}

// ------------------------------------------------------------------------------ commands

function fail(message) {
  throw new Error(message);
}

export function checkChannels(playApk, directApk) {
  const play = badging(playApk);
  const direct = badging(directApk);
  if (play.package !== PLAY_PACKAGE) fail(`The Play build is ${play.package}, expected ${PLAY_PACKAGE}`);
  if (direct.package !== DIRECT_PACKAGE) fail(`The Direct build is ${direct.package}, expected ${DIRECT_PACKAGE}`);
  if (play.permissions.has(INSTALL_PERMISSION)) fail(`The Play build must not ask for ${INSTALL_PERMISSION}`);
  if (!direct.permissions.has(INSTALL_PERMISSION)) fail(`The Direct build needs ${INSTALL_PERMISSION} to install updates`);
  if (!dexContains(playApk, PLAY_UPDATES_CLASS)) fail("The Play build is missing Google Play's in-app updates");
  if (dexContains(directApk, PLAY_UPDATES_CLASS)) fail("The Direct build must not contain Google Play's in-app updates");
  console.log(`ok: ${play.package} and ${direct.package} (versionCode ${play.versionCode})`);
}

/** Checks a release Direct APK; returns its version and digest. */
export function verifyDirect(apk, tag, cert) {
  const version = versionOfTag(tag);
  const versionCode = versionCodeOf(version);
  const expectedSigner = normalizeDigest(cert);
  const info = badging(apk);
  if (info.package !== DIRECT_PACKAGE) fail(`${basename(apk)} is ${info.package}, expected ${DIRECT_PACKAGE}`);
  if (info.versionName !== version) fail(`${basename(apk)} is version ${info.versionName}, expected ${version}`);
  if (info.versionCode !== versionCode) fail(`${basename(apk)} has versionCode ${info.versionCode}, expected ${versionCode}`);
  if (info.debuggable) fail("Refusing to publish a debuggable APK");
  if (!info.permissions.has(INSTALL_PERMISSION)) fail(`The Direct build needs ${INSTALL_PERMISSION}`);
  if (dexContains(apk, PLAY_UPDATES_CLASS)) fail("The Direct build must not contain Google Play's in-app updates");
  const found = signers(apk);
  if (found.length !== 1 || found[0] !== expectedSigner) {
    fail(`${basename(apk)} is signed by ${found.join(", ") || "nobody"}, expected ${expectedSigner}`);
  }
  const bytes = readFileSync(apk);
  return { version, versionCode, sha256: createHash("sha256").update(bytes).digest("hex"), sizeBytes: bytes.length };
}

function prepare(apk, { tag, cert, repo, out, current }) {
  if (!/^[\w.-]+\/[\w.-]+$/.test(repo ?? "")) fail("--repo must be owner/name");
  if (!out) fail("--out is required");
  const checked = verifyDirect(apk, tag, cert);
  const live = current && existsSync(current) ? JSON.parse(readFileSync(current, "utf8")) : null;
  const action = feedAction(live, checked.versionCode);
  const { apkKey, feed } = feedFor({ ...checked, repo, tag, publishedAt: new Date().toISOString() });
  const apkPath = join(out, apkKey);
  const feedPath = join(out, FEED_PREFIX, "latest.json");
  mkdirSync(dirname(apkPath), { recursive: true });
  copyFileSync(apk, apkPath);
  writeFileSync(feedPath, `${JSON.stringify(feed, null, 2)}\n`);
  const outputs = { publish: action === "publish", apk_key: apkKey, apk_path: apkPath, feed_path: feedPath, sha256: checked.sha256 };
  if (process.env.GITHUB_OUTPUT) {
    appendFileSync(process.env.GITHUB_OUTPUT, Object.entries(outputs).map(([k, v]) => `${k}=${v}\n`).join(""));
  }
  console.log(
    action === "publish"
      ? `ready: ${apkKey} (${checked.sizeBytes} bytes, sha256 ${checked.sha256})`
      : `versionCode ${checked.versionCode} is already live; nothing to publish`,
  );
}

function options(args) {
  const positional = [];
  const named = {};
  for (let i = 0; i < args.length; i++) {
    if (args[i].startsWith("--")) {
      const value = args[i + 1];
      if (value === undefined || value.startsWith("--")) fail(`${args[i]} needs a value`);
      named[args[i].slice(2)] = value;
      i++;
    } else positional.push(args[i]);
  }
  return { positional, named };
}

function main([command, ...rest]) {
  const { positional, named } = options(rest);
  if (command === "check-channels" && positional.length === 2) return checkChannels(...positional);
  if (command === "verify" && positional.length === 1) {
    const { versionCode, sha256 } = verifyDirect(positional[0], named.tag, named.cert);
    return console.log(`ok: ${DIRECT_PACKAGE} versionCode ${versionCode}, sha256 ${sha256}`);
  }
  if (command === "prepare" && positional.length === 1) return prepare(positional[0], named);
  fail("Usage: android-release.mjs check-channels <play.apk> <direct.apk> | verify <apk> --tag --cert | prepare <apk> --tag --cert --repo --out [--current]");
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    main(process.argv.slice(2));
  } catch (err) {
    console.error(`error: ${err.message}`);
    process.exit(1);
  }
}
