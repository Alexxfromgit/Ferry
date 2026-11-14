// Checks that the Google Play service account can publish Ferry, without changing anything:
// it opens an edit on the app, reads the internal track and discards the edit.
//
//   PLAY_SERVICE_ACCOUNT_JSON='{…}' node scripts/play-check.mjs com.xenvaultinteractive.ferry
import { createSign } from "node:crypto";

const API = "https://androidpublisher.googleapis.com/androidpublisher/v3/applications";

function fail(message) {
  console.error(`error: ${message}`);
  process.exit(1);
}

const packageName = process.argv[2] ?? fail("Pass the package name");
let account;
try {
  account = JSON.parse(process.env.PLAY_SERVICE_ACCOUNT_JSON ?? "");
} catch {
  fail("PLAY_SERVICE_ACCOUNT_JSON isn't the service account's JSON key");
}
if (account.type !== "service_account" || !account.client_email || !account.private_key) {
  fail("PLAY_SERVICE_ACCOUNT_JSON isn't a service account key (no client_email or private_key)");
}

const tokenUri = account.token_uri ?? "https://oauth2.googleapis.com/token";
const now = Math.floor(Date.now() / 1000);
const part = (value) => Buffer.from(JSON.stringify(value)).toString("base64url");
const unsigned = `${part({ alg: "RS256", typ: "JWT" })}.${part({
  iss: account.client_email,
  scope: "https://www.googleapis.com/auth/androidpublisher",
  aud: tokenUri,
  iat: now,
  exp: now + 600,
})}`;
const assertion = `${unsigned}.${createSign("RSA-SHA256").update(unsigned).sign(account.private_key, "base64url")}`;

const tokenResponse = await fetch(tokenUri, {
  method: "POST",
  headers: { "content-type": "application/x-www-form-urlencoded" },
  body: new URLSearchParams({ grant_type: "urn:ietf:params:oauth:grant-type:jwt-bearer", assertion }),
});
if (!tokenResponse.ok) {
  fail(`Google refused the service account key (${tokenResponse.status}). Create a new JSON key for ${account.client_email}.`);
}
const { access_token: token } = await tokenResponse.json();
const call = (method, path) =>
  fetch(`${API}/${packageName}${path}`, { method, headers: { authorization: `Bearer ${token}` } });

const edit = await call("POST", "/edits");
if (!edit.ok) {
  const detail = (await edit.json().catch(() => ({})))?.error?.message ?? "";
  const hint =
    edit.status === 404
      ? `Play Console has no app ${packageName} yet, or the service account can't see it.`
      : edit.status === 403
        ? "Enable the Google Play Android Developer API in the account's Cloud project, and give the service account access to the app in Play Console (it can take a day to apply)."
        : "";
  fail(`Google Play answered ${edit.status} ${detail}\n${hint}`.trim());
}
const { id } = await edit.json();
const track = await call("GET", `/edits/${id}/tracks/internal`);
await call("DELETE", `/edits/${id}`);
if (!track.ok) fail(`Couldn't read the internal testing track (${track.status})`);
const releases = (await track.json()).releases ?? [];
console.log(
  `ok: ${account.client_email} can publish ${packageName}; internal track has ${releases.length} release(s)` +
    (releases.length ? ` (latest: ${releases[0].name ?? releases[0].versionCodes?.join(", ")}, ${releases[0].status})` : ""),
);
