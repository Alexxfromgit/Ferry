// What a share's ticket and its compact strip both do, so the two views behave the same.
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { ask } from "@tauri-apps/plugin-dialog";
import { api, message } from "./api";
import { bytes, plural } from "./format";
import { app, share as createShare, toast } from "./store.svelte";
import type { ShareActivity, ShareInfo } from "./types";

/** Copy a link built now, so it carries the current addresses (not those from start-up). */
export async function copyLink(share: ShareInfo) {
  try {
    const fresh = await api.getShare(share.id);
    await writeText(fresh.link);
    toast("Link copied. Paste it into any chat or email.");
  } catch (err) {
    toast(`Couldn't copy the link: ${message(err)}`, true);
  }
}

const busy = new Set<string>();

/** Turn a share off or on. Turning it off asks first only if someone is downloading. */
export async function setSharing(share: ShareInfo, on: boolean) {
  if (busy.has(share.id)) return;
  const receiving = app.activity[share.id]?.receivers.length ?? 0;
  if (!on && receiving > 0) {
    const ok = await ask(
      `${plural(receiving, "download", "downloads")} in progress will stop. They can carry on where they left off once you turn sharing back on.`,
      { title: `Turn off “${share.name}”?`, kind: "warning", okLabel: "Turn off", cancelLabel: "Keep sharing" },
    );
    if (!ok) return;
  }
  busy.add(share.id);
  try {
    await api.setSharePaused(share.id, !on);
    delete app.shareProblems[share.id];
    toast(on ? `Sharing ${share.name} again` : `Stopped sharing ${share.name} for now`);
  } catch (err) {
    // The change can be made but not saved (the error then says what that means after a
    // restart), so go by the share as it is now, not by the error alone.
    const now = await api.getShare(share.id).catch(() => undefined);
    if (!now) {
      // Removed meanwhile; nothing left to turn on.
    } else if (now.paused === !on) {
      const i = app.shares.findIndex((s) => s.id === now.id);
      if (i >= 0) app.shares[i] = now;
      if (on) delete app.shareProblems[share.id];
      toast(message(err).replace(/^./, (c) => c.toUpperCase()), true);
    } else if (on) {
      app.shareProblems[share.id] = message(err);
      toast(`Couldn't turn ${share.name} back on. See its ticket for why.`, true);
    } else {
      toast(`Couldn't turn sharing off: ${message(err)}`, true);
    }
  } finally {
    busy.delete(share.id);
  }
}

/** Remove a share for good, after asking. */
export async function removeShare(share: ShareInfo) {
  const ok = await ask(
    `The ticket for “${share.name}” stops working for good, and downloads in progress stop. Your files stay where they are.`,
    { title: "Remove this ticket?", kind: "warning", okLabel: "Remove", cancelLabel: "Keep" },
  );
  if (!ok) return;
  try {
    await api.removeShare(share.id);
    toast(`Removed the ticket for ${share.name}`);
  } catch (err) {
    toast(`Couldn't remove the ticket: ${message(err)}`, true);
  }
}

/** Share the same places again after the files changed: a new ticket with a new link. */
export async function shareAgain(share: ShareInfo) {
  const created = await createShare(share.sourcePaths);
  if (!created || created.id === share.id) return;
  const ok = await ask(
    `The new ticket has a new link. The old one for “${share.name}” can't be turned on again, since its files changed.`,
    { title: "Remove the old ticket?", kind: "info", okLabel: "Remove old ticket", cancelLabel: "Keep both" },
  );
  if (!ok) return;
  try {
    await api.removeShare(share.id);
  } catch (err) {
    toast(`Couldn't remove the old ticket: ${message(err)}`, true);
  }
}

/** Overall progress of everyone receiving, 0–100, or null when nobody is. */
export function overallPercent(activity?: ShareActivity): number | null {
  const receivers = activity?.receivers ?? [];
  const total = receivers.reduce((n, r) => n + r.totalBytes, 0);
  if (total === 0) return null;
  const done = receivers.reduce((n, r) => n + Math.min(r.doneBytes, r.totalBytes), 0);
  return Math.min(100, (done / total) * 100);
}

/** One short line on what a share is doing. */
export function statusLine(share: ShareInfo, activity?: ShareActivity): string {
  if (share.paused) return "Off. The link won't work until you turn sharing on";
  const receiving = activity?.receivers.length ?? 0;
  if (receiving > 0 && activity) {
    return `↑ ${bytes(activity.bytesPerSec)}/s · ${plural(receiving, "person", "people")} receiving · ${bytes(activity.uploadedBytes)} sent`;
  }
  if (activity && activity.uploadedBytes > 0) return `${bytes(activity.uploadedBytes)} sent since Ferry started`;
  return "Waiting for someone to open the ticket";
}
