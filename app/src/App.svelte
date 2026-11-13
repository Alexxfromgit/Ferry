<script lang="ts">
  import { onMount, tick } from "svelte";
  import { fade } from "svelte/transition";
  import { activeDownloadCount, app, init, receive, toast, type View } from "./lib/store.svelte";
  import { compactCount, exactCount } from "./lib/format";
  import { message } from "./lib/api";
  import SendView from "./lib/SendView.svelte";
  import ReceiveView from "./lib/ReceiveView.svelte";
  import SettingsView from "./lib/SettingsView.svelte";
  import { initUpdates, updates } from "./lib/updates.svelte";
  import Scene, { type SceneMode } from "./lib/Scene.svelte";
  import Splash from "./lib/Splash.svelte";
  import { motion } from "./lib/motion";

  onMount(() => {
    init()
      .catch((err) => toast(`Ferry couldn't start: ${message(err)}`, true))
      .finally(() => (app.ready = true));
    return initUpdates();
  });

  /** The mascot is the illustration on an empty screen and recedes once content covers her. */
  const sceneMode = $derived.by<SceneMode>(() => {
    if (app.dragging) return "drop";
    if (app.view === "send") return app.shares.length === 0 && !app.importing ? "hero" : "ambient";
    if (app.view === "receive") {
      const busy = receive.preview || receive.problem || receive.protectedTicket;
      return app.downloads.length === 0 && !busy ? "hero" : "ambient";
    }
    return "ambient";
  });

  const tabs: { id: View; label: string }[] = [
    { id: "send", label: "Send" },
    { id: "receive", label: "Receive" },
    { id: "settings", label: "Settings" },
  ];

  /** What each tab's badge counts, with the words for its tooltip and screen readers. */
  const badges = $derived<Record<View, { n: number; one: string; many: string }>>({
    send: { n: app.shares.length, one: "share", many: "shares" },
    receive: { n: activeDownloadCount(), one: "download in progress", many: "downloads in progress" },
    settings: { n: 0, one: "", many: "" },
  });

  function badgeText(b: { n: number; one: string; many: string }): string {
    return `${exactCount(b.n)} ${b.n === 1 ? b.one : b.many}`;
  }

  async function viewUpdate() {
    app.view = "settings";
    await tick();
    document.getElementById("updates-heading")?.focus();
  }

  const reach = $derived.by(() => {
    if (!app.status) return { level: "off", text: "Starting…" };
    if (app.status.relayUrl) return { level: "on", text: "Reachable from anywhere" };
    if (app.status.directAddresses.length > 0)
      return { level: "local", text: "Reachable on this network only" };
    return { level: "off", text: "Offline" };
  });
</script>

<!-- Neon light streaks behind the glass, after the app icon's backdrop. -->
<svg class="backdrop" viewBox="0 0 1200 800" preserveAspectRatio="xMidYMid slice" aria-hidden="true">
  <defs>
    <filter id="glow" x="-20%" y="-20%" width="140%" height="140%">
      <feGaussianBlur stdDeviation="6" />
    </filter>
    <radialGradient id="haze" cx="78%" cy="18%" r="60%">
      <stop offset="0" stop-color="#ff2d95" stop-opacity="0.22" />
      <stop offset="1" stop-color="#ff2d95" stop-opacity="0" />
    </radialGradient>
    <radialGradient id="haze2" cx="10%" cy="95%" r="55%">
      <stop offset="0" stop-color="#7a1fd1" stop-opacity="0.18" />
      <stop offset="1" stop-color="#7a1fd1" stop-opacity="0" />
    </radialGradient>
  </defs>
  <rect width="1200" height="800" fill="url(#haze)" />
  <rect width="1200" height="800" fill="url(#haze2)" />
  <g fill="none" stroke="#ff2d95" stroke-linecap="round">
    <path d="M520 420 C 720 210, 980 150, 1260 170" stroke-width="7" opacity="0.5" filter="url(#glow)" />
    <path d="M520 420 C 720 210, 980 150, 1260 170" stroke-width="1.6" opacity="0.9" />
    <path d="M-60 690 C 220 560, 380 520, 640 560" stroke-width="6" opacity="0.32" filter="url(#glow)" />
    <path d="M-60 690 C 220 560, 380 520, 640 560" stroke-width="1.2" opacity="0.6" />
  </g>
