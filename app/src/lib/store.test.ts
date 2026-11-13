// Tests for the UI state logic in store.svelte.ts, with the Tauri backend mocked.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { downloads as sampleDownloads, shares as sampleShares } from "./contract.generated";
import type { DownloadInfo, FerryEvent, Preview, ShareInfo } from "./types";

type Handler = (event: { payload: unknown }) => void;
type DropHandler = (event: { payload: { type: string; paths?: string[] } }) => Promise<void>;

const handlers: Record<string, Handler> = {};
let dropHandler: DropHandler | undefined;
const invoke = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invoke(...args) }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: async (name: string, cb: Handler) => {
    handlers[name] = cb;
    return () => {};
  },
}));
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({
    onDragDropEvent: async (cb: DropHandler) => {
      dropHandler = cb;
      return () => {};
    },
  }),
}));

const share = (id: string, extra: Partial<ShareInfo> = {}): ShareInfo => ({
  ...sampleShares[0],
  id,
  paused: false,
  ...extra,
});
const download = (id: string, extra: Partial<DownloadInfo> = {}): DownloadInfo => ({
  ...sampleDownloads[0],
  id,
  ...extra,
});
const activity = {
  activePeers: 1,
  uploadedBytes: 5,
  bytesPerSec: 2048,
  receivers: [{ id: 1, doneBytes: 5, totalBytes: 42, bytesPerSec: 2048, etaSecs: 1 }],
};
const preview = (name: string): Preview => ({ id: name, name, files: [], fileCount: 0, totalSize: 0 });

/** Default backend: empty lists, no pending links. Tests override single commands. */
let backend: Record<string, (args: any) => unknown>;

function emit(event: FerryEvent) {
  handlers["ferry-event"]({ payload: event });
}

async function freshStore() {
  vi.resetModules();
  const store = await import("./store.svelte");
  await store.init();
  return store;
}

beforeEach(() => {
  vi.useFakeTimers();
  for (const key of Object.keys(handlers)) delete handlers[key];
  dropHandler = undefined;
  backend = {
    list_shares: () => [],
    list_downloads: () => [],
    node_status: () => ({ nodeId: "n", directAddresses: [] }),
    take_pending_links: () => [],
    get_settings: () => ({ downloadDir: "/d", relay: { mode: "default" }, closeToTray: true, showArtwork: true }),
  };
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string, args: unknown) => {
    const handler = backend[cmd];
    if (!handler) throw new Error(`unexpected command ${cmd}`);
    return handler(args);
  });
});

afterEach(() => {
  vi.useRealTimers();
});

describe("events", () => {
  it("updates a download in place and removes it", async () => {
    const { app } = await freshStore();
    emit({ type: "downloadUpdated", download: download("d1", { doneBytes: 1 }) });
    emit({ type: "downloadUpdated", download: download("d2") });
    emit({ type: "downloadUpdated", download: download("d1", { doneBytes: 5 }) });
    expect(app.downloads.map((d) => [d.id, d.doneBytes])).toEqual([
      ["d2", 10],
      ["d1", 5],
    ]);
    emit({ type: "downloadRemoved", downloadId: "d1" });
    expect(app.downloads.map((d) => d.id)).toEqual(["d2"]);
  });

  it("forgets the activity of a removed share and ignores activity for unknown shares", async () => {
    const { app } = await freshStore();
    emit({ type: "shareCreated", share: share("s1") });
    emit({ type: "shareActivity", shareId: "s1", ...activity });
    emit({ type: "shareActivity", shareId: "ghost", ...activity });
    expect(Object.keys(app.activity)).toEqual(["s1"]);
    emit({ type: "shareRemoved", shareId: "s1" });
    expect(app.shares).toEqual([]);
    expect(app.activity).toEqual({});
  });

  it("keeps each receiver's progress and clears it when a share is turned off", async () => {
    const { app } = await freshStore();
    emit({ type: "shareCreated", share: share("s1") });
    emit({ type: "shareActivity", shareId: "s1", ...activity });
    expect(app.activity.s1.receivers).toHaveLength(1);
    expect(app.activity.s1.bytesPerSec).toBe(2048);

    emit({ type: "shareUpdated", share: share("s1", { paused: true }) });
    expect(app.shares[0].paused).toBe(true);
    expect(app.activity.s1).toEqual({ activePeers: 0, uploadedBytes: 5, bytesPerSec: 0, receivers: [] });
  });

  it("ignores a late snapshot for a share that is off, and starts clean once it is on", async () => {
    const { app } = await freshStore();
    emit({ type: "shareCreated", share: share("s1") });
    emit({ type: "shareUpdated", share: share("s1", { paused: true }) });
    // Taken just before the share was turned off, delivered after.
    emit({ type: "shareActivity", shareId: "s1", ...activity });
    expect(app.activity.s1?.receivers ?? []).toEqual([]);

    emit({ type: "shareUpdated", share: share("s1", { paused: false }) });
    emit({ type: "shareActivity", shareId: "s1", ...activity });
    expect(app.activity.s1.receivers).toHaveLength(1);
    emit({ type: "shareUpdated", share: share("s1", { paused: false }) });
    expect(app.activity.s1.receivers).toEqual([]);
    expect(app.activity.s1.uploadedBytes).toBe(5);
  });

  it("forgets a share's problem once it is back on", async () => {
    const { app } = await freshStore();
    emit({ type: "shareCreated", share: share("s1", { paused: true }) });
    app.shareProblems.s1 = "a.txt was moved";
    emit({ type: "shareUpdated", share: share("s1", { paused: false }) });
    expect(app.shareProblems).toEqual({});
  });

  it("remembers the list layout", async () => {
    const saved = new Map<string, string>();
    // Node's own `localStorage` is an accessor, so replace the property itself.
    const original = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
    Object.defineProperty(globalThis, "localStorage", {
      configurable: true,
      value: {
        getItem: (k: string) => saved.get(k) ?? null,
        setItem: (k: string, v: string) => void saved.set(k, v),
      },
    });
    const store = await freshStore();
    expect(store.app.shareLayout).toBe("tickets");
    store.setShareLayout("compact");
    const again = await freshStore();
    expect(again.app.shareLayout).toBe("compact");
    if (original) Object.defineProperty(globalThis, "localStorage", original);
    else delete (globalThis as { localStorage?: unknown }).localStorage;
  });

  it("reloads everything on resync and refreshes links", async () => {
    const { app } = await freshStore();
    const epoch = app.linkEpoch;
    backend.list_shares = () => [share("s1")];
    backend.list_downloads = () => [download("d1")];
    handlers["ferry-resync"]({ payload: null });
    await vi.waitFor(() => expect(app.shares.map((s) => s.id)).toEqual(["s1"]));
    expect(app.downloads.map((d) => d.id)).toEqual(["d1"]);
    expect(app.linkEpoch).toBe(epoch + 1);
  });
});

