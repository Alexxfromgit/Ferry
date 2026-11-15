// A minimal Google Play Developer API client for the release scripts: signs in with the
// service account key in PLAY_SERVICE_ACCOUNT_JSON and works inside one edit, which it discards.
import { createSign } from "node:crypto";

const API = "https://androidpublisher.googleapis.com/androidpublisher/v3/applications";

export class PlayError extends Error {}

/** The service account key from PLAY_SERVICE_ACCOUNT_JSON, or null when it isn't set. */
export function serviceAccount(json = process.env.PLAY_SERVICE_ACCOUNT_JSON) {
  if (!json) return null;
  let account;
  try {
    account = JSON.parse(json);
  } catch {
    throw new PlayError("PLAY_SERVICE_ACCOUNT_JSON isn't the service account's JSON key");
  }
  if (account.type !== "service_account" || !account.client_email || !account.private_key) {
    throw new PlayError("PLAY_SERVICE_ACCOUNT_JSON isn't a service account key (no client_email or private_key)");
  }
  return account;
}

async function accessToken(account) {
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
  const response = await fetch(tokenUri, {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({ grant_type: "urn:ietf:params:oauth:grant-type:jwt-bearer", assertion }),
  });
  if (!response.ok) {
    throw new PlayError(`Google refused the service account key (${response.status}). Create a new JSON key for ${account.client_email}.`);
  }
  return (await response.json()).access_token;
}

/**
 * Opens an edit on the app, passes `get(path)` (paths relative to the edit) to `work`, and
 * discards the edit afterwards, so nothing on Google Play changes.
 */
export async function withEdit(account, packageName, work) {
  const token = await accessToken(account);
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
    throw new PlayError(`Google Play answered ${edit.status} ${detail}\n${hint}`.trim());
  }
  const { id } = await edit.json();
  try {
    return await work(async (path) => {
      const response = await call("GET", `/edits/${id}${path}`);
      if (!response.ok) throw new PlayError(`Google Play answered ${response.status} for ${path}`);
      return response.json();
    });
  } finally {
    await call("DELETE", `/edits/${id}`);
  }
}

/** Version codes Google Play knows: on any track (drafts included), and every bundle or APK ever uploaded. */
export async function playVersionCodes(account, packageName) {
  return withEdit(account, packageName, async (get) => {
    const [tracks, bundles, apks] = await Promise.all([get("/tracks"), get("/bundles"), get("/apks")]);
    const onTracks = (tracks.tracks ?? []).flatMap((t) => (t.releases ?? []).flatMap((r) => r.versionCodes ?? []));
    const uploaded = [...(bundles.bundles ?? []), ...(apks.apks ?? [])].map((b) => b.versionCode);
    return {
      onTracks: onTracks.map(Number),
      uploaded: uploaded.map(Number),
      internal: (tracks.tracks ?? []).find((t) => t.track === "internal")?.releases ?? [],
    };
  });
}
