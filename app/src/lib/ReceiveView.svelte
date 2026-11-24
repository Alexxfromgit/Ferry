<script lang="ts">
  import { onMount } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { api, message } from "./api";
  import { bytes, exactCount, plural } from "./format";
  import {
    activeDownloadCount,
    app,
    checkLink,
    clearPreview,
    isMobile,
    openTicket,
    receive,
    splashing,
    startDownload,
  } from "./store.svelte";
  import DownloadRow from "./DownloadRow.svelte";

  let settingsDir = $state("");

  onMount(() => {
    api.getSettings().then((s) => (settingsDir = s.downloadDir));
  });

  let scanning = $state(false);

  /** Scan a share QR code with the phone camera, then check the link it holds. */
  async function scanQr() {
    if (scanning) return;
    scanning = true;
    try {
      const { Format, checkPermissions, requestPermissions, scan } = await import(
        "@tauri-apps/plugin-barcode-scanner"
      );
      let permission = await checkPermissions();
      if (permission !== "granted" && permission !== "denied") permission = await requestPermissions();
      if (permission !== "granted") {
        receive.problem = {
          title: "Ferry can't use the camera.",
          detail: "Allow camera access for Ferry in your phone's settings, or paste the link instead.",
        };
        return;
      }
      const result = await scan({ windowed: false, formats: [Format.QRCode] });
      receive.link = result.content;
      await checkLink(result.content);
    } catch (err) {
      // Backing out of the scanner is not an error worth showing.
      if (!/cancel/i.test(message(err))) receive.problem = { title: "Couldn't scan the code.", detail: message(err) };
    } finally {
      scanning = false;
    }
  }

  async function openTicketFile() {
    try {
      // Phone pickers don't know the .ferry extension, so they offer every file.
      const filters = isMobile() ? [] : [{ name: "Ferry ticket", extensions: ["ferry"] }];
      const path = await open({ filters });
      if (typeof path === "string") await openTicket(path);
    } catch (err) {
      receive.problem = { title: "Couldn't open the ticket file.", detail: message(err) };
    }
  }

  /** Unlock a protected ticket file, or open a share whose sender asks for a password. */
  function unlock(event: SubmitEvent & { currentTarget: HTMLFormElement }) {
    event.preventDefault();
    if (receive.openingTicket || receive.checking) return;
    const password = new FormData(event.currentTarget).get("password");
    event.currentTarget.reset();
    if (typeof password !== "string" || !password) return;
    if (receive.protectedTicket) void openTicket(receive.protectedTicket, password);
    else if (receive.lockedLink) void checkLink(receive.lockedLink, password);
  }

  // The password field takes focus when it appears. A protected ticket opened at launch
  // mounts it behind the startup splash, where the UI is inert and focus can't land, so it
  // waits for the splash to go.
  let passwordInput = $state<HTMLInputElement | null>(null);
  $effect(() => {
    if (passwordInput && !splashing()) passwordInput.focus();
  });

  async function chooseDest() {
    const dir = await open({ directory: true, defaultPath: receive.dest ?? settingsDir });
    if (typeof dir === "string") receive.dest = dir;
  }

  const shown = $derived(receive.preview ? receive.preview.files.slice(0, 8) : []);
</script>

<section class="intake" aria-label="Open a ticket">
  <h1>Receive files someone sent you</h1>
  <p class="muted lede">
    {isMobile()
      ? "Scan the QR code on the sender's screen, paste the link you were sent, or open the ticket file."
      : "Paste the Ferry link you were sent, or open the ticket file."}
  </p>

  {#if isMobile()}
    <button class="primary scan" onclick={scanQr} disabled={scanning || receive.checking}>
      {scanning ? "Opening camera…" : "Scan QR code"}
    </button>
  {/if}

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
      placeholder="Paste a Ferry link"
      bind:value={receive.link}
      spellcheck="false"
      autocomplete="off"
    />
    <button class:primary={!receive.preview} type="submit" disabled={receive.checking || !receive.link.trim()}>
      Check link
    </button>
    <button type="button" onclick={openTicketFile} disabled={receive.checking || receive.openingTicket}>
      Open ticket file
    </button>
  </form>

  {#if receive.protectedTicket || receive.lockedLink}
    {#key receive.protectedTicket ?? receive.lockedLink}
      <form class="unlock glass" onsubmit={unlock}>
        <label for="ticket-password">
          {receive.protectedTicket
            ? `Password for ${receive.protectedTicket.split(/[\\/]/).at(-1)}`
            : "This share is protected. Enter the password the sender gave you."}
        </label>
        <div class="paste">
          <input id="ticket-password" name="password" type="password" autocomplete="off" bind:this={passwordInput} required />
          <button class="primary" type="submit" disabled={receive.openingTicket || receive.checking}>
            {receive.protectedTicket ? "Unlock ticket" : "Open share"}
          </button>
          <button class="quiet" type="button" onclick={clearPreview}>Cancel</button>
        </div>
      </form>
    {/key}
  {/if}

  {#if receive.checking}
    <p class="muted status" aria-live="polite">Contacting the sender…</p>
  {:else if receive.openingTicket}
    <p class="muted status" aria-live="polite">{receive.protectedTicket ? "Unlocking ticket…" : "Opening ticket…"}</p>
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
      {#if !isMobile()}
        <div class="dest">
          <span class="muted">Save to</span>
          <span class="code path" title={receive.dest ?? settingsDir}>{receive.dest ?? settingsDir}</span>
          <button class="quiet" onclick={chooseDest}>Change</button>
        </div>
      {/if}
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
  <h2>
    Downloads
    {#if activeDownloadCount() > 0}<span class="h-count">{exactCount(activeDownloadCount())} in progress</span>{/if}
  </h2>
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

  .scan {
    width: 100%;
    min-height: 3.25rem;
    margin-bottom: 0.75rem;
    font-size: 1.05rem;
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

  .unlock {
    display: grid;
    gap: 0.6rem;
    margin-top: 1rem;
    padding: 0.85rem 1.1rem;
    border-radius: 12px;
    overflow-wrap: anywhere;
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
