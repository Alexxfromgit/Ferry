import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { api, message } from "./api";
import { plural } from "./format";
import type { DownloadInfo, FerryEvent, NodeStatus, Preview, ShareInfo } from "./types";

export type View = "send" | "receive" | "settings";
export type DragKind = "files" | "tickets";

/** Dropping only .ferry files opens the first one as a ticket; anything else is shared. */
export function dragKind(paths: string[]): DragKind {
  return paths.length > 0 && paths.every((x) => x.toLowerCase().endsWith(".ferry")) ? "tickets" : "files";
}

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
  /** What a drop would do: share the files, or open the ticket files being dragged. */
  dragKind: "files" as DragKind,
  /** False until `init` has finished, while the startup splash shows. */
  ready: false,
  /** Mirrors the saved setting, so the backdrop can follow it without reloading settings. */
  showArtwork: true,
  toast: null as { text: string; error: boolean } | null,
  /** Bumped when share links may have changed (e.g. the relay connected); QR codes refetch. */
  linkEpoch: 0,
});

/** The Receive screen's state lives here so a preview survives switching tabs. */
export const receive = $state({
  link: "",
  checking: false,
  openingTicket: false,
  protectedTicket: null as string | null,
  preview: null as Preview | null,
  /** The link text the preview was made from. */
  previewLink: "",
  problem: null as { title: string; detail: string } | null,
  dest: null as string | null,
  starting: false,
});

const ACTIVE_DOWNLOAD = ["connecting", "downloading", "exporting"];

/** Downloads still under way: what the Receive tab's badge counts. */
export function activeDownloadCount(): number {
  return app.downloads.filter((d) => ACTIVE_DOWNLOAD.includes(d.status)).length;
}

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
  const incoming = (await api.takePendingLinks()).at(-1);
  if (!incoming) return;
  app.view = "receive";
  if (incoming.kind === "file") {
    await openTicket(incoming.path);
  } else {
    receive.link = incoming.link;
    await checkLink(incoming.link);
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

/** Open a ticket from the file picker, a drop, or the operating system. */
export async function openTicket(path: string, password?: string) {
  clearPreview();
  const token = checkToken;
  app.view = "receive";
  receive.link = "";
  receive.openingTicket = true;
  if (password !== undefined) receive.protectedTicket = path;
  try {
    const ticket = await api.readShareFile(path, password);
    if (token !== checkToken) return;
    if (ticket.kind === "passwordRequired") {
      receive.protectedTicket = path;
    } else {
      await checkLink(ticket.link);
    }
  } catch (err) {
    if (token !== checkToken) return;
    receive.problem = {
      title: password === undefined ? "Couldn't open the ticket file." : "Couldn't unlock the ticket file.",
      detail: message(err),
    };
  } finally {
    if (token === checkToken) receive.openingTicket = false;
  }
}

/** Contact the sender and show what the link contains. Only the latest check wins. */
export async function checkLink(text: string) {
  if (!text.trim()) return;
  const token = ++checkToken;
  receive.openingTicket = false;
  receive.protectedTicket = null;
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
  receive.openingTicket = false;
  receive.protectedTicket = null;
  receive.problem = null;
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
  void api
    .getSettings()
    .then((s) => (app.showArtwork = s.showArtwork))
    .catch(() => {});
  await listen<FerryEvent>("ferry-event", (e) => onEvent(e.payload));
  await listen("ferry-open-link", () => void takeIncoming());
  await listen("ferry-resync", () => void resync());

  // Dropping files anywhere shares them; dropping .ferry files opens them.
  await getCurrentWebview().onDragDropEvent(async (e) => {
    const p = e.payload;
    if (p.type === "enter") {
      app.dragKind = dragKind(p.paths);
      app.dragging = true;
    } else if (p.type === "over") app.dragging = true;
    else if (p.type === "leave") app.dragging = false;
    else if (p.type === "drop") {
      app.dragging = false;
      if (dragKind(p.paths) === "tickets") {
        await openTicket(p.paths[0]);
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
