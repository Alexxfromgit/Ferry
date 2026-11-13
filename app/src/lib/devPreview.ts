// Dev-only: lets the UI run in a plain browser (`npm run dev`) with sample data, for design work
// and screenshots. Loaded only when Vite is in dev mode and there is no Tauri backend.

import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import { version } from "../../package.json";
import type { DownloadInfo, ShareInfo } from "./types";

const now = Math.floor(Date.now() / 1000);
const hex = (seed: number) =>
  Array.from({ length: 64 }, (_, i) => ((i * 7 + seed * 13) % 16).toString(16)).join("");

const shares: ShareInfo[] = [
  {
    id: hex(1),
    name: "Holiday in Lisbon",
    sourcePaths: ["C:\\Users\\alex\\Pictures\\2026\\Holiday in Lisbon"],
    fileCount: 148,
    totalSize: 621_600_000,
    createdAt: now - 3600,
    link: "ferry://get?t=blobaa5jnzjibhu5omj572hveqe33ruu5itl",
    ticket: "blobaa5jnzjibhu5omj572hveqe33ruu5itl",
    skipped: [
      { name: "Holiday in Lisbon/.DS_Store", reason: "systemFile" },
      { name: "Holiday in Lisbon/Edits (old)", reason: "emptyFolder" },
      { name: "Holiday in Lisbon/Best of", reason: "link" },
    ],
    skippedCount: 3,
  },
  {
    id: hex(2),
    name: "Quarterly report.pdf",
    sourcePaths: ["C:\\Users\\alex\\Documents\\Quarterly report.pdf"],
    fileCount: 1,
    totalSize: 2_340_000,
    createdAt: now - 86400 * 2,
    link: "ferry://get?t=blobaafakesecond",
    ticket: "blobaafakesecond",
  },
];

const downloads: DownloadInfo[] = [
  {
    id: hex(3), ticket: "", destDir: "C:\\Users\\alex\\Downloads", name: "Wedding video", status: "downloading",
    doneBytes: 1_830_000_000, totalBytes: 4_100_000_000, createdAt: now, bytesPerSec: 6_400_000, etaSecs: 355,
    connection: "relay",
  },
  {
    id: hex(4), ticket: "", destDir: "C:\\Users\\alex\\Downloads", name: "Band rehearsal", status: "paused",
    doneBytes: 310_000_000, totalBytes: 890_000_000, createdAt: now - 500, bytesPerSec: 0,
  },
  {
    id: hex(5), ticket: "", destDir: "C:\\Users\\alex\\Downloads", name: "Recipes", status: "completed",
    doneBytes: 12_000_000, totalBytes: 12_000_000, createdAt: now - 4000, bytesPerSec: 0,
    savedPath: "C:\\Users\\alex\\Downloads\\Recipes",
  },
  {
    id: hex(6), ticket: "", destDir: "C:\\Users\\alex\\Downloads", name: "Old photos", status: "failed",
    doneBytes: 0, totalBytes: 0, createdAt: now - 9000, bytesPerSec: 0,
    error: "could not reach the sender (is their Ferry running?)",
  },
];

// A small black-and-white grid that looks like a QR code.
const fakeQr = (() => {
  let cells = "";
  for (let y = 0; y < 25; y++)
    for (let x = 0; x < 25; x++) {
      const finder = (x < 7 && y < 7) || (x > 17 && y < 7) || (x < 7 && y > 17);
      const on = finder
        ? !(x % 6 === 1 || y % 6 === 1) || (x % 18 >= 2 && x % 18 <= 4 && y % 18 >= 2 && y % 18 <= 4)
        : (x * 31 + y * 17 + x * y) % 3 === 0;
      if (on) cells += `<rect x="${x}" y="${y}" width="1" height="1"/>`;
    }
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 25 25" fill="#2A0D20">${cells}</svg>`;
})();

export function installDevPreview() {
  mockWindows("main");
  mockIPC(
    (cmd, args) => {
      switch (cmd) {
        case "get_share":
          return shares.find((s) => s.id === (args as { id: string }).id);
        case "node_status":
          return { nodeId: hex(9), relayUrl: "https://euw1-1.relay.n0.iroh.link./", directAddresses: ["192.168.1.20:51820"] };
        case "list_shares":
          return shares;
        case "list_downloads":
          return downloads;
        case "share_qr":
          return fakeQr;
        case "get_settings":
          return { downloadDir: "C:\\Users\\alex\\Downloads", relay: { mode: "default" }, closeToTray: true };
        case "update_info":
          return { version, support: "unavailable", autoCheck: true, readyVersion: null };
        case "take_pending_links":
          return [];
        case "preview":
          return {
            id: hex(7),
            name: "Concert photos",
            files: [
              { name: "Concert photos/stage-01.jpg", size: 5_100_000 },
              { name: "Concert photos/stage-02.jpg", size: 4_800_000 },
              { name: "Concert photos/crowd.mp4", size: 182_000_000 },
            ],
            fileCount: 3,
            totalSize: 191_900_000,
            connection: "direct",
          };
        default:
          return null;
      }
    },
    { shouldMockEvents: true },
  );

  // One share has someone downloading right now.
  setTimeout(() => {
    void emit("ferry-event", {
      type: "shareActivity",
      shareId: shares[0].id,
      activePeers: 1,
      uploadedBytes: 214_000_000,
    });
  }, 600);
}
