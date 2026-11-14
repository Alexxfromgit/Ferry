// Picks the Android build number (versionCode) for the next release: one more than the highest
// build either channel has released, so numbering starts at 1 and every release, rebuild or
// version bump alike, adds one. Both channels share the number.
//
//   node scripts/android-version-code.mjs
//
// Reads the live Direct feed and, with PLAY_SERVICE_ACCOUNT_JSON set, Google Play's tracks and
// uploads. Prints the number and, on GitHub Actions, writes `code=<n>` to GITHUB_OUTPUT.
import { appendFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { DOWNLOAD_HOST, FEED_PREFIX, PLAY_PACKAGE } from "./android-release.mjs";
import { playVersionCodes, serviceAccount } from "./play-api.mjs";

/**
 * One more than the highest build released on Google Play (any track, drafts included) or in
 * the Direct feed, skipping numbers Google Play has already taken for uploads that were never
 * released, since it never accepts a number twice.
 */
export function nextVersionCode({ released = [], uploaded = [] }) {
  const taken = new Set(uploaded);
  let code = Math.max(0, ...released) + 1;
  while (taken.has(code)) code++;
  return code;
}

async function liveFeedCode() {
  const url = `https://${DOWNLOAD_HOST}/${FEED_PREFIX}/latest.json`;
  const response = await fetch(url, { headers: { "cache-control": "no-cache" } });
  if (response.status === 404) return null;
  if (!response.ok) throw new Error(`${url} answered ${response.status}`);
  const code = Number((await response.json()).versionCode);
  if (!Number.isSafeInteger(code)) throw new Error(`${url} has no valid versionCode`);
  return code;
}

async function main() {
  const feed = await liveFeedCode();
  const account = serviceAccount();
  const play = account ? await playVersionCodes(account, PLAY_PACKAGE) : null;
  if (!play) {
    console.log("::warning::PLAY_SERVICE_ACCOUNT_JSON isn't set, so the build number only accounts for the Direct feed.");
  }
  const code = nextVersionCode({
    released: [...(feed === null ? [] : [feed]), ...(play?.onTracks ?? [])],
    uploaded: play?.uploaded ?? [],
  });
  const playSummary = play
    ? `highest on a track ${play.onTracks.length ? Math.max(...play.onTracks) : "none"}, uploaded ${play.uploaded.join(", ") || "none"}`
    : "not checked";
  console.log(`next build number: ${code} (Direct feed: ${feed ?? "none"}; Google Play: ${playSummary})`);
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, `code=${code}\n`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((err) => {
    console.error(`error: ${err.message}`);
    process.exit(1);
  });
}
