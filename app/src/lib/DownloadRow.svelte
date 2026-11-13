<script lang="ts">
  import { ask } from "@tauri-apps/plugin-dialog";
  import { api, message } from "./api";
  import { bytes, duration, shortId } from "./format";
  import { toast } from "./store.svelte";
  import type { DownloadInfo } from "./types";

  let { download }: { download: DownloadInfo } = $props();

  const percent = $derived(
    download.totalBytes > 0 ? Math.min(100, (download.doneBytes / download.totalBytes) * 100) : 0,
  );

  const title = $derived(download.name || `Share ${shortId(download.id)}`);

  const detail = $derived.by(() => {
    const d = download;
    switch (d.status) {
      case "connecting":
        return "Contacting the sender…";
      case "downloading": {
        const parts = [`${bytes(d.doneBytes)} of ${bytes(d.totalBytes)}`];
        if (d.bytesPerSec > 0) parts.push(`${bytes(d.bytesPerSec)}/s`);
        if (d.etaSecs != null) parts.push(duration(d.etaSecs));
        return parts.join(", ");
      }
      case "exporting":
        return "Saving files…";
      case "completed":
        return `${bytes(d.totalBytes)}, saved`;
      case "paused":
        return d.totalBytes > 0 ? `Paused at ${bytes(d.doneBytes)} of ${bytes(d.totalBytes)}` : "Paused";
      case "failed":
        return d.error ?? "Stopped with an error";
    }
  });

  const showBar = $derived(["downloading", "paused", "exporting"].includes(download.status));

  async function act(fn: () => Promise<void>, failure: string) {
    try {
      await fn();
    } catch (err) {
      toast(`${failure}: ${message(err)}`, true);
    }
  }

  async function remove() {
    const finished = download.status === "completed";
    if (!finished) {
      const ok = await ask("The part downloaded so far will be discarded.", {
        title: `Remove ${title}?`,
        kind: "warning",
        okLabel: "Remove",
        cancelLabel: "Keep",
      });
      if (!ok) return;
    }
    await act(() => api.removeDownload(download.id), "Couldn't remove the download");
  }
</script>

<li class="row" data-status={download.status}>
  <div class="main">
    <div class="top">
      <strong class="name" {title}>{title}</strong>
      {#if download.connection && download.status === "downloading"}
        <span
          class="via" data-kind={download.connection}
          title={download.connection === "direct"
            ? "Data flows straight between you and the sender."
            : "No direct path through the networks in between, so data passes an encrypted relay. Slower, but private."}
        >
          {download.connection === "direct" ? "Direct" : "Via relay"}
        </span>
      {/if}
    </div>
    {#if showBar}
      <div class="bar"><span style:width="{percent}%"></span></div>
    {/if}
    <p class="detail" class:error={download.status === "failed"}>{detail}</p>
  </div>

  <div class="actions">
    {#if download.status === "downloading" || download.status === "connecting"}
      <button onclick={() => act(() => api.pauseDownload(download.id), "Couldn't pause")}>Pause</button>
    {:else if download.status === "paused" || download.status === "failed"}
      <button onclick={() => act(() => api.resumeDownload(download.id), "Couldn't resume")}>
        {download.status === "failed" ? "Try again" : "Resume"}
      </button>
    {:else if download.status === "completed" && download.savedPath}
      <button onclick={() => act(() => api.revealDownload(download.id), "Couldn't open the folder")}>
        Show in folder
      </button>
    {/if}
    <button class="quiet danger" onclick={remove} disabled={download.status === "exporting"}>
      {download.status === "completed" ? "Clear" : "Remove"}
    </button>
  </div>
</li>

<style>
  .row {
    display: flex;
    gap: 1.25rem;
    align-items: center;
    padding: 0.9rem 0;
    border-bottom: 1px solid var(--glass-edge);
  }

  .row:last-child {
    border-bottom: none;
  }

  .main {
    flex: 1;
    min-width: 0;
    display: grid;
    gap: 0.35rem;
  }

  .top {
    display: flex;
    align-items: baseline;
    gap: 0.75rem;
  }

  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .via {
    flex: none;
    font-size: var(--t-xs);
    font-weight: 650;
    padding: 0 0.5rem;
    border-radius: 999px;
    color: var(--magenta);
    background: var(--magenta-soft);
  }

  .via[data-kind="relay"] {
    color: var(--relay);
    background: rgb(183 156 255 / 0.14);
  }

  .detail {
    font-size: var(--t-sm);
    color: var(--ink-soft);
  }

  .detail.error {
    color: var(--danger);
  }

  .actions {
    display: flex;
    gap: 0.4rem;
    flex: none;
  }

  [data-status="paused"] .bar > :global(span) {
    background: var(--ink-soft);
  }
</style>
