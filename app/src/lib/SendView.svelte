<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import { app, share } from "./store.svelte";
  import { bytes, exactCount } from "./format";
  import ShareTicket from "./ShareTicket.svelte";

  async function pick(directory: boolean) {
    const picked = await open({ multiple: true, directory });
    if (!picked) return;
    await share(Array.isArray(picked) ? picked : [picked]);
  }

  const importPercent = $derived(
    app.importing && app.importing.totalBytes > 0
      ? (app.importing.doneBytes / app.importing.totalBytes) * 100
      : 0,
  );
</script>

<section class="dock" aria-label="Share files">
  <h1>Send files straight from this computer</h1>
  <p class="muted lede">
    Drop files or folders anywhere in this window, or choose them below. Ferry makes a ticket you
    can send to anyone; they download directly from you while Ferry is running.
  </p>

  <div class="pier glass">
  {#if app.importing}
    <div class="importing" aria-live="polite">
      <p>
        Preparing the ticket…
        {#if app.importing.totalBytes > 0}
          <span class="muted">{bytes(app.importing.doneBytes)} of {bytes(app.importing.totalBytes)}</span>
        {/if}
      </p>
      <div class="bar"><span style:width="{importPercent}%"></span></div>
      {#if app.importing.currentFile}
        <p class="muted file code">{app.importing.currentFile}</p>
      {/if}
    </div>
  {:else}
    <div class="actions">
      <button class="primary" onclick={() => pick(false)}>Choose files</button>
      <button onclick={() => pick(true)}>Choose a folder</button>
      <span class="muted hint">or drop them here</span>
    </div>
  {/if}
  </div>
</section>

<section aria-label="Your shares">
  <h2>
    Shared now
    {#if app.shares.length > 0}<span class="h-count">{exactCount(app.shares.length)}</span>{/if}
  </h2>
  {#if app.shares.length === 0}
    <div class="empty glass">
      <img src="/ferry-icon.png" alt="" width="96" height="96" />
      <p class="muted">
        Nothing is shared yet. Your first ticket will show up here, with a link to copy and a QR
        code for phones.
      </p>
    </div>
  {:else}
    <div class="tickets">
      {#each app.shares as s (s.id)}
        <ShareTicket share={s} activity={app.activity[s.id]} />
      {/each}
    </div>
  {/if}
</section>

<style>
  .dock {
    max-width: 62ch;
    margin-bottom: 2.75rem;
  }

  .lede {
    margin: 0.6rem 0 1.25rem;
  }

  .pier {
    border-radius: 14px;
    padding: 1.1rem 1.2rem;
    border-style: dashed;
    border-color: rgb(255 45 149 / 0.32);
  }

  .actions {
    display: flex;
    align-items: center;
    gap: 0.6rem;
  }

  .hint {
    font-size: var(--t-sm);
    margin-left: 0.4rem;
  }

  .importing {
    display: grid;
    gap: 0.4rem;
  }

  .file {
    font-size: var(--t-xs);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  h2 {
    margin-bottom: 1rem;
  }

  .empty {
    display: flex;
    align-items: center;
    gap: 1.2rem;
    max-width: 620px;
    padding: 1rem 1.2rem;
    border-radius: 14px;
  }

  .empty img {
    flex: none;
    border-radius: 14px;
    box-shadow: 0 0 0 1px rgb(255 45 149 / 0.4), 0 0 26px -4px var(--magenta-glow);
  }

  .tickets {
    display: grid;
    gap: 1.4rem;
    max-width: 800px;
  }
</style>