</svg>

{#if app.showArtwork}
  <Scene mode={sceneMode} />
{/if}

<div class="shell">
  <nav class="glass" aria-label="Main">
    <div class="brand">
      <img src="/ferry-icon.png" alt="" width="40" height="40" />
      <span>Ferry</span>
    </div>

    <ul>
      {#each tabs as tab (tab.id)}
        <li>
          <button
            class="tab"
            class:active={app.view === tab.id}
            aria-current={app.view === tab.id ? "page" : undefined}
            title={badges[tab.id].n > 0 ? badgeText(badges[tab.id]) : undefined}
            onclick={() => (app.view = tab.id)}
          >
            {tab.label}
            {#if badges[tab.id].n > 0}
              <span class="count" aria-hidden="true">{compactCount(badges[tab.id].n)}</span>
              <span class="sr-only">, {badgeText(badges[tab.id])}</span>
            {/if}
          </button>
        </li>
      {/each}
    </ul>

    <p class="reach" data-level={reach.level} title={app.status?.relayUrl ?? ""}>
      <span class="dot" aria-hidden="true"></span>
      {reach.text}
    </p>
  </nav>

  <main>
    {#if updates.info?.readyVersion && app.view !== "settings"}
      <div class="update-banner glass" role="status">
        <span>Ferry {updates.info.readyVersion} is ready to install.</span>
        <button onclick={viewUpdate}>View update</button>
      </div>
    {/if}
    {#if app.view === "send"}
      <SendView />
    {:else if app.view === "receive"}
      <ReceiveView />
    {:else}
      <SettingsView />
    {/if}
  </main>

  {#if app.toast}
    <div class="toast glass" class:error={app.toast.error} role={app.toast.error ? "alert" : "status"}>{app.toast.text}</div>
  {/if}
</div>

{#if app.dragging}
  <!-- Shown while files are dragged over the window; the drop itself goes to Tauri. -->
  <div class="drop" class:artwork={app.showArtwork} transition:fade={{ duration: motion(150) }} aria-hidden="true">
    <div class="drop-words">
      <p class="drop-title">{app.dragKind === "tickets" ? "Drop to open the ticket" : "Drop to make a ticket"}</p>
      <p class="muted">
        {app.dragKind === "tickets"
          ? "Ferry shows what's inside before anything downloads."
          : "The files stay where they are. Ferry shares them from this computer."}
      </p>
    </div>
  </div>
{/if}

{#if !app.ready}
  <Splash artwork={app.showArtwork} />
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    width: 100%;
    height: 100%;
    z-index: 0;
    pointer-events: none;
  }

  .shell {
    position: relative;
    z-index: 1;
    display: grid;
    grid-template-columns: 210px 1fr;
    height: 100%;
  }

  nav {
    display: flex;
    flex-direction: column;
    gap: 1.6rem;
    padding: 1.25rem 0.9rem;
    border-width: 0 1px 0 0;
    border-radius: 0;
    box-shadow: inset -1px 0 0 rgb(255 255 255 / 0.03);
  }

  .brand {
    display: flex;
    align-items: center;
    gap: 0.65rem;
    padding: 0 0.4rem;
    font-size: var(--t-lg);
    font-weight: 800;
    letter-spacing: -0.02em;
    font-variation-settings: "MONO" 0, "CASL" 1;
  }

  .brand img {
    border-radius: 11px;
    box-shadow: 0 0 0 1px rgb(255 45 149 / 0.45), 0 0 18px -2px var(--magenta-glow);
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 0.2rem;
  }

  .tab {
    width: 100%;
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 0.6rem;
    text-align: left;
    border: 1px solid transparent;
    background: transparent;
    font-size: var(--t-md);
    font-weight: 600;
    padding: 0.55rem 0.7rem;
    color: var(--ink-soft);
  }

  .tab:hover:not(:disabled) {
    color: var(--ink);
    background: var(--glass);
    border-color: transparent;
  }

  .tab.active {
    color: var(--ink);
    background: linear-gradient(90deg, var(--magenta-soft), transparent);
    border-color: rgb(255 45 149 / 0.25);
    box-shadow: inset 3px 0 0 var(--magenta), -6px 0 18px -10px var(--magenta-glow);
  }

  .count {
    flex: none;
    min-width: 1.4rem;
    line-height: 1.5;
    font-variant-numeric: tabular-nums;
    text-align: center;
    font-size: var(--t-xs);
    font-variation-settings: var(--mono);
    color: var(--ink);
    background: rgb(255 45 149 / 0.25);
    border-radius: 999px;
    padding: 0 0.4rem;
  }

  .reach {
    margin-top: auto;
    display: flex;
    gap: 0.55rem;
    align-items: baseline;
    padding: 0 0.4rem;
    font-size: var(--t-xs);
    color: var(--ink-soft);
  }

  .dot {
    flex: none;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--ink-faint);
    transform: translateY(-1px);
  }

  .reach[data-level="on"] .dot {
    background: var(--magenta);
    box-shadow: 0 0 10px var(--magenta);
  }

  .reach[data-level="local"] .dot {
    background: var(--relay);
  }

  main {
    overflow-y: auto;
    padding: 2.2rem 2.6rem 3rem;
  }

  .update-banner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    padding: 0.6rem 0.9rem;
    margin-bottom: 1rem;
    font-size: var(--t-sm);
  }

  /* Under the scene (which lights up above it) and over everything else. */
  .drop {
    position: fixed;
    inset: 0;
    z-index: 2;
    display: grid;
    place-items: center;
    pointer-events: none;
    padding-left: 210px;
    background: radial-gradient(circle at 40% 45%, rgb(255 45 149 / 0.14), transparent 60%), rgb(8 6 11 / 0.86);
    backdrop-filter: blur(12px);
    -webkit-backdrop-filter: blur(12px);
  }

  .drop::before {
    content: "";
    position: absolute;
    inset: 14px;
    border: 2px dashed rgb(255 45 149 / 0.7);
    border-radius: 18px;
  }

  /* With the scene on the right, the words centre in the space left of her. */
  .drop.artwork {
    padding-right: min(56.3vh, 60vw);
  }

  .drop-words {
    display: grid;
    gap: 0.5rem;
    max-width: 26em;
    margin: 0 1rem;
    padding: 1.3rem 1.6rem;
    text-align: center;
    /* Readable on its own, even where the backdrop blur isn't available. */
    background: rgb(8 6 11 / 0.82);
    border: 1px solid rgb(255 45 149 / 0.3);
    border-radius: 16px;
    box-shadow: 0 0 40px -12px var(--magenta-glow);
  }

  .drop-title {
    font-size: var(--t-2xl);
    font-weight: 780;
    letter-spacing: -0.025em;
    line-height: 1.15;
    text-shadow: 0 0 24px var(--magenta-glow);
  }

  @media (max-width: 820px) {
    .drop {
      padding-left: 0;
    }
  }

  .toast {
    position: fixed;
    bottom: 1.25rem;
    left: 50%;
    transform: translateX(-50%);
    padding: 0.65rem 1.1rem;
    border-radius: 12px;
    font-size: var(--t-sm);
    max-width: min(560px, 90vw);
    border-color: rgb(255 45 149 / 0.35);
  }

  .toast.error {
    border-color: rgb(255 107 107 / 0.6);
    color: #ffd9d9;
  }

  @media (max-width: 820px) {
    .shell {
      grid-template-columns: 1fr;
      grid-template-rows: auto 1fr;
    }
    nav {
      flex-direction: row;
      align-items: center;
      padding: 0.6rem 1rem;
      border-width: 0 0 1px 0;
    }
    .brand span {
      display: none;
    }
    ul {
      display: flex;
    }
    .reach {
      margin: 0 0 0 auto;
    }
    main {
      padding: 1.5rem 1rem 2rem;
    }
  }
</style>
