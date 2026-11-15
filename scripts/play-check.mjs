// Checks that the Google Play service account can publish Ferry, without changing anything:
// it opens an edit on the app, reads its tracks and uploads, and discards the edit.
//
//   PLAY_SERVICE_ACCOUNT_JSON='{…}' node scripts/play-check.mjs com.xenvaultinteractive.ferry
import { PlayError, playVersionCodes, serviceAccount } from "./play-api.mjs";

try {
  const packageName = process.argv[2];
  if (!packageName) throw new PlayError("Pass the package name");
  const account = serviceAccount();
  if (!account) throw new PlayError("PLAY_SERVICE_ACCOUNT_JSON isn't set");
  const { internal, uploaded } = await playVersionCodes(account, packageName);
  const latest = internal[0];
  console.log(
    `ok: ${account.client_email} can publish ${packageName}; ${uploaded.length} build(s) uploaded, ` +
      `internal track: ${latest ? `${latest.name ?? latest.versionCodes?.join(", ")} (${latest.status})` : "empty"}`,
  );
} catch (err) {
  console.error(`error: ${err.message}`);
  process.exit(1);
}
