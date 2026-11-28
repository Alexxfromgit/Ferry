// Native dialogs, held open in the tray panel. A dialog takes focus from the panel, which would
// hide it (and with it the dialog's owner) the moment the dialog appears; while one is up the
// panel stays. Every desktop dialog holds, so it can't depend on the page knowing it's a panel:
// a regular window ignores the hold.

import * as dialog from "@tauri-apps/plugin-dialog";
import { api } from "./api";
import { isMobile } from "./store.svelte";

async function held<T>(show: () => Promise<T>): Promise<T> {
  if (isMobile()) return show();
  await api.panelHold(true).catch(() => {});
  try {
    return await show();
  } finally {
    void api.panelHold(false).catch(() => {});
  }
}

export function open<T extends dialog.OpenDialogOptions>(options?: T) {
  return held(() => dialog.open(options));
}

export function save(options?: dialog.SaveDialogOptions) {
  return held(() => dialog.save(options));
}

export function ask(text: string, options?: string | dialog.ConfirmDialogOptions) {
  return held(() => dialog.ask(text, options));
}
