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
    link: "https://ferry.xenvaultinteractive.com/s#AXbqm9jZ2Kc0eNhgEaQqWfH1aE3qB5JbXy7Yk2LmfaPbaQ",
    code: "K7X2QP9A4MZT",
    locked: false,
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
    link: "https://ferry.xenvaultinteractive.com/s#AXbqm9jZ2Kc0eNhgEaQqWfH1aE3qB5JbXy7Yk2LmfaPbbQ",
    code: "6N76Y6BE0EQX",
    locked: true,
  },
  {
    id: hex(8),
    name: "Old build",
    sourcePaths: ["C:\\Users\\alex\\Projects\\build"],
    fileCount: 12,
    totalSize: 838_000_000,
    createdAt: now - 86400 * 9,
    link: "https://ferry.xenvaultinteractive.com/s#AXbqm9jZ2Kc0eNhgEaQqWfH1aE3qB5JbXy7Yk2LmfaPbcQ",
    code: "Q3M8D2TW7HRA",
    locked: false,
    paused: true,
  },
  ...Array.from({ length: 8 }, (_, i): ShareInfo => ({
    id: `${hex(10 + i).slice(0, 56)}5ca9${String(i).padStart(4, "0")}`,
    name: `Scan ${String(i + 1).padStart(2, "0")}.pdf`,
    sourcePaths: [`C:\\Users\\alex\\Documents\\Scan ${i + 1}.pdf`],
    fileCount: 1,
    totalSize: 1_200_000 + i * 310_000,
    createdAt: now - 86400 * (10 + i),
    link: `https://ferry.xenvaultinteractive.com/s#AXbqm9jZ2Kc0eNhgEaQqWfH1aE3qB5JbXy7Yk2Lmf${i}`,
    code: `SCAN${String(i).padStart(4, "0")}A1B2`,
    locked: false,
  })),
];

