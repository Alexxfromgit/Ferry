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
  ...extra,
});
const download = (id: string, extra: Partial<DownloadInfo> = {}): DownloadInfo => ({
  ...sampleDownloads[0],
  id,
  ...extra,
});
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
    emit({ type: "shareActivity", shareId: "s1", activePeers: 1, uploadedBytes: 5 });
    emit({ type: "shareActivity", shareId: "ghost", activePeers: 1, uploadedBytes: 5 });
    expect(Object.keys(app.activity)).toEqual(["s1"]);
    emit({ type: "shareRemoved", shareId: "s1" });
    expect(app.shares).toEqual([]);
    expect(app.activity).toEqual({});
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
    backend.take_pending_links = () => ['{"ticket": "blob"}'];
    backend.preview = () => Promise.resolve(preview("from file"));
    handlers["ferry-open-link"]({ payload: null });
    await vi.waitFor(() => expect(receive.preview?.name).toBe("from file"));
    expect(app.view).toBe("receive");
    expect(receive.link, "JSON from a .ferry file is not shown in the link field").toBe("");
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
    backend.read_share_file = () => '{"ticket": "blob"}';
    backend.preview = () => Promise.resolve(preview("dropped"));
    await dropHandler!({ payload: { type: "drop", paths: ["C:/x/trip.ferry"] } });
    expect(app.view).toBe("receive");
    expect(receive.preview?.name).toBe("dropped");

    backend.create_share = () => share("s2", { name: "Photos" });
    await dropHandler!({ payload: { type: "drop", paths: ["C:/x/Photos", "C:/x/trip.ferry"] } });
    expect(app.view).toBe("send");
    expect(invoke).toHaveBeenCalledWith("create_share", { paths: ["C:/x/Photos", "C:/x/trip.ferry"] });
  });
});
