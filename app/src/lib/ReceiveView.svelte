<script lang="ts">
  import { onMount } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { api, message } from "./api";
  import { bytes, plural } from "./format";
  import { app, checkLink, clearPreview, receive, startDownload } from "./store.svelte";
  import DownloadRow from "./DownloadRow.svelte";

  let settingsDir = $state("");

  onMount(() => {
    api.getSettings().then((s) => (settingsDir = s.downloadDir));
  });

  async function openTicketFile() {
    const path = await open({ filters: [{ name: "Ferry ticket", extensions: ["ferry"] }] });
    if (typeof path !== "string") return;
    try {
      const text = await api.readShareFile(path);
      receive.link = "";
      await checkLink(text);
    } catch (err) {
      receive.problem = { title: "Couldn't open the ticket file.", detail: message(err) };
    }
  }

  async function chooseDest() {
    const dir = await open({ directory: true, defaultPath: receive.dest ?? settingsDir });
    if (typeof dir === "string") receive.dest = dir;
  }

  const shown = $derived(receive.preview ? receive.preview.files.slice(0, 8) : []);
</script>

<section class="intake" aria-label="Open a ticket">
  <h1>Receive files someone sent you</h1>
  <p class="muted lede">Paste the Ferry link you were sent, or open the ticket file.</p>

  <form
    class="paste"
    onsubmit={(e) => {
      e.preventDefault();
      void checkLink(receive.link);
    }}
  >
    <label class="sr" for="link">Ferry link</label>
    <input
      id="link"
      type="text"
      class="code"
      placeholder="ferry://get?t=…"
      bind:value={receive.link}
      spellcheck="false"
      autocomplete="off"
    />
    <button class:primary={!receive.preview} type="submit" disabled={receive.checking || !receive.link.trim()}>
      Check link
    </button>
    <button type="button" onclick={openTicketFile} disabled={receive.checking}>Open ticket file</button>
  </form>

  {#if receive.checking}
    <p class="muted status" aria-live="polite">Contacting the sender…</p>
  {:else if receive.problem}
    <div class="problem glass" role="alert">
      <p><strong>{receive.problem.title}</strong></p>
      <p class="muted">{receive.problem.detail}</p>
    </div>
  {/if}

  {#if receive.preview}
    {@const preview = receive.preview}
    <div class="preview glass">
      <header>
        <h2>{preview.name}</h2>
        <span>{bytes(preview.totalSize)}</span>
      </header>
      <ul class="files">
        {#each shown as f, i (i)}
          <li><span class="code name">{f.name}</span><span class="muted">{bytes(f.size)}</span></li>
        {/each}
        {#if preview.fileCount > shown.length}
          <li class="muted">and {plural(preview.fileCount - shown.length, "more file")}</li>
        {/if}
      </ul>
      <div class="dest">
        <span class="muted">Save to</span>
        <span class="code path" title={receive.dest ?? settingsDir}>{receive.dest ?? settingsDir}</span>
        <button class="quiet" onclick={chooseDest}>Change</button>
      </div>
      <div class="go">
        <button class="primary" onclick={startDownload} disabled={receive.starting}>
          {receive.starting ? "Starting…" : "Download"}
        </button>
        <button class="quiet" onclick={clearPreview}>Cancel</button>
        {#if preview.connection}
          <span class="muted conn">
            {preview.connection === "direct" ? "Direct connection to the sender" : "Connected through a relay"}
          </span>
        {/if}
      </div>
    </div>
  {/if}
</section>

<section aria-label="Downloads">
  <h2>Downloads</h2>
  {#if app.downloads.length === 0}
    <p class="muted">Files you receive appear here, with their progress.</p>
  {:else}
    <ul class="downloads glass">
      {#each app.downloads as d (d.id)}
        <DownloadRow download={d} />
      {/each}
    </ul>
  {/if}
</section>

<style>
  .intake {
    max-width: 740px;
    margin-bottom: 2.75rem;
  }

  .lede {
    margin: 0.6rem 0 1.25rem;
  }

  .paste {
    display: flex;
    gap: 0.5rem;
  }

  .paste input {
    flex: 1;
    min-width: 0;
  }

  .paste button {
    flex: none;
  }

  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
  }

  .status {
    margin-top: 1rem;
  }

  .problem {
    margin-top: 1rem;
    padding: 0.85rem 1.1rem;
    border-radius: 12px;
    border-color: rgb(255 107 107 / 0.45);
    box-shadow: inset 3px 0 0 var(--danger);
    display: grid;
    gap: 0.25rem;
  }

  .preview {
    margin-top: 1.25rem;
    padding: 1.15rem 1.4rem;
    border-radius: 14px;
    border-color: rgb(255 45 149 / 0.3);
  }

  .preview header {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: 1rem;
    font-weight: 650;
  }

  .files {
    list-style: none;
    margin: 0.8rem 0;
    padding: 0;
    display: grid;
    gap: 0.25rem;
    font-size: var(--t-sm);
  }

  .files li {
    display: flex;
    justify-content: space-between;
    gap: 1rem;
  }

  .name,
  .path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }

  .dest {
    display: flex;
    align-items: baseline;
    gap: 0.6rem;
    padding: 0.65rem 0;
    border-top: 1px solid var(--glass-edge);
    font-size: var(--t-sm);
  }

  .dest .path {
    flex: 1;
  }

  .go {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin-top: 0.4rem;
  }

  .conn {
    margin-left: auto;
    font-size: var(--t-xs);
  }

  h2 {
    margin-bottom: 1rem;
  }

  .downloads {
    list-style: none;
    margin: 0;
    padding: 0.2rem 1.2rem;
    max-width: 800px;
    border-radius: 14px;
  }

  @media (max-width: 640px) {
    .paste {
      flex-wrap: wrap;
    }
    .paste input {
      flex-basis: 100%;
    }
  }
</style>