const downloads: DownloadInfo[] = [
  {
    id: hex(3), link: "", destDir: "C:\\Users\\alex\\Downloads", name: "Wedding video", status: "downloading",
    doneBytes: 1_830_000_000, totalBytes: 4_100_000_000, createdAt: now, bytesPerSec: 6_400_000, etaSecs: 355,
    connection: "relay",
  },
  {
    id: hex(4), link: "", destDir: "C:\\Users\\alex\\Downloads", name: "Band rehearsal", status: "paused",
    doneBytes: 310_000_000, totalBytes: 890_000_000, createdAt: now - 500, bytesPerSec: 0,
  },
  {
    id: hex(5), link: "", destDir: "C:\\Users\\alex\\Downloads", name: "Recipes", status: "completed",
    doneBytes: 12_000_000, totalBytes: 12_000_000, createdAt: now - 4000, bytesPerSec: 0,
    savedPath: "C:\\Users\\alex\\Downloads\\Recipes",
  },
  {
    id: hex(6), link: "", destDir: "C:\\Users\\alex\\Downloads", name: "Old photos", status: "failed",
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

// ?empty starts with nothing shared or downloaded; ?slow makes starting take 2 seconds, longer
// than the startup intro.
// ?mobile previews the phone build.
const params = new URLSearchParams(location.search);
const empty = params.has("empty");
const slow = params.has("slow");
const mobile = params.has("mobile");

const status = {
  nodeId: hex(9),
  relayUrl: "https://euw1-1.relay.n0.iroh.link./",
  directAddresses: ["192.168.1.20:51820"],
};
const settings = {
  downloadDir: "C:\\Users\\alex\\Downloads",
  relay: { mode: "default" },
  closeToTray: true,
  showArtwork: true,
  shortLinks: true,
};

export function installDevPreview() {
  mockWindows("main");
  mockIPC(
    (cmd, args) => {
      switch (cmd) {
        case "get_share":
          return shares.find((s) => s.id === (args as { id: string }).id);
        case "node_status":
          if (slow) return new Promise((done) => setTimeout(() => done(status), 2000));
          return status;
        case "set_share_paused": {
          const { id, paused } = args as { id: string; paused: boolean };
          const share = shares.find((s) => s.id === id);
          if (!share) throw "unknown share";
          if (!paused && share.name === "Old build") throw "Old build/app.exe was moved, deleted or changed since this ticket was made";
          share.paused = paused;
          setTimeout(() => void emit("ferry-event", { type: "shareUpdated", share: { ...share } }));
          return share;
        }
        case "list_shares":
          return empty ? [] : shares;
        case "list_downloads":
          return empty ? [] : downloads;
        case "open_project_page":
        case "open_third_party_notices":
          return null;
        case "share_qr":
          return fakeQr;
        case "get_settings":
          return settings;
        case "set_settings":
          Object.assign(settings, (args as { settings: typeof settings }).settings);
          return false;
        case "app_info":
          return mobile
            ? { platform: "android", mobile: true, distribution: "direct" }
            : { platform: "windows", mobile: false, distribution: "desktop" };
        case "saved_downloads":
          return {
            [downloads[2].id]: {
              state: "saved",
              location: "Download/Ferry/Recipes",
              files: [{ uri: "content://x/1", name: "a.pdf" }, { uri: "content://x/2", name: "b.pdf" }],
            },
          };
        case "open_download":
        case "share_download":
        case "retry_save":
        case "share_link":
          return null;
        case "take_incoming":
          return [];
        case "pick_items":
          return ["content://com.android.providers.media.documents/document/image%3A1001"];
        case "share_items":
          // Copying, then preparing the ticket, then a new share.
          return new Promise((done) => {
            const total = 48_000_000;
            let copied = 0;
            const copy = setInterval(() => {
              copied = Math.min(total, copied + 6_000_000);
              void emit("ferry-staging", { doneBytes: copied, totalBytes: total });
              if (copied < total) return;
              clearInterval(copy);
              void emit("ferry-event", { type: "importProgress", opId: 1, doneBytes: total, totalBytes: total, currentFile: "IMG_2041.jpg" });
              const share = { ...shares[0], id: hex(32), name: "IMG_2041.jpg", fileCount: 1, totalSize: total, createdAt: Date.now() / 1000, sourcePaths: ["/data/user/0/com.xenvaultinteractive.ferry/outgoing/1/IMG_2041.jpg"], skipped: [], skippedCount: 0, paused: false };
              shares.unshift(share);
              setTimeout(() => {
                void emit("ferry-event", { type: "shareCreated", share });
                done(share);
              }, 600);
            }, 250);
          });
        case "update_info":
          return { version, support: "unavailable", autoCheck: true, readyVersion: null };
        case "take_pending_links":
          return [];
        case "set_share_password": {
          const share = shares.find((s) => s.id === (args as { id: string }).id)!;
          share.locked = (args as { password: string | null }).password !== null;
          void emit("ferry-event", { type: "shareUpdated", share });
          return share;
        }
        case "preview":
          return {
            kind: "ready",
            preview: {
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
            },
          };
        default:
          return null;
      }
    },
    { shouldMockEvents: true },
  );

  // One share has two people downloading right now; their progress moves along.
  let tick = 0;
  setInterval(() => {
    tick++;
    const a = Math.min(621_600_000, 214_000_000 + tick * 2_100_000);
    const b = Math.min(310_000_000, 18_000_000 + tick * 900_000);
    void emit("ferry-event", {
      type: "shareActivity",
      shareId: shares[0].id,
      activePeers: 2,
      uploadedBytes: a + b,
      bytesPerSec: 6_000_000,
      receivers: [
        { id: 1, doneBytes: a, totalBytes: 621_600_000, bytesPerSec: 4_200_000, etaSecs: Math.round((621_600_000 - a) / 4_200_000) },
        { id: 2, doneBytes: b, totalBytes: 310_000_000, bytesPerSec: 1_800_000, etaSecs: Math.round((310_000_000 - b) / 1_800_000) },
      ],
    });
  }, 500);
}
