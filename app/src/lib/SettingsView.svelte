<script lang="ts">
  import { onMount } from "svelte";
  import { getVersion } from "@tauri-apps/api/app";
  import { writeText } from "@tauri-apps/plugin-clipboard-manager";
  import { open } from "@tauri-apps/plugin-dialog";
  import { api, message } from "./api";
  import { app, toast } from "./store.svelte";
  import type { Settings } from "./types";

  let settings = $state<Settings | null>(null);
  let relayMode = $state<"default" | "custom" | "disabled">("default");
  let relayUrl = $state("");
  /** True while the saved relay differs from the one Ferry is running with. */
  let restartNeeded = $state(false);
  let version = $state("");

  onMount(() => {
    getVersion()
      .then((v) => (version = v))
      .catch(() => {});
    api.getSettings().then((s) => {
      settings = s;
      relayMode = s.relay.mode;
      relayUrl = s.relay.mode === "custom" ? s.relay.url : "";
    });
  });

  async function persist() {
    if (!settings) return;
    const next: Settings = {
      ...settings,
      relay:
        relayMode === "custom" ? { mode: "custom", url: relayUrl.trim() } : { mode: relayMode },
    };
    try {
      restartNeeded = await api.setSettings(next);
      settings = next;
      app.showArtwork = next.showArtwork;
      toast("Settings saved");
    } catch (err) {
      toast(message(err), true);
    }
  }

  async function chooseDownloadDir() {
    if (!settings) return;
    const dir = await open({ directory: true, defaultPath: settings.downloadDir });
    if (typeof dir === "string") {
      settings.downloadDir = dir;
      await persist();
    }
  }

  async function openLink(open: () => Promise<void>) {
    try {
      await open();
    } catch (err) {
      toast(message(err), true);
    }
  }

  async function copyId() {
    if (app.status) {
      await writeText(app.status.nodeId);
      toast("Device ID copied");
    }
  }
</script>

<h1>Settings</h1>

