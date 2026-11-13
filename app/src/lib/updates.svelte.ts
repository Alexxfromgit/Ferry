import { api, message } from "./api";
import { activeDownloadCount, app } from "./store.svelte";
import type { UpdateInfo } from "./types";

export const updates = $state({
  info: null as UpdateInfo | null,
  checking: false,
  installing: false,
  saving: false,
  status: "",
});

async function refresh() {
  updates.info = await api.updateInfo();
}

export function initUpdates() {
  refresh().catch(() => {
    updates.status = "Ferry couldn't load update settings.";
  });
  const check = () => void checkForUpdates(true);
  const startup = setTimeout(check, 10_000);
  const daily = setInterval(check, 24 * 60 * 60 * 1000);
  return () => {
    clearTimeout(startup);
    clearInterval(daily);
  };
}

export async function setAutoCheck(enabled: boolean) {
  const info = updates.info;
  if (info?.support !== "available" || updates.saving) return;
  updates.saving = true;
  try {
    await api.setAutoCheckUpdates(enabled);
    info.autoCheck = enabled;
    if (enabled) void checkForUpdates(true);
  } catch (err) {
    updates.status = message(err);
  } finally {
    updates.saving = false;
  }
}

export async function checkForUpdates(automatic = false) {
  const info = updates.info;
  if (info?.support !== "available" || updates.checking || updates.installing || info.readyVersion) return;
  if (automatic && !info.autoCheck) return;
  updates.checking = true;
  if (!automatic) updates.status = "";
  try {
    info.readyVersion = await api.checkForUpdates(automatic);
    if (info.readyVersion) updates.status = "";
    else if (!automatic) updates.status = "Ferry is up to date.";
  } catch (err) {
    if (!automatic) updates.status = message(err);
  } finally {
    updates.checking = false;
  }
}

export function transferring() {
  return (
    app.importing !== null ||
    activeDownloadCount() > 0 ||
    Object.values(app.activity).some((share) => share.activePeers > 0)
  );
}

export async function restartToUpdate() {
  if (updates.installing || updates.checking || transferring()) return;
  updates.installing = true;
  updates.status = "";
  try {
    await api.installUpdate();
  } catch (err) {
    updates.status = message(err);
    try {
      await refresh();
    } catch {
      // Keep the last known version and show the installation error.
    } finally {
      updates.installing = false;
    }
  }
}
