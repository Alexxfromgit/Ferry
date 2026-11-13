import { invoke } from "@tauri-apps/api/core";
import type { DownloadInfo, NodeStatus, Preview, Settings, ShareInfo } from "./types";

export const api = {
  nodeStatus: () => invoke<NodeStatus>("node_status"),
  listShares: () => invoke<ShareInfo[]>("list_shares"),
  /** A share with a link built from the current addresses. */
  getShare: (id: string) => invoke<ShareInfo>("get_share", { id }),
  createShare: (paths: string[]) => invoke<ShareInfo>("create_share", { paths }),
  removeShare: (id: string) => invoke<void>("remove_share", { id }),
  saveShareFile: (id: string, path: string) => invoke<void>("save_share_file", { id, path }),
  shareQr: (id: string) => invoke<string>("share_qr", { id }),
  readShareFile: (path: string) => invoke<string>("read_share_file", { path }),
  preview: (link: string) => invoke<Preview>("preview", { link }),
  listDownloads: () => invoke<DownloadInfo[]>("list_downloads"),
  startDownload: (link: string, dest?: string) =>
    invoke<DownloadInfo>("start_download", { link, dest: dest ?? null }),
  pauseDownload: (id: string) => invoke<void>("pause_download", { id }),
  resumeDownload: (id: string) => invoke<void>("resume_download", { id }),
  removeDownload: (id: string) => invoke<void>("remove_download", { id }),
  revealDownload: (id: string) => invoke<void>("reveal_download", { id }),
  getSettings: () => invoke<Settings>("get_settings"),
  setSettings: (settings: Settings) => invoke<boolean>("set_settings", { settings }),
  takePendingLinks: () => invoke<string[]>("take_pending_links"),
};

/** Errors from commands arrive as plain strings. */
export function message(err: unknown): string {
  return typeof err === "string" ? err : err instanceof Error ? err.message : String(err);
}
