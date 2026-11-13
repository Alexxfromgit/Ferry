// Mirrors the serde types in crates/ferry-core (camelCase JSON).

export type ConnectionKind = "direct" | "relay";

export interface FileEntry {
  name: string;
  size: number;
}

export type SkipReason = "systemFile" | "link" | "emptyFolder" | "unreadable" | "notAFile";

export interface SkippedItem {
  name: string;
  reason: SkipReason;
}

export interface ShareInfo {
  id: string;
  name: string;
  sourcePaths: string[];
  /** Number of files (the list itself stays in the engine). */
  fileCount: number;
  totalSize: number;
  createdAt: number;
  link: string;
  ticket: string;
  /** Up to 100 items left out of the share; `skippedCount` is the full number. */
  skipped?: SkippedItem[];
  skippedCount?: number;
}

export type DownloadStatus =
  | "connecting"
  | "downloading"
  | "exporting"
  | "completed"
  | "paused"
  | "failed";

export interface DownloadInfo {
  id: string;
  ticket: string;
  destDir: string;
  name: string;
  status: DownloadStatus;
  doneBytes: number;
  totalBytes: number;
  createdAt: number;
  savedPath?: string;
  error?: string;
  /** Top-level names chosen in `destDir` when saving started (original → saved as). */
  exportNames?: Record<string, string>;
  bytesPerSec: number;
  etaSecs?: number;
  connection?: ConnectionKind;
}

export interface Preview {
  id: string;
  name: string;
  /** The first 200 files; fileCount is the full number. */
  files: FileEntry[];
  fileCount: number;
  totalSize: number;
  connection?: ConnectionKind;
}

export interface NodeStatus {
  nodeId: string;
  relayUrl?: string;
  directAddresses: string[];
}

export type RelayChoice =
  | { mode: "default" }
  | { mode: "disabled" }
  | { mode: "custom"; url: string };

export interface Settings {
  downloadDir: string;
  relay: RelayChoice;
  closeToTray: boolean;
  showArtwork: boolean;
}

export type FerryEvent =
  | { type: "importProgress"; opId: number; doneBytes: number; totalBytes: number; currentFile: string }
  | { type: "shareCreated"; share: ShareInfo }
  | { type: "shareRemoved"; shareId: string }
  | { type: "shareActivity"; shareId: string; activePeers: number; uploadedBytes: number }
  | { type: "downloadUpdated"; download: DownloadInfo }
  | { type: "downloadRemoved"; downloadId: string };
