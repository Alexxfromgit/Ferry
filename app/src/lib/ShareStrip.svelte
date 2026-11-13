<script lang="ts">
  // A share in the compact list: a one-line ticket that opens into the full one.
  import { bytes, plural, shortId } from "./format";
  import { copyLink, overallPercent, setSharing } from "./shareActions";
  import ShareSwitch from "./ShareSwitch.svelte";
  import { app } from "./store.svelte";
  import type { ShareActivity, ShareInfo } from "./types";

  let {
    share,
    activity,
    open,
    ontoggle,
  }: { share: ShareInfo; activity?: ShareActivity; open: boolean; ontoggle: () => void } = $props();

  const paused = $derived(share.paused === true);
  const receiving = $derived(paused ? 0 : (activity?.receivers.length ?? 0));
  const live = $derived(receiving > 0);
  const pct = $derived(paused ? null : overallPercent(activity));
  const problem = $derived(app.shareProblems[share.id]);

  const status = $derived.by(() => {
    if (problem) return "Files changed";
    if (paused) return "Off";
    if (live && activity) return `↑ ${bytes(activity.bytesPerSec)}/s · ${receiving}`;
    if (activity && activity.uploadedBytes > 0) return `${bytes(activity.uploadedBytes)} sent`;
    return "Waiting";
  });

  const statusTitle = $derived.by(() => {
    if (problem) return problem;
    if (paused) return "Sharing is off. The link won't work until you turn it on.";
    if (live) return `${plural(receiving, "person", "people")} receiving${pct != null ? `, ${Math.floor(pct)}% done` : ""}`;
    return "Nobody is receiving right now";
  });
</script>

