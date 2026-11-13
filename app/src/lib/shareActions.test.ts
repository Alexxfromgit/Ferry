// Tests for turning sharing on and off, with the Tauri backend mocked.
import { beforeEach, describe, expect, it, vi } from "vitest";
import { shares as sampleShares } from "./contract.generated";
import type { ShareInfo } from "./types";

const invoke = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invoke(...args) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: async () => () => {} }));
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({ onDragDropEvent: async () => () => {} }),
}));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ ask: vi.fn(async () => true) }));

const { app } = await import("./store.svelte");
const { setSharing } = await import("./shareActions");

const share = (extra: Partial<ShareInfo> = {}): ShareInfo => ({
  ...sampleShares[0],
  id: "s1",
  name: "Holiday",
  paused: false,
  ...extra,
});

/** Answer `set_share_paused` with `error`, and `get_share` with `now`. */
function backend(error: string, now: ShareInfo | undefined) {
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === "set_share_paused") throw error;
    if (cmd === "get_share") {
      if (!now) throw "unknown share";
      return now;
    }
  });
}

describe("setSharing", () => {
  beforeEach(() => {
    invoke.mockReset();
    app.shares = [share()];
    app.activity = {};
    app.shareProblems = {};
    app.toast = null;
  });

  it("shows the share as off when turning it off worked but couldn't be saved", async () => {
    const error = "sharing is off now, but Ferry couldn't save that: disk full";
    backend(error, share({ paused: true }));
    await setSharing(app.shares[0], false);
    expect(app.shares[0].paused).toBe(true);
    expect(app.toast).toEqual({ text: "Sharing is off now, but Ferry couldn't save that: disk full", error: true });
  });

  it("reports a failure to turn off when the share is still on", async () => {
    backend("something broke", share());
    await setSharing(app.shares[0], false);
    expect(app.shares[0].paused).toBe(false);
    expect(app.toast).toEqual({ text: "Couldn't turn sharing off: something broke", error: true });
  });

  it("keeps the reason a share couldn't be turned on", async () => {
    app.shares = [share({ paused: true })];
    backend("Holiday/a.jpg was moved", share({ paused: true }));
    await setSharing(app.shares[0], true);
    expect(app.shareProblems).toEqual({ s1: "Holiday/a.jpg was moved" });
  });

  it("says nothing when the share was removed meanwhile", async () => {
    backend("this ticket was removed", undefined);
    await setSharing(app.shares[0], false);
    expect(app.toast).toBeNull();
  });
});
