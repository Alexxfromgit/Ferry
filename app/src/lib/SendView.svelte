<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import { app, setShareLayout, share } from "./store.svelte";
  import { bytes, exactCount } from "./format";
  import ShareStrip from "./ShareStrip.svelte";
  import ShareTicket from "./ShareTicket.svelte";

  /** The share opened from the compact list, if any (one at a time). */
  let expanded = $state<string | null>(null);

  // A share that couldn't be turned back on opens, so its "Share again" is in view.
  let knownProblems = new Set<string>();
  $effect(() => {
    const ids = Object.keys(app.shareProblems);
    const fresh = ids.find((id) => !knownProblems.has(id));
    knownProblems = new Set(ids);
    if (fresh && app.shareLayout === "compact") expanded = fresh;
  });

  function toggle(id: string) {
    expanded = expanded === id ? null : id;
  }

  function closeOnEscape(e: KeyboardEvent) {
    if (e.key === "Escape" && expanded && !(e.target as Element | null)?.closest("dialog")) {
      const id = expanded;
      expanded = null;
      // Return focus to the strip that was opened.
      queueMicrotask(() =>
        document.querySelector<HTMLElement>(`[data-share="${id}"] .open-btn`)?.focus(),
      );
    }
  }

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
  <div class="list-head">
    <h2>
      Shared now
      {#if app.shares.length > 0}<span class="h-count">{exactCount(app.shares.length)}</span>{/if}
    </h2>
    {#if app.shares.length > 0}
      <div class="layout" role="group" aria-label="List layout">
        <button
          class="quiet"
          aria-pressed={app.shareLayout === "tickets"}
          title="Full tickets"
          onclick={() => setShareLayout("tickets")}
        >
          <svg viewBox="0 0 20 20" width="16" height="16" aria-hidden="true">
            <g fill="none" stroke="currentColor" stroke-width="1.6">
              <rect x="2.5" y="3" width="15" height="6" rx="1.5" />
              <rect x="2.5" y="11" width="15" height="6" rx="1.5" />
            </g>
          </svg>
          Tickets
        </button>
        <button
          class="quiet"
          aria-pressed={app.shareLayout === "compact"}
          title="One line per ticket"
          onclick={() => setShareLayout("compact")}
        >
          <svg viewBox="0 0 20 20" width="16" height="16" aria-hidden="true">
            <g fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
              <line x1="3" y1="5" x2="17" y2="5" />
              <line x1="3" y1="10" x2="17" y2="10" />
              <line x1="3" y1="15" x2="17" y2="15" />
            </g>
          </svg>
          Compact
        </button>
      </div>
    {/if}
  </div>
  {#if app.shares.length === 0}
    <!-- With the artwork on, the mascot behind the page is this screen's illustration. -->
    <div class="empty" class:glass={app.showArtwork === false} class:bare={app.showArtwork !== false}>
      {#if app.showArtwork === false}
        <img src="/ferry-icon.png" alt="" width="96" height="96" />
      {/if}
      <p class="muted">
        Nothing is shared yet. Your first ticket will show up here, with a link to copy and a QR
        code for phones.
      </p>
    </div>
  {:else if app.shareLayout === "compact"}
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <ul class="strips" aria-label="Tickets" onkeydown={closeOnEscape}>
      {#each app.shares as s (s.id)}
        <li data-share={s.id} class:open={expanded === s.id}>
          <div class="strip-wrap">
            <ShareStrip share={s} activity={app.activity[s.id]} open={expanded === s.id} ontoggle={() => toggle(s.id)} />
          </div>
          {#if expanded === s.id}
            <div class="opened">
              <ShareTicket share={s} activity={app.activity[s.id]} />
            </div>
          {/if}
        </li>
      {/each}
    </ul>
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

  .list-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    max-width: 800px;
    margin-bottom: 1rem;
  }

  .layout {
    display: inline-flex;
    gap: 0.15rem;
    padding: 0.15rem;
    border-radius: 11px;
    border: 1px solid var(--glass-edge);
    background: var(--glass);
  }

  .layout button {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.3rem 0.7rem;
    border-radius: 8px;
    font-size: var(--t-xs);
  }

  .layout button[aria-pressed="true"] {
    color: var(--ink);
    background: var(--magenta-soft);
    border-color: rgb(255 45 149 / 0.35);
  }

  .strips {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 0.5rem;
    max-width: 800px;
  }

  .strip-wrap {
    filter: drop-shadow(0 6px 14px rgb(0 0 0 / 0.45));
  }

  li.open .strip-wrap {
    filter: drop-shadow(0 6px 14px rgb(0 0 0 / 0.45)) drop-shadow(0 0 14px rgb(255 45 149 / 0.25));
  }

  .opened {
    margin: 0.6rem 0 0.9rem;
  }

  .empty {
    display: flex;
    align-items: center;
    gap: 1.2rem;
    max-width: 620px;
    padding: 1rem 1.2rem;
    border-radius: 14px;
  }

  .empty.bare {
    max-width: 44ch;
    padding: 0;
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
