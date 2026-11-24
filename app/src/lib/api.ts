import { invoke } from "@tauri-apps/api/core";
import type { AppInfo, DownloadInfo, NodeStatus, Preview, SaveState, Settings, ShareInfo, UpdateInfo } from "./types";

type IncomingShare = { kind: "link"; link: string } | { kind: "file"; path: string };
type OpenedTicket = { kind: "ready"; link: string } | { kind: "passwordRequired" };
/** A preview, or why the sender wants a (different) password first. */
export type PreviewOutcome =
  | { kind: "ready"; preview: Preview }
  | { kind: "passwordRequired" }
  | { kind: "wrongPassword" }
  | { kind: "tooManyGuesses"; retryAfterSecs: number };

export const api = {
  nodeStatus: () => invoke<NodeStatus>("node_status"),
  listShares: () => invoke<ShareInfo[]>("list_shares"),
  /** A share with a link built from the current addresses. */
  getShare: (id: string) => invoke<ShareInfo>("get_share", { id }),
  createShare: (paths: string[]) => invoke<ShareInfo>("create_share", { paths }),
  removeShare: (id: string) => invoke<void>("remove_share", { id }),
  /** Turn a share off (paused) or back on; turning on fails if its files changed. */
  setSharePaused: (id: string, paused: boolean) => invoke<ShareInfo>("set_share_paused", { id, paused }),
  /** A locked share's ticket file needs its password, and is encrypted with it. */
  saveShareFile: (id: string, path: string, password?: string) =>
    invoke<void>("save_share_file", { id, path, password: password ?? null }),
  /** Protect a share with a password, change it, or remove it (`null`). */
  setSharePassword: (id: string, password: string | null) =>
    invoke<ShareInfo>("set_share_password", { id, password }),
  shareQr: (id: string) => invoke<string>("share_qr", { id }),
  readShareFile: (path: string, password?: string) =>
    invoke<OpenedTicket>("read_share_file", { path, password: password ?? null }),
  preview: (link: string, password?: string) =>
    invoke<PreviewOutcome>("preview", { link, password: password ?? null }),
  listDownloads: () => invoke<DownloadInfo[]>("list_downloads"),
  startDownload: (link: string, dest?: string, password?: string) =>
    invoke<DownloadInfo>("start_download", { link, dest: dest ?? null, password: password ?? null }),
  pauseDownload: (id: string) => invoke<void>("pause_download", { id }),
  resumeDownload: (id: string) => invoke<void>("resume_download", { id }),
  removeDownload: (id: string) => invoke<void>("remove_download", { id }),
  revealDownload: (id: string) => invoke<void>("reveal_download", { id }),
  getSettings: () => invoke<Settings>("get_settings"),
  setSettings: (settings: Settings) => invoke<boolean>("set_settings", { settings }),
  updateInfo: () => invoke<UpdateInfo>("update_info"),
  setAutoCheckUpdates: (enabled: boolean) => invoke<void>("set_auto_check_updates", { enabled }),
  checkForUpdates: (automatic: boolean) => invoke<string | null>("check_for_updates", { automatic }),
  installUpdate: () => invoke<void>("install_update"),
  takePendingLinks: () => invoke<IncomingShare[]>("take_pending_links"),
  appInfo: () => invoke<AppInfo>("app_info"),
  // Android only: received files are copied into Downloads/Ferry.
  savedDownloads: () => invoke<Record<string, SaveState>>("saved_downloads"),
  openDownload: (id: string) => invoke<void>("open_download", { id }),
  shareDownload: (id: string) => invoke<void>("share_download", { id }),
  retrySave: (id: string) => invoke<void>("retry_save", { id }),
  // Android only: sending from the phone. Picked files are copied into Ferry and shared.
  pickItems: (kind: "files" | "folder") => invoke<string[]>("pick_items", { kind }),
  takeIncoming: () => invoke<string[][]>("take_incoming"),
  shareItems: (uris: string[]) => invoke<ShareInfo>("share_items", { uris }),
  shareLink: (id: string) => invoke<void>("share_link", { id }),
  openProjectPage: () => invoke<void>("open_project_page"),
  openThirdPartyNotices: () => invoke<void>("open_third_party_notices"),
};

/** Errors from commands arrive as plain strings. */
export function message(err: unknown): string {
  return typeof err === "string" ? err : err instanceof Error ? err.message : String(err);
}