<div class="strip" class:paused class:live class:open class:problem={!!problem}>
  <span class="edge" aria-hidden="true">
    <svg viewBox="0 0 24 24" width="16" height="16">
      <g fill="none" stroke="currentColor" stroke-linecap="round" stroke-width="1.8">
        <circle cx="12" cy="12" r="5.6" />
        {#each [0, 45, 90, 135, 180, 225, 270, 315] as a (a)}
          <line x1="12" y1="12" x2="12" y2="2" transform="rotate({a} 12 12)" />
        {/each}
      </g>
    </svg>
  </span>

  <button class="open-btn" aria-expanded={open} onclick={ontoggle} title={open ? "Show less" : "Show the full ticket"}>
    <span class="name">{share.name}</span>
    <span class="meta">{bytes(share.totalSize)} · {plural(share.fileCount, "file")}</span>
  </button>

  <span class="status" title={statusTitle}>
    {#if live}<span class="dot" aria-hidden="true"></span>{/if}{status}
  </span>

  <span class="toggle">
    <ShareSwitch compact on={!paused} label="Sharing {share.name}" onchange={(on) => setSharing(share, on)} />
  </span>

  <span class="stub">
    <span class="number code" title={share.id}>No. {shortId(share.id)}</span>
    <button class="copy" onclick={() => copyLink(share)} aria-label="Copy link for {share.name}" title="Copy link">
      <svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true">
        <g fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round">
          <rect x="8.5" y="8.5" width="11" height="11" rx="2.2" />
          <path d="M15.5 8.5V6.7a2.2 2.2 0 0 0-2.2-2.2H6.7a2.2 2.2 0 0 0-2.2 2.2v6.6a2.2 2.2 0 0 0 2.2 2.2h1.8" />
        </g>
      </svg>
    </button>
  </span>

  {#if pct != null}
    <span class="progress" aria-hidden="true"><span style:width="{pct}%"></span></span>
  {/if}
</div>

<style>
  .strip {
    --stub: 11rem;
    --notch: 7px;
    position: relative;
    display: grid;
    grid-template-columns: 2.4rem minmax(0, 1fr) auto auto var(--stub);
    align-items: center;
    min-height: 3.4rem;
    color: var(--paper-ink);
    background: var(--paper);
    border-radius: 9px;
    overflow: hidden;
    -webkit-mask:
      radial-gradient(circle var(--notch) at calc(100% - var(--stub)) 0, transparent 98%, #000) top / 100% 51% no-repeat,
      radial-gradient(circle var(--notch) at calc(100% - var(--stub)) 100%, transparent 98%, #000) bottom / 100% 51% no-repeat;
    mask:
      radial-gradient(circle var(--notch) at calc(100% - var(--stub)) 0, transparent 98%, #000) top / 100% 51% no-repeat,
      radial-gradient(circle var(--notch) at calc(100% - var(--stub)) 100%, transparent 98%, #000) bottom / 100% 51% no-repeat;
  }

  .strip.live {
    box-shadow: inset 0 0 0 1px rgb(255 45 149 / 0.35);
  }

  /* The magenta band of the full ticket, turned on its side. */
  .edge {
    align-self: stretch;
    display: grid;
    place-items: center;
    color: #fff;
    background:
      repeating-linear-gradient(115deg, rgb(255 255 255 / 0.06) 0 2px, transparent 2px 7px),
      linear-gradient(180deg, var(--magenta), var(--magenta-deep));
  }

  .paused .edge {
    background: linear-gradient(180deg, #7a6573, #5d4a57);
  }

  .open-btn {
    min-width: 0;
    display: flex;
    align-items: baseline;
    gap: 0.75rem;
    padding: 0.55rem 0.9rem;
    border: none;
    border-radius: 0;
    background: transparent;
    color: inherit;
    text-align: left;
    backdrop-filter: none;
  }

  .open-btn:hover:not(:disabled) {
    background: transparent;
  }

  /* The whole strip opens the ticket; the switch and the copy button sit above this. */
  .open-btn::after {
    content: "";
    position: absolute;
    inset: 0;
  }

  .strip:hover {
    background: #fff;
  }

  .name {
    min-width: 0;
    font-weight: 780;
    letter-spacing: -0.015em;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .meta {
    flex: none;
    font-size: var(--t-xs);
    color: var(--paper-soft);
    font-variant-numeric: tabular-nums;
  }

  .paused .name,
  .paused .meta {
    opacity: 0.6;
  }

  .status {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    padding-right: 0.6rem;
    font-size: var(--t-xs);
    font-weight: 650;
    color: var(--paper-soft);
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }

  .live .status {
    color: var(--magenta-deep);
  }

  .problem .status {
    color: #c0392b;
  }

  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--magenta);
    animation: pulse 1.4s ease-out infinite;
  }

  @keyframes pulse {
    0% {
      box-shadow: 0 0 0 0 rgb(255 45 149 / 0.7);
    }
    100% {
      box-shadow: 0 0 0 6px rgb(255 45 149 / 0);
    }
  }

  /* Above the strip-wide "open" area, so their tooltips can be hovered. */
  .status,
  .number,
  .toggle,
  .stub .copy {
    position: relative;
    z-index: 1;
  }

  .toggle {
    padding-right: 0.5rem;
  }

  .stub {
    align-self: stretch;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.4rem;
    padding: 0 0.5rem 0 0.8rem;
    border-left: 2px dashed #e2c8d8;
    background: #f7eef4;
  }

  .number {
    font-size: var(--t-xs);
    font-weight: 700;
    padding: 0.05rem 0.45rem;
    border-radius: 5px;
    color: var(--gold);
    background: var(--paper-ink);
    white-space: nowrap;
  }

  .paused .number {
    color: #cfc3ca;
  }

  .copy {
    display: grid;
    place-items: center;
    padding: 0.35rem;
    border-radius: 8px;
    border-color: transparent;
    background: transparent;
    color: var(--paper-ink);
    backdrop-filter: none;
  }

  .copy:hover:not(:disabled) {
    background: #f3e6ef;
    border-color: #d8bfd0;
  }

  /* Overall progress of everyone receiving, along the bottom edge. */
  .progress {
    position: absolute;
    left: 2.4rem;
    right: var(--stub);
    bottom: 0;
    height: 3px;
    background: #efdfe9;
  }

  .progress > span {
    display: block;
    height: 100%;
    background: linear-gradient(90deg, var(--magenta-deep), var(--magenta));
    transition: width 240ms linear;
  }

  @media (max-width: 620px) {
    .strip {
      --stub: 3rem;
      -webkit-mask: none;
      mask: none;
    }
    .number,
    .meta {
      display: none;
    }
    .stub {
      padding: 0 0.3rem;
      justify-content: center;
    }
  }
</style>
