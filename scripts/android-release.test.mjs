// node --test "scripts/*.test.mjs"
import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { deflateRawSync } from "node:zlib";
import {
  dexContains,
  feedAction,
  feedFor,
  normalizeDigest,
  parseBadging,
  parseSigners,
  parseVersionCode,
  versionOfTag,
} from "./android-release.mjs";
import { nextVersionCode } from "./android-version-code.mjs";

test("build numbers and tags are validated", () => {
  assert.equal(parseVersionCode("1"), 1);
  assert.equal(parseVersionCode("42"), 42);
  assert.throws(() => parseVersionCode("0"));
  assert.throws(() => parseVersionCode("1.5"));
  assert.throws(() => parseVersionCode(undefined));
  assert.equal(versionOfTag("v0.3.0"), "0.3.0");
  assert.throws(() => versionOfTag("0.3.0"));
});

test("build numbers count up from 1 across both channels", () => {
  assert.equal(nextVersionCode({}), 1);
  assert.equal(nextVersionCode({ released: [3] }), 4);
  // The higher of the Direct feed and Google Play wins.
  assert.equal(nextVersionCode({ released: [3, 5, 4] }), 6);
  // A number Google Play took for an upload that was never released is skipped...
  assert.equal(nextVersionCode({ released: [3], uploaded: [3, 4] }), 5);
  // ...but an old upload far ahead doesn't hold numbering back.
  assert.equal(nextVersionCode({ released: [], uploaded: [2000] }), 1);
  assert.equal(nextVersionCode({ released: [1999], uploaded: [2000] }), 2001);
});

test("certificate digests accept keytool and apksigner spellings", () => {
  const hex = "ab".repeat(32);
  assert.equal(normalizeDigest("AB:".repeat(31) + "AB"), hex);
  assert.equal(normalizeDigest(` ${hex.toUpperCase()}\n`), hex);
  assert.throws(() => normalizeDigest("abcd"));
  assert.throws(() => normalizeDigest(""));
});

test("badging and signer output parse", () => {
  const info = parseBadging(
    [
      "package: name='com.xenvaultinteractive.ferry.direct' versionCode='3000' versionName='0.3.0' platformBuildVersionName='17'",
      "sdkVersion:'29'",
      "uses-permission: name='android.permission.INTERNET'",
      "uses-permission: name='android.permission.REQUEST_INSTALL_PACKAGES'",
      "application-debuggable",
    ].join("\n"),
  );
  assert.equal(info.package, "com.xenvaultinteractive.ferry.direct");
  assert.equal(info.versionCode, 3000);
  assert.equal(info.versionName, "0.3.0");
  assert.equal(info.debuggable, true);
  assert.ok(info.permissions.has("android.permission.REQUEST_INSTALL_PACKAGES"));
  assert.equal(parseBadging("package: name='a' versionCode='1' versionName='1'\n").debuggable, false);
  assert.throws(() => parseBadging("nothing here"));

  const digest = "0f".repeat(32);
  const signers = parseSigners(
    `Signer #1 certificate DN: CN=Ferry\nSigner #1 certificate SHA-256 digest: ${digest.toUpperCase()}\nSigner #1 certificate SHA-1 digest: ${"1".repeat(40)}\n`,
  );
  assert.deepEqual(signers, [digest]);
});

test("the feed never goes backwards", () => {
  assert.equal(feedAction(null, 3000), "publish");
  assert.equal(feedAction({ versionCode: 2000 }, 3000), "publish");
  assert.equal(feedAction({ versionCode: 3000 }, 3000), "published");
  assert.throws(() => feedAction({ versionCode: 4000 }, 3000), /newer/);
  assert.throws(() => feedAction({}, 3000));
});

test("the feed matches what the Direct updater expects", () => {
  const { apkKey, feed } = feedFor({
    version: "0.3.0",
    versionCode: 3000,
    sha256: "aa".repeat(32),
    sizeBytes: 42,
    repo: "Alexxfromgit/Ferry",
    tag: "v0.3.0",
    publishedAt: "2026-11-14T00:00:00.000Z",
  });
  assert.equal(apkKey, "ferry/android/releases/3000/Ferry-0.3.0-android.apk");
  assert.equal(feed.apkUrl, "https://downloads.xenvaultinteractive.com/ferry/android/releases/3000/Ferry-0.3.0-android.apk");
  assert.deepEqual(Object.keys(feed), ["versionCode", "versionName", "apkUrl", "sha256", "sizeBytes", "releaseNotes", "publishedAt"]);
});

/** A minimal zip with the given entries, stored or deflated. */
function zip(entries) {
  const locals = [];
  const central = [];
  let offset = 0;
  for (const [name, content, deflate] of entries) {
    const data = deflate ? deflateRawSync(content) : content;
    const nameBytes = Buffer.from(name);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(deflate ? 8 : 0, 8);
    local.writeUInt32LE(data.length, 18);
    local.writeUInt32LE(content.length, 22);
    local.writeUInt16LE(nameBytes.length, 26);
    const cd = Buffer.alloc(46);
    cd.writeUInt32LE(0x02014b50, 0);
    cd.writeUInt16LE(deflate ? 8 : 0, 10);
    cd.writeUInt32LE(data.length, 20);
    cd.writeUInt32LE(content.length, 24);
    cd.writeUInt16LE(nameBytes.length, 28);
    cd.writeUInt32LE(offset, 42);
    locals.push(local, nameBytes, data);
    central.push(cd, nameBytes);
    offset += local.length + nameBytes.length + data.length;
  }
  const cdBytes = Buffer.concat(central);
  const eocd = Buffer.alloc(22);
  eocd.writeUInt32LE(0x06054b50, 0);
  eocd.writeUInt16LE(entries.length, 8);
  eocd.writeUInt16LE(entries.length, 10);
  eocd.writeUInt32LE(cdBytes.length, 12);
  eocd.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, cdBytes, eocd]);
}

test("dex search looks only inside dex files", () => {
  const dir = mkdtempSync(join(tmpdir(), "ferry-apk-"));
  const needle = Buffer.from("Lcom/google/android/play/core/appupdate/");
  const withPlay = join(dir, "play.apk");
  const without = join(dir, "direct.apk");
  writeFileSync(
    withPlay,
    zip([
      ["AndroidManifest.xml", Buffer.from("manifest"), false],
      ["classes2.dex", Buffer.concat([Buffer.from("dex\n035"), needle, Buffer.from("AppUpdateManager;")]), true],
    ]),
  );
  writeFileSync(
    without,
    zip([
      ["classes.dex", Buffer.from("dex\n035 Lcom/xenvaultinteractive/ferry/UpdatePlugin;"), true],
      ["assets/notes.txt", needle, false],
    ]),
  );
  assert.equal(dexContains(withPlay, needle), true);
  assert.equal(dexContains(without, needle), false);
});