describe("updates", () => {
  it("honors the automatic check preference and lets users check manually", async () => {
    await freshStore();
    backend.update_info = () => ({
      version: "0.1.0",
      support: "available",
      autoCheck: false,
      readyVersion: null,
    });
    backend.check_for_updates = () => "0.2.0";
    const { initUpdates, checkForUpdates, updates } = await import("./updates.svelte");
    const stop = initUpdates();
    await vi.advanceTimersByTimeAsync(10_000);
    expect(invoke).not.toHaveBeenCalledWith("check_for_updates", expect.anything());

    await checkForUpdates();
    expect(invoke).toHaveBeenCalledWith("check_for_updates", { automatic: false });
    expect(updates.info?.readyVersion).toBe("0.2.0");
    stop();
  });
});

describe("checking links", () => {
  it("keeps only the newest preview when checks overlap", async () => {
    const { receive, checkLink } = await freshStore();
    let resolveSlow!: (p: Preview) => void;
    backend.preview = ({ link }: { link: string }) =>
      link === "slow"
        ? new Promise<Preview>((r) => (resolveSlow = r))
        : Promise.resolve(preview("fast"));
    const slow = checkLink("slow");
    await checkLink("fast");
    resolveSlow(preview("slow"));
    await slow;
    expect(receive.preview?.name).toBe("fast");
    expect(receive.previewLink).toBe("fast");
    expect(receive.checking).toBe(false);
  });

  it("explains unreachable senders differently from bad links", async () => {
    const { receive, checkLink } = await freshStore();
    backend.preview = () => Promise.reject("could not reach the sender (is their Ferry running?)");
    await checkLink("ferry://get?t=x");
    expect(receive.problem?.title).toMatch(/reach/);
    backend.preview = () => Promise.reject("invalid share ticket");
    await checkLink("garbage");
    expect(receive.problem?.title).toMatch(/isn't a Ferry link/);
  });

  it("opens links handed over from outside on the Receive screen", async () => {
    const { app, receive } = await freshStore();
    backend.take_pending_links = () => [{ kind: "link", link: "ferry://get?t=blob" }];
    backend.preview = () => Promise.resolve(preview("from link"));
    handlers["ferry-open-link"]({ payload: null });
    await vi.waitFor(() => expect(receive.preview?.name).toBe("from link"));
    expect(app.view).toBe("receive");
    expect(receive.link).toBe("ferry://get?t=blob");
  });

  it("prompts for a protected ticket opened from the desktop and lets the user retry", async () => {
    const { app, receive, openTicket } = await freshStore();
    const path = "C:/x/protected.ferry";
    backend.take_pending_links = () => [{ kind: "file", path }];
    backend.read_share_file = ({ password }) => {
      if (password === null) return { kind: "passwordRequired" };
      if (password === "correct") return { kind: "ready", link: "ferry://get?t=blob" };
      throw new Error("Incorrect password");
    };
    backend.preview = () => Promise.resolve(preview("from file"));
    handlers["ferry-open-link"]({ payload: null });
    await vi.waitFor(() => expect(receive.protectedTicket).toBe(path));
    expect(app.view).toBe("receive");
    expect(invoke).not.toHaveBeenCalledWith("preview", expect.anything());

    await openTicket(path, "wrong");
    expect(receive.protectedTicket).toBe(path);
    expect(receive.problem?.detail).toMatch(/Incorrect password/);

    await openTicket(path, "correct");
    expect(receive.protectedTicket).toBeNull();
    expect(receive.preview?.name).toBe("from file");
    expect(receive.link).toBe("");
  });
});

describe("sharing", () => {
  it("mentions what was left out", async () => {
    const { app, share: shareFiles } = await freshStore();
    backend.create_share = () => share("s1", { name: "Trip", skippedCount: 3 });
    await shareFiles(["/home/example/Trip"]);
    expect(app.toast?.text).toMatch(/Sharing Trip\. 3 items were left out/);
    expect(app.importing).toBeNull();
  });

  it("refuses a second share while one is being prepared", async () => {
    const { app, share: shareFiles } = await freshStore();
    let finish!: (s: ShareInfo) => void;
    backend.create_share = () => new Promise<ShareInfo>((r) => (finish = r));
    const first = shareFiles(["/a"]);
    await shareFiles(["/b"]);
    expect(app.toast?.error).toBe(true);
    expect(invoke.mock.calls.filter(([cmd]) => cmd === "create_share")).toHaveLength(1);
    finish(share("s1"));
    await first;
  });
});

describe("drag and drop", () => {
  it("opens dropped .ferry files and shares anything else", async () => {
    const { app, receive } = await freshStore();
    backend.read_share_file = () => ({ kind: "ready", link: "ferry://get?t=blob" });
    backend.preview = () => Promise.resolve(preview("dropped"));
    await dropHandler!({ payload: { type: "drop", paths: ["C:/x/trip.ferry"] } });
    expect(app.view).toBe("receive");
    expect(receive.preview?.name).toBe("dropped");

    backend.create_share = () => share("s2", { name: "Photos" });
    await dropHandler!({ payload: { type: "drop", paths: ["C:/x/Photos", "C:/x/trip.ferry"] } });
    expect(app.view).toBe("send");
    expect(invoke).toHaveBeenCalledWith("create_share", { paths: ["C:/x/Photos", "C:/x/trip.ferry"] });
  });

  it("tells the overlay whether a drop would open tickets or share files", async () => {
    const { app } = await freshStore();
    await dropHandler!({ payload: { type: "enter", paths: ["C:/x/a.FERRY", "C:/x/b.ferry"] } });
    expect(app.dragging).toBe(true);
    expect(app.dragKind).toBe("tickets");
    await dropHandler!({ payload: { type: "leave" } });
    expect(app.dragging).toBe(false);
    await dropHandler!({ payload: { type: "enter", paths: ["C:/x/Photos", "C:/x/b.ferry"] } });
    expect(app.dragKind).toBe("files");
  });
});

describe("artwork", () => {
  it("follows the saved setting", async () => {
    backend.get_settings = () => ({ downloadDir: "/d", relay: { mode: "default" }, closeToTray: true, showArtwork: false });
    const { app } = await freshStore();
    expect(app.showArtwork).toBe(false);
  });

  it("stays unknown until settings load, and init waits for them", async () => {
    let release!: () => void;
    const loaded = new Promise<void>((done) => (release = done));
    backend.get_settings = async () => {
      await loaded;
      return { downloadDir: "/d", relay: { mode: "default" }, closeToTray: true, showArtwork: false };
    };
    vi.resetModules();
    const { app, init } = await import("./store.svelte");
    let finished = false;
    const started = init().then(() => (finished = true));
    await vi.advanceTimersByTimeAsync(100);
    expect(app.showArtwork).toBeNull();
    expect(finished).toBe(false);
    release();
    await started;
    expect(app.showArtwork).toBe(false);
  });

  it("falls back to showing artwork when settings can't be read, even if startup fails", async () => {
    backend.get_settings = () => {
      throw new Error("unreadable");
    };
    backend.list_shares = () => {
      throw new Error("node down");
    };
    vi.resetModules();
    const { app, init } = await import("./store.svelte");
    await expect(init()).rejects.toThrow("node down");
    expect(app.showArtwork).toBe(true);
  });
});
