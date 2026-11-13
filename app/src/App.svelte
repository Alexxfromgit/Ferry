<script lang="ts">
  import { onMount } from "svelte";
  import { app, init, toast, type View } from "./lib/store.svelte";
  import { message } from "./lib/api";
  import SendView from "./lib/SendView.svelte";
  import ReceiveView from "./lib/ReceiveView.svelte";
  import SettingsView from "./lib/SettingsView.svelte";

  onMount(() => {
    init().catch((err) => toast(`Ferry couldn't start: ${message(err)}`, true));
  });

  const tabs: { id: View; label: string }[] = [
    { id: "send", label: "Send" },
    { id: "receive", label: "Receive" },
    { id: "settings", label: "Settings" },
  ];

  const activeDownloads = $derived(
    app.downloads.filter((d) => ["connecting", "downloading", "exporting"].includes(d.status)).length,
  );

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

<div class="shell" class:dragging={app.dragging}>
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
            onclick={() => (app.view = tab.id)}
          >
            {tab.label}
            {#if tab.id === "send" && app.shares.length > 0}
              <span class="count">{app.shares.length}</span>
            {:else if tab.id === "receive" && activeDownloads > 0}
              <span class="count">{activeDownloads}</span>
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
    min-width: 1.4rem;
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

  .dragging main {
    outline: 2px dashed var(--magenta);
    outline-offset: -14px;
    background: radial-gradient(circle at 50% 40%, var(--magenta-soft), transparent 60%);
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
