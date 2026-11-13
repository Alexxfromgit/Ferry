import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { api, message } from "./api";
import { plural } from "./format";
import type { DownloadInfo, FerryEvent, NodeStatus, Preview, ShareInfo } from "./types";

export type View = "send" | "receive" | "settings";

interface Importing {
  doneBytes: number;
  totalBytes: number;
  currentFile: string;
}

export const app = $state({
  view: "send" as View,
  status: null as NodeStatus | null,
  shares: [] as ShareInfo[],
  activity: {} as Record<string, { activePeers: number; uploadedBytes: number }>,
  downloads: [] as DownloadInfo[],
  importing: null as Importing | null,
  dragging: false,
  toast: null as { text: string; error: boolean } | null,
  /** Bumped when share links may have changed (e.g. the relay connected); QR codes refetch. */
  linkEpoch: 0,
});

/** The Receive screen's state lives here so a preview survives switching tabs. */
export const receive = $state({
  link: "",
  checking: false,
  preview: null as Preview | null,
  /** The link text the preview was made from. */
  previewLink: "",
  problem: null as { title: string; detail: string } | null,
  dest: null as string | null,
  starting: false,
});

let toastTimer: ReturnType<typeof setTimeout> | undefined;

export function toast(text: string, error = false) {
  app.toast = { text, error };
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (app.toast = null), error ? 6000 : 2800);
}

function upsertDownload(download: DownloadInfo) {
  const i = app.downloads.findIndex((d) => d.id === download.id);
  if (i >= 0) app.downloads[i] = download;
  else app.downloads.unshift(download);
}

function onEvent(event: FerryEvent) {
  switch (event.type) {
    case "importProgress":
      if (app.importing) {
        app.importing = {
          doneBytes: event.doneBytes,
          totalBytes: event.totalBytes,
          currentFile: event.currentFile,
        };
      }
      break;
    case "shareCreated":
      app.shares = [event.share, ...app.shares.filter((s) => s.id !== event.share.id)];
      break;
    case "shareRemoved":
      app.shares = app.shares.filter((s) => s.id !== event.shareId);
      delete app.activity[event.shareId];
      break;
    case "shareActivity":
      if (app.shares.some((s) => s.id === event.shareId)) {
        app.activity[event.shareId] = {
          activePeers: event.activePeers,
          uploadedBytes: event.uploadedBytes,
        };
      }
      break;
    case "downloadUpdated":
      upsertDownload(event.download);
      break;
    case "downloadRemoved":
      app.downloads = app.downloads.filter((d) => d.id !== event.downloadId);
      break;
  }
}

/** Reload everything from the backend (after missed events, or once the relay connects). */
async function resync() {
  const [shares, downloads] = await Promise.all([api.listShares(), api.listDownloads()]);
  app.shares = shares.sort((a, b) => b.createdAt - a.createdAt);
  app.downloads = downloads.sort((a, b) => b.createdAt - a.createdAt);
  app.linkEpoch++;
}

async function takeIncoming() {
  const links = await api.takePendingLinks();
  if (links.length > 0) {
    const link = links[links.length - 1];
    app.view = "receive";
    // A .ferry file's JSON is checked but not shown in the link field.
    receive.link = link.trim().startsWith("{") ? "" : link.trim();
    await checkLink(link);
  }
}

export async function share(paths: string[]) {
  if (paths.length === 0) return;
  if (app.importing) {
    toast("Ferry is still preparing the previous ticket. Try again when it's done.", true);
    return;
  }
  app.importing = { doneBytes: 0, totalBytes: 0, currentFile: "" };
  try {
    const created = await api.createShare(paths);
    const left = created.skippedCount ?? 0;
    toast(
      left > 0
        ? `Sharing ${created.name}. ${plural(left, "item was", "items were")} left out; see the ticket.`
        : `Sharing ${created.name}`,
    );
  } catch (err) {
    toast(`Couldn't share: ${message(err)}`, true);
  } finally {
    app.importing = null;
  }
}

let checkToken = 0;

/** Contact the sender and show what the link contains. Only the latest check wins. */
export async function checkLink(text: string) {
  if (!text.trim()) return;
  const token = ++checkToken;
  receive.checking = true;
  receive.problem = null;
  receive.preview = null;
  try {
    const preview = await api.preview(text);
    if (token !== checkToken) return;
    receive.preview = preview;
    receive.previewLink = text;
  } catch (err) {
    if (token !== checkToken) return;
    const detail = message(err);
    const unreachable = /reach|refused|no longer has|transfer/i.test(detail);
    receive.problem = unreachable
      ? {
          title: "Couldn't reach this share.",
          detail: `${detail}. The sender's Ferry has to be running. If it is, ask them whether they stopped sharing it.`,
        }
      : { title: "This isn't a Ferry link Ferry can open.", detail };
  } finally {
    if (token === checkToken) receive.checking = false;
  }
}

export function clearPreview() {
  checkToken++;
  receive.preview = null;
  receive.previewLink = "";
  receive.checking = false;
  receive.dest = null;
}

export async function startDownload() {
  if (!receive.preview || receive.starting) return;
  receive.starting = true;
  try {
    await api.startDownload(receive.previewLink, receive.dest ?? undefined);
    toast(`Downloading ${receive.preview.name}`);
    receive.link = "";
    clearPreview();
  } catch (err) {
    toast(`Couldn't start the download: ${message(err)}`, true);
  } finally {
    receive.starting = false;
  }
}

async function refreshStatus() {
  try {
    app.status = await api.nodeStatus();
  } catch {
    app.status = null;
  }
}

export async function init() {
  await listen<FerryEvent>("ferry-event", (e) => onEvent(e.payload));
  await listen("ferry-open-link", () => void takeIncoming());
  await listen("ferry-resync", () => void resync());

  // Dropping files anywhere shares them; dropping .ferry files opens them.
  await getCurrentWebview().onDragDropEvent(async (e) => {
    const p = e.payload;
    if (p.type === "enter" || p.type === "over") app.dragging = true;
    else if (p.type === "leave") app.dragging = false;
    else if (p.type === "drop") {
      app.dragging = false;
      const tickets = p.paths.filter((x) => x.toLowerCase().endsWith(".ferry"));
      if (tickets.length > 0 && tickets.length === p.paths.length) {
        try {
          const text = await api.readShareFile(tickets[0]);
          app.view = "receive";
          receive.link = "";
          await checkLink(text);
        } catch (err) {
          toast(`Couldn't open the ticket file: ${message(err)}`, true);
        }
      } else {
        app.view = "send";
        await share(p.paths);
      }
    }
  });

  await resync();
  await refreshStatus();
  setInterval(refreshStatus, 4000);
  await takeIncoming();
}