{#if settings}
  <div class="groups">
    <section class="glass card">
      <h2>Downloads</h2>
      <div class="field">
        <span class="label">Save received files to</span>
        <div class="value">
          <span class="code path" title={settings.downloadDir}>{settings.downloadDir}</span>
          <button onclick={chooseDownloadDir}>Change</button>
        </div>
      </div>
    </section>

    <section class="glass card">
      <h2>Window</h2>
      <label class="check">
        <input type="checkbox" bind:checked={settings.closeToTray} onchange={persist} />
        <span>
          Keep sharing when the window is closed
          <span class="muted hint">Ferry stays in the system tray. Use Quit Ferry from the tray to stop.</span>
        </span>
      </label>
    </section>

    <section class="glass card">
      <h2>Appearance</h2>
      <label class="check">
        <input type="checkbox" bind:checked={settings.showArtwork} onchange={persist} />
        <span>
          Show artwork
          <span class="muted hint">The Ferry mascot behind the app. Turn it off for a plainer look, e.g. when sharing your screen.</span>
        </span>
      </label>
    </section>

    <section class="glass card">
      <h2>Connections</h2>
      <p class="muted hint block">
        Ferry connects directly whenever the networks allow it. When they don't, an encrypted relay
        passes the data along; relays can't read your files.
      </p>
      <fieldset>
        <legend class="sr">Relay servers</legend>
        <label class="radio">
          <input type="radio" name="relay" value="default" bind:group={relayMode} onchange={persist} />
          <span>Public relays <span class="muted hint">Free relays run by the iroh project. No setup.</span></span>
        </label>
        <label class="radio">
          <input type="radio" name="relay" value="custom" bind:group={relayMode} />
          <span>My own relay <span class="muted hint">A self-hosted iroh-relay, e.g. on a small VPS.</span></span>
        </label>
        {#if relayMode === "custom"}
          <form
            class="relay-url"
            onsubmit={(e) => {
              e.preventDefault();
              void persist();
            }}
          >
            <input type="url" class="code" placeholder="https://relay.example.com" bind:value={relayUrl} />
            <button type="submit" disabled={!relayUrl.trim()}>Save</button>
          </form>
        {/if}
        <label class="radio">
          <input type="radio" name="relay" value="disabled" bind:group={relayMode} onchange={persist} />
          <span>No relays <span class="muted hint">Only direct connections. Many people abroad won't reach you.</span></span>
        </label>
      </fieldset>
      {#if restartNeeded}
        <p class="restart">Restart Ferry to use the new relay setting.</p>
      {/if}
    </section>

    <section class="glass card">
      <h2>This device</h2>
      <div class="field">
        <span class="label">Device ID</span>
        <div class="value">
          <span class="code path" title={app.status?.nodeId}>{app.status?.nodeId ?? "…"}</span>
          <button onclick={copyId} disabled={!app.status}>Copy</button>
        </div>
      </div>
      <p class="muted hint block">
        Your links contain this ID. It stays the same across restarts, so tickets you already sent
        keep working.
      </p>
      {#if app.status?.relayUrl}
        <p class="muted hint block">Home relay: <span class="code">{app.status.relayUrl}</span></p>
      {/if}
    </section>

    <section class="glass card about" class:artwork={app.showArtwork} aria-labelledby="about-title">
      {#if app.showArtwork}
        <div class="banner" aria-hidden="true"><img src="/art/scene-square.webp" alt="" /></div>
      {:else}
        <img class="icon" src="/ferry-icon.png" alt="" width="56" height="56" />
      {/if}
      <div class="about-text">
        <h2 id="about-title">Ferry {#if version}<span class="h-count">{version}</span>{/if}</h2>
        <p class="muted hint">
          Files go straight from one computer to another, end-to-end encrypted. Free and open
          source under the MIT license.
        </p>
        <div class="links">
          <button onclick={() => openLink(api.openProjectPage)}>Source code</button>
          <button class="quiet" onclick={() => openLink(api.openThirdPartyNotices)}>Third-party notices</button>
        </div>
      </div>
    </section>
  </div>
{/if}

<style>
  h1 {
    margin-bottom: 1.75rem;
  }

  .groups {
    display: grid;
    gap: 1rem;
    max-width: 660px;
  }

  .card {
    padding: 1rem 1.2rem 1.1rem;
    border-radius: 14px;
  }

  h2 {
    font-size: var(--t-md);
    margin-bottom: 0.6rem;
  }

  .field {
    display: grid;
    gap: 0.3rem;
  }

  .label {
    font-size: var(--t-sm);
    color: var(--ink-soft);
  }

  .value {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }

  .path {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    background: rgb(0 0 0 / 0.35);
    border: 1px solid var(--glass-edge);
    border-radius: 8px;
    padding: 0.45rem 0.7rem;
    font-size: var(--t-sm);
  }

  .hint {
    display: block;
    font-size: var(--t-sm);
  }

  .block {
    margin-bottom: 0.75rem;
  }

  fieldset {
    border: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: 0.65rem;
  }

  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
  }

  .check,
  .radio {
    display: flex;
    gap: 0.6rem;
    align-items: flex-start;
    cursor: pointer;
  }

  .check input,
  .radio input {
    margin-top: 0.3rem;
    accent-color: var(--magenta);
  }

  .relay-url {
    display: flex;
    gap: 0.5rem;
    margin-left: 1.6rem;
  }

  .about {
    display: flex;
    align-items: center;
    gap: 1.1rem;
    overflow: hidden;
  }

  /* The mascot fills the card's left side and fades into it. */
  .about.artwork {
    padding: 0 1.2rem 0 0;
    gap: 0.4rem;
  }

  /* A close crop of her: the square scene scaled up, head near the top of the card. */
  .banner {
    position: relative;
    flex: none;
    align-self: stretch;
    width: 190px;
    min-height: 168px;
    overflow: hidden;
    -webkit-mask-image: linear-gradient(to right, #000 60%, transparent);
    mask-image: linear-gradient(to right, #000 60%, transparent);
  }

  .banner img {
    position: absolute;
    width: 380px;
    left: -12px;
    top: -88px;
  }

  .about .icon {
    flex: none;
    border-radius: 14px;
    box-shadow: 0 0 0 1px rgb(255 45 149 / 0.4), 0 0 22px -4px var(--magenta-glow);
  }

  .about-text {
    padding: 1rem 0;
  }

  .links {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
    margin-top: 0.75rem;
  }

  .restart {
    margin-top: 0.75rem;
    font-weight: 650;
    color: var(--danger);
  }
</style>
