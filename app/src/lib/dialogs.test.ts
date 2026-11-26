// Tests that native dialogs hold the Windows tray panel open, and always let it go again.
import { beforeEach, describe, expect, it, vi } from "vitest";

const calls: string[] = [];
let panel = false;
let dialogResult: () => Promise<unknown>;

vi.mock("@tauri-apps/plugin-dialog", () => {
  const show = (name: string) => () => {
    calls.push(name);
    return dialogResult();
  };
  return { open: show("open"), save: show("save"), ask: show("ask") };
});
vi.mock("./api", () => ({
  api: {
    panelHold: async (held: boolean) => {
      calls.push(held ? "hold" : "release");
    },
  },
}));
vi.mock("./store.svelte", () => ({ isPanel: () => panel }));

const dialogs = await import("./dialogs");

beforeEach(() => {
  calls.length = 0;
  panel = true;
  dialogResult = async () => "C:\\picked";
});

describe("native dialogs in the tray panel", () => {
  it("hold the panel open while each dialog is up", async () => {
    expect(await dialogs.open({ multiple: true })).toBe("C:\\picked");
    await dialogs.save();
    await dialogs.ask("Remove it?");
    expect(calls).toEqual(["hold", "open", "release", "hold", "save", "release", "hold", "ask", "release"]);
  });

  it("let it go when a dialog fails", async () => {
    dialogResult = async () => {
      throw new Error("no dialog");
    };
    await expect(dialogs.open()).rejects.toThrow("no dialog");
    expect(calls).toEqual(["hold", "open", "release"]);
  });

  it("leave a regular window alone", async () => {
    panel = false;
    await dialogs.open();
    expect(calls).toEqual(["open"]);
  });
});
