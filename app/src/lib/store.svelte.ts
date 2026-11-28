import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { api, message } from "./api";
import { plural } from "./format";
import type { AppInfo, DownloadInfo, FerryEvent, NodeStatus, Preview, SaveState, ShareActivity, ShareInfo, WindowStyle } from "./types";

export type View = "send" | "receive" | "settings";
export type DragKind = "files" | "tickets";

/** Dropping only .ferry files opens the first one as a ticket; anything else is shared. */
export function dragKind(paths: string[]): DragKind {
  return paths.length > 0 && paths.every((x) => x.toLowerCase().endsWith(".ferry")) ? "tickets" : "files";
}

/** How the Send tab lists shares: full tickets, or one-line strips. */
export type ShareLayout = "tickets" | "compact";

interface Importing {
  doneBytes: number;
  totalBytes: number;
  currentFile: string;
  /** Phones first copy picked files into Ferry; then the ticket is prepared as on desktop. */
  copying: boolean;
}

const LAYOUT_KEY = "ferry.sendView";

export const app = $state({
  view: "send" as View,
  /** What kind of Ferry this is (desktop, or a phone build); null until `init` loads it. */
  info: null as AppInfo | null,
  /** Android: where each completed download's files went, by download id. */
  saved: {} as Record<string, SaveState>,
  status: null as NodeStatus | null,
  shares: [] as ShareInfo[],
  activity: {} as Record<string, ShareActivity>,
  /** Why a share couldn't be turned back on (its files changed), by share id. */
  shareProblems: {} as Record<string, string>,
  shareLayout: loadShareLayout(),
  downloads: [] as DownloadInfo[],
  importing: null as Importing | null,
  dragging: false,
  /** What a drop would do: share the files, or open the ticket files being dragged. */
  dragKind: "files" as DragKind,
  /** False until `init` has finished, while the startup splash shows. */
  ready: false,
  /** Set once the startup intro has had its minimum time; it stays up while init runs on. */
  introDone: false,
  /** Mirrors the saved setting, so the backdrop can follow it without reloading settings.
   *  Null until it has loaded, so artwork never shows to someone who turned it off. */
  showArtwork: null as boolean | null,
  /** Windows: Ferry is a panel by the tray icon rather than a window. Known with showArtwork. */
  panel: false,
  /** The panel stays open when focus moves away (e.g. to drag files in from Explorer). */
  pinned: false,
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

function loadShareLayout(): ShareLayout {
  try {
    return globalThis.localStorage?.getItem(LAYOUT_KEY) === "compact" ? "compact" : "tickets";
  } catch {
    return "tickets";
  }
}

export function setShareLayout(layout: ShareLayout) {
  app.shareLayout = layout;
  try {
    globalThis.localStorage?.setItem(LAYOUT_KEY, layout);
  } catch {
    // Storage can be unavailable; the choice then lasts until Ferry closes.
  }
}

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
          copying: false,
        };
      }
      break;
    case "shareCreated":
      app.shares = [event.share, ...app.shares.filter((s) => s.id !== event.share.id)];
      delete app.shareProblems[event.share.id];
      break;
    case "shareRemoved":
      app.shares = app.shares.filter((s) => s.id !== event.shareId);
      delete app.activity[event.shareId];
      delete app.shareProblems[event.shareId];
      break;
    case "shareUpdated": {
      const i = app.shares.findIndex((s) => s.id === event.share.id);
      if (i >= 0) app.shares[i] = event.share;
      if (!event.share.paused) delete app.shareProblems[event.share.id];
      // Turning a share off or on ends every transfer; keep only the total sent. (A snapshot
      // taken just before may still arrive; it is ignored below while the share is off, and
      // replaced by fresh ones once it is on.)
      const a = app.activity[event.share.id];
      if (a) app.activity[event.share.id] = { ...a, activePeers: 0, bytesPerSec: 0, receivers: [] };
      break;
    }
    case "shareActivity":
      if (app.shares.some((s) => s.id === event.shareId && !(s.paused && event.receivers.length > 0))) {
        app.activity[event.shareId] = {
          activePeers: event.activePeers,
          uploadedBytes: event.uploadedBytes,
          bytesPerSec: event.bytesPerSec,
          receivers: event.receivers,
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

/** Share files; resolves to the new share, or undefined if it didn't happen. */
export async function share(paths: string[]): Promise<ShareInfo | undefined> {
  if (paths.length === 0) return;
  return makeShare(() => api.createShare(paths), false);
}

/** Phones: let the user pick files or a folder, then share them. */
export async function shareFromPhone(kind: "files" | "folder") {
  let uris: string[];
  try {
    uris = await api.pickItems(kind);
  } catch (err) {
    toast(`Couldn't open the file picker: ${message(err)}`, true);
    return;
  }
  if (uris.length > 0) await makeShare(() => api.shareItems(uris), true);
}

let incoming = Promise.resolve();

/** Phones: share what was sent to Ferry from other apps' Share menus, one batch at a time. */
function takeSharedFiles() {
  incoming = incoming.then(async () => {
    const batches = await api.takeIncoming().catch(() => [] as string[][]);
    for (const uris of batches) {
      app.view = "send";
      // A ticket started from Ferry's own pickers finishes first.
      while (app.importing) await new Promise((resolve) => setTimeout(resolve, 300));
      await makeShare(() => api.shareItems(uris), true);
    }
  });
}

async function makeShare(create: () => Promise<ShareInfo>, copying: boolean): Promise<ShareInfo | undefined> {
  if (app.importing) {
    toast("Ferry is still preparing the previous ticket. Try again when it's done.", true);
    return;
  }
  app.importing = { doneBytes: 0, totalBytes: 0, currentFile: "", copying };
  try {
    const created = await create();
    const left = created.skippedCount ?? 0;
    toast(
      left > 0
        ? `Sharing ${created.name}. ${plural(left, "item was", "items were")} left out; see the ticket.`
        : `Sharing ${created.name}`,
    );
    return created;
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

/** Loads the settings that shape the screen: artwork, and panel or window. The panel is known
 *  first, as the splash starts once the artwork setting is. If settings can't be read, the
 *  defaults apply. */
async function loadDisplaySettings() {
  // Separately, so app info failing can't discard a saved "artwork off".
  const [settings, info] = await Promise.allSettled([api.getSettings(), api.appInfo()]);
  if (settings.status === "fulfilled" && info.status === "fulfilled") {
    app.panel = isPanelStyle(info.value, settings.value.windowStyle);
  }
  app.showArtwork = settings.status === "fulfilled" ? settings.value.showArtwork : true;
}

/** Only Windows has the tray panel; elsewhere the setting is ignored. */
export function isPanelStyle(info: AppInfo, style: WindowStyle | undefined): boolean {
  return info.platform === "windows" && !info.mobile && (style ?? "panel") === "panel";
}

export function isPanel(): boolean {
  return app.panel;
}

export async function setPinned(pinned: boolean) {
  app.pinned = pinned;
  try {
    await api.setPanelPinned(pinned);
  } catch (err) {
    app.pinned = !pinned;
    toast(message(err), true);
  }
}

export async function init() {
  // Started first so it runs alongside the rest, and awaited even if the rest fails: the
  // splash and backdrop must not settle before the saved settings are known.
  const displaySettings = loadDisplaySettings();
  try {
    await initRest();
  } finally {
    await displaySettings;
  }
}

async function loadSaved() {
  try {
    app.saved = await api.savedDownloads();
  } catch (err) {
    toast(`Couldn't check saved downloads: ${message(err)}`, true);
  }
}

/** Phones open on Receive (scanning a code is the common case) and can't drop files. */
/** Whether the startup splash still covers the UI, which is inert until it goes. */
export function splashing(): boolean {
  return !app.ready || !app.introDone;
}

export function isMobile(): boolean {
  return app.info?.mobile ?? false;
}

async function initRest() {
  app.info = await api.appInfo();
  if (isMobile()) app.view = "receive";
  if (app.info.platform === "android") {
    await listen("ferry-saved", () => void loadSaved());
    await loadSaved();
    await listen<{ doneBytes: number; totalBytes: number }>("ferry-staging", (e) => {
      if (app.importing?.copying) app.importing = { ...app.importing, ...e.payload };
    });
    await listen<string>("ferry-notice", (e) => toast(e.payload));
    await listen("ferry-incoming", () => takeSharedFiles());
  }
  await listen<FerryEvent>("ferry-event", (e) => onEvent(e.payload));
  await listen("ferry-open-link", () => void takeIncoming());
  await listen("ferry-resync", () => void resync());

  // Dropping files anywhere shares them; dropping .ferry files opens them. Phones can't drop.
  if (!isMobile()) await getCurrentWebview().onDragDropEvent(async (e) => {
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
  // Files shared to Ferry before the UI was listening (Ferry was opened from a Share menu).
  if (app.info.platform === "android") takeSharedFiles();
}
