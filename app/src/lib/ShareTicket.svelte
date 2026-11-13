<script lang="ts">
  import { save } from "@tauri-apps/plugin-dialog";
  import { api, message } from "./api";
  import { bytes, duration, plural, shortId, shortPath } from "./format";
  import { copyLink, removeShare, setSharing, shareAgain, statusLine } from "./shareActions";
  import ShareSwitch from "./ShareSwitch.svelte";
  import { app, toast } from "./store.svelte";
  import type { ReceiverProgress, ShareActivity, ShareInfo, SkipReason } from "./types";

  let { share, activity }: { share: ShareInfo; activity?: ShareActivity } = $props();

  let qr = $state<string | null>(null);
  let qrDialog = $state<HTMLDialogElement | null>(null);
  let passwordDialog = $state<HTMLDialogElement | null>(null);
  let passwordProblem = $state("");
  let saving = $state(false);

  // Re-fetch when the share changes or its link may have changed (relay connected).
  $effect(() => {
    const id = share.id;
    void app.linkEpoch;
    api.shareQr(id).then((svg) => (qr = svg)).catch(() => (qr = null));
  });

  // Show the folder the files are served from; the full path is in the tooltip.
  const fullSource = $derived(share.sourcePaths.join("\n"));
  const source = $derived(
    share.sourcePaths.length === 1
      ? shortPath(share.sourcePaths[0].replace(/[\\/][^\\/]*$/, "") || share.sourcePaths[0], 34)
      : `${share.sourcePaths.length} places`,
  );
  const issued = $derived(
    new Date(share.createdAt * 1000).toLocaleDateString(undefined, {
      day: "numeric",
      month: "short",
      year: "numeric",
    }),
  );
  const paused = $derived(share.paused === true);
  const receivers = $derived(paused ? [] : (activity?.receivers ?? []));
  const live = $derived(receivers.length > 0);
  const problem = $derived(app.shareProblems[share.id]);
  const percent = (done: number, total: number) => (total > 0 ? Math.min(100, (done / total) * 100) : 0);

  // Each receiver keeps its passenger number while it downloads, even when others finish.
  const passengers = new Map<number, number>();
  function passengerNo(id: number): number {
    for (const known of passengers.keys()) {
      if (!receivers.some((r) => r.id === known)) passengers.delete(known);
    }
    let n = passengers.get(id);
    if (n === undefined) {
      const taken = new Set(passengers.values());
      n = 1;
      while (taken.has(n)) n++;
      passengers.set(id, n);
    }
    return n;
  }

  function receiverLine(r: ReceiverProgress): string {
    const parts = [
      `${bytes(r.doneBytes)} of ${bytes(r.totalBytes)}`,
      r.bytesPerSec > 0 ? `${bytes(r.bytesPerSec)}/s` : "stalled",
    ];
    if (r.etaSecs != null) parts.push(duration(r.etaSecs));
    return parts.join(" · ");
  }
  const skippedCount = $derived(share.skippedCount ?? 0);
  const reasonText: Record<SkipReason, string> = {
    systemFile: "system file",
    link: "symbolic link",
    emptyFolder: "empty folder",
    unreadable: "couldn't be read",
    notAFile: "not a regular file",
  };

  // The stub's barcode is drawn from the share's hash, so every ticket's bars are its own.
  const barcode = $derived.by(() => {
    const bars: { x: number; w: number }[] = [];
    let x = 0;
    for (const ch of share.id.slice(0, 28)) {
      const d = parseInt(ch, 16);
      const w = 1 + (d % 3);
      bars.push({ x, w });
      x += w + 1 + ((d >> 2) % 2);
    }
    return { bars, width: x };
  });

  const spokes = [0, 45, 90, 135, 180, 225, 270, 315];

  async function saveFile(password?: string) {
    const id = share.id;
    saving = true;
    try {
      const path = await save({
        defaultPath: password === undefined ? `${share.name}.ferry` : "protected-ticket.ferry",
        filters: [{ name: "Ferry ticket", extensions: ["ferry"] }],
      });
      if (!path) return;
      await api.saveShareFile(id, path, password);
      toast(password === undefined ? "Ticket file saved" : "Password-protected ticket saved");
    } catch (err) {
      toast(`Couldn't save the ticket file: ${message(err)}`, true);
    } finally {
      saving = false;
    }
  }

  function saveProtected(event: SubmitEvent & { currentTarget: HTMLFormElement }) {
    event.preventDefault();
    const values = new FormData(event.currentTarget);
    const password = values.get("password");
    if (typeof password !== "string" || !password.trim()) {
      passwordProblem = "Enter a password.";
    } else if (password !== values.get("confirmation")) {
      passwordProblem = "Passwords do not match.";
    } else {
      event.currentTarget.reset();
      passwordDialog?.close();
      void saveFile(password);
    }
  }

</script>

{#snippet helm(size: number)}
  <svg class="helm" viewBox="0 0 24 24" width={size} height={size} aria-hidden="true">
    <g fill="none" stroke="currentColor" stroke-linecap="round">
      <circle cx="12" cy="12" r="5.6" stroke-width="1.7" />
      <circle cx="12" cy="12" r="1.5" stroke-width="1.5" />
      {#each spokes as a (a)}
        <line x1="12" y1="12" x2="12" y2="1.6" stroke-width="1.7" transform="rotate({a} 12 12)" />
      {/each}
    </g>
  </svg>
{/snippet}

<div class="wrap" class:live class:paused>
  <article class="ticket" aria-label="Ticket for {share.name}">
    <div class="main">
      <header class="band">
        {@render helm(20)}
        <span class="line">Ferry</span>
        <span class="kind">Ticket</span>
        <span class="state" class:on={live}>
          {#if paused}
            Off, not sharing
          {:else if live}
            <span class="pulse" aria-hidden="true"></span>Boarding now
          {:else}
            Valid while shared
          {/if}
        </span>
      </header>

      <div class="content">
        <svg class="ship" viewBox="0 0 220 90" aria-hidden="true">
          <path
            d="M18 58h184l-18 22c-3 4-8 6-13 6H50c-5 0-10-2-13-6z M44 58V44c0-3 2-5 5-5h108c3 0 5 2 5 5v14z M66 39V27c0-3 2-5 5-5h62c3 0 5 2 5 5v12z M98 22V10h10v12z"
          />
          <g class="windows">
            <rect x="56" y="46" width="9" height="6" rx="1.5" />
            <rect x="72" y="46" width="9" height="6" rx="1.5" />
            <rect x="88" y="46" width="9" height="6" rx="1.5" />
            <rect x="104" y="46" width="9" height="6" rx="1.5" />
            <rect x="120" y="46" width="9" height="6" rx="1.5" />
            <rect x="136" y="46" width="9" height="6" rx="1.5" />
          </g>
          <path class="wave" d="M6 88c12 0 12-5 24-5s12 5 24 5 12-5 24-5 12 5 24 5 12-5 24-5 12 5 24 5 12-5 24-5 12 5 24 5" />
        </svg>

        <h3 title={share.name}>{share.name}</h3>

        <dl class="fields">
          <div class="from">
            <dt>From</dt>
            <dd class="code" title={fullSource}>{source}</dd>
          </div>
          <div>
            <dt>Files</dt>
            <dd>{share.fileCount}</dd>
          </div>
          <div>
            <dt>Size</dt>
            <dd>{bytes(share.totalSize)}</dd>
          </div>
          <div>
            <dt>Issued</dt>
            <dd>{issued}</dd>
          </div>
        </dl>

        <div class="manifest">
          <p class="activity">{statusLine(share, activity)}</p>
          <!-- Announced when it changes, without the speeds that change every half second. -->
          <span class="sr-only" aria-live="polite">
            {paused ? "Sharing is off" : live ? `${plural(receivers.length, "person", "people")} receiving` : ""}
          </span>
          {#if receivers.length > 0}
            <ol class="passengers" aria-label="People receiving">
              {#each receivers as r (r.id)}
                {@const pct = percent(r.doneBytes, r.totalBytes)}
                {@const no = passengerNo(r.id)}
                <li>
                  <span class="who">Passenger {no}</span>
                  <div
                    class="bar"
                    role="progressbar"
                    aria-label="Passenger {no}"
                    aria-valuemin="0"
                    aria-valuemax="100"
                    aria-valuenow={Math.round(pct)}
                  >
                    <span style:width="{pct}%"></span>
                  </div>
                  <span class="pct">{Math.floor(pct)}%</span>
                  <span class="nums">{receiverLine(r)}</span>
                </li>
              {/each}
            </ol>
          {/if}
        </div>

        {#if problem}
          <div class="problem" role="alert">
            <p><strong>Couldn't turn sharing back on.</strong> {problem}.</p>
            <p class="why">Share the same files again to make a new ticket with a new link.</p>
            <div class="problem-actions">
              <button class="t-main" onclick={() => shareAgain(share)} disabled={app.importing !== null}>
                Share again
              </button>
              <button class="t-text" onclick={() => delete app.shareProblems[share.id]}>Dismiss</button>
            </div>
          </div>
        {/if}

        {#if skippedCount > 0}
          <details class="skipped">
            <summary>{plural(skippedCount, "item")} left out</summary>
            <ul>
              {#each share.skipped ?? [] as item, i (i)}
                <li><span class="code">{item.name}</span> <span class="why">{reasonText[item.reason]}</span></li>
              {/each}
              {#if skippedCount > (share.skipped?.length ?? 0)}
                <li class="why">and {skippedCount - (share.skipped?.length ?? 0)} more</li>
              {/if}
            </ul>
            <p class="why">
              System files, links and empty folders are never shared. Unreadable items may be
              locked or need permissions; share them again once they can be opened.
            </p>
          </details>
        {/if}

        <div class="actions">
          <button class="t-main" onclick={() => copyLink(share)}>Copy link</button>
          <button class="t-line" onclick={() => void saveFile()} disabled={saving}>Save ticket file</button>
          <button class="t-line" onclick={() => passwordDialog?.showModal()} disabled={saving}>
            Save with password
          </button>
          <span class="end">
            <ShareSwitch on={!paused} label="Sharing {share.name}" onchange={(on) => setSharing(share, on)} />
            <button class="t-text" onclick={() => removeShare(share)}>Remove</button>
          </span>
        </div>
      </div>
    </div>

    <div class="stub">
      <header class="band">
        {@render helm(16)}
        <span class="kind">Scan to board</span>
      </header>
      <div class="stub-body">
        <button class="qr" onclick={() => qrDialog?.showModal()} aria-label="Enlarge QR code for {share.name}" disabled={!qr}>
          {#if qr}
            {@html qr}
          {/if}
          {#if paused}<span class="qr-off">Sharing is off</span>{/if}
        </button>
        <p class="number code" title={share.id}>No. {shortId(share.id)}</p>
        <svg
          class="barcode"
          viewBox="0 0 {barcode.width} 10"
          preserveAspectRatio="none"
          aria-hidden="true"
        >
          {#each barcode.bars as bar, i (i)}
            <rect x={bar.x} y="0" width={bar.w} height="10" />
          {/each}
        </svg>
      </div>
    </div>
  </article>
</div>

<dialog
  bind:this={passwordDialog}
  class="protect-dialog"
  aria-label="Save a password-protected ticket"
  onclose={() => {
    passwordDialog?.querySelector("form")?.reset();
    passwordProblem = "";
  }}
>
  <form onsubmit={saveProtected}>
    <h2>Save a password-protected ticket</h2>
    <p class="muted">Use a strong passphrase and send it separately from the ticket file.</p>
    <label>
      Password
      <input name="password" type="password" autocomplete="new-password" required />
    </label>
    <label>
      Confirm password
      <input name="confirmation" type="password" autocomplete="new-password" required />
    </label>
    {#if passwordProblem}<p role="alert">{passwordProblem}</p>{/if}
    <div class="protect-actions">
      <button class="quiet" type="button" onclick={() => passwordDialog?.close()}>Cancel</button>
      <button class="primary" type="submit">Choose where to save</button>
    </div>
  </form>
</dialog>

<!-- A native modal dialog: focus moves in, Escape closes it, the page behind is inert, and
     focus returns to the QR button afterwards. Clicking the backdrop also closes it. -->
<dialog
  bind:this={qrDialog}
  class="qr-large"
  aria-label="QR code for {share.name}"
  onclick={(e) => e.target === qrDialog && qrDialog?.close()}
>
  {#if qr}
    <div class="qr-frame">{@html qr}</div>
  {/if}
  <p>Scan with a phone camera to get the link for <strong>{share.name}</strong>.</p>
  <form method="dialog">
    <button>Close</button>
  </form>
</dialog>

<style>
  .protect-dialog {
    width: min(27rem, calc(100% - 2rem));
    padding: 1.4rem;
    border: 1px solid var(--glass-edge);
    border-radius: 14px;
    background: var(--night-2);
    color: var(--ink);
  }

  .protect-dialog::backdrop {
    background: rgb(0 0 0 / 0.65);
  }

  .protect-dialog form,
  .protect-dialog label {
    display: grid;
    gap: 0.5rem;
  }

  .protect-dialog form {
    gap: 0.85rem;
  }

  .protect-actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 0.5rem;
  }

  /* Outer wrapper carries the glow; the ticket itself is masked for the notches. */
  .wrap {
    --stub: 168px;
    --notch: 12px;
    filter: drop-shadow(0 14px 26px rgb(0 0 0 / 0.55)) drop-shadow(0 0 22px rgb(255 45 149 / 0.16));
  }

  .wrap.live {
    filter: drop-shadow(0 14px 26px rgb(0 0 0 / 0.55)) drop-shadow(0 0 30px rgb(255 45 149 / 0.38));
  }

  .ticket {
    display: grid;
    grid-template-columns: 1fr var(--stub);
    color: var(--paper-ink);
    background: var(--paper);
    border-radius: 12px;
    overflow: hidden;
    -webkit-mask:
      radial-gradient(circle var(--notch) at calc(100% - var(--stub)) 0, transparent 98%, #000) top / 100% 51% no-repeat,
      radial-gradient(circle var(--notch) at calc(100% - var(--stub)) 100%, transparent 98%, #000) bottom / 100% 51% no-repeat;
    mask:
      radial-gradient(circle var(--notch) at calc(100% - var(--stub)) 0, transparent 98%, #000) top / 100% 51% no-repeat,
      radial-gradient(circle var(--notch) at calc(100% - var(--stub)) 100%, transparent 98%, #000) bottom / 100% 51% no-repeat;
  }

  .main {
    min-width: 0;
    display: flex;
    flex-direction: column;
  }

  .band {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    padding: 0.55rem 1.4rem;
    color: #fff;
    background:
      repeating-linear-gradient(115deg, rgb(255 255 255 / 0.05) 0 2px, transparent 2px 7px),
      linear-gradient(90deg, var(--magenta-deep), #e01a84 60%, var(--magenta));
    font-size: var(--t-sm);
  }

  .line {
    font-weight: 800;
    letter-spacing: -0.01em;
    font-variation-settings: "MONO" 0, "CASL" 1;
  }

  .kind {
    opacity: 0.85;
  }

  .state {
    margin-left: auto;
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    font-size: var(--t-xs);
    padding: 0.1rem 0.6rem;
    border-radius: 999px;
    background: rgb(0 0 0 / 0.18);
  }

  .state.on {
    background: rgb(255 255 255 / 0.95);
    color: var(--magenta-deep);
    font-weight: 700;
  }

  .pulse {
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
      box-shadow: 0 0 0 7px rgb(255 45 149 / 0);
    }
  }

  .content {
    position: relative;
    padding: 1rem 1.4rem 1.2rem;
    background:
      radial-gradient(circle at 100% 0, rgb(255 45 149 / 0.07), transparent 45%),
      repeating-radial-gradient(circle at 115% 130%, transparent 0 9px, rgb(184 16 106 / 0.035) 9px 10px);
    flex: 1;
  }

  .ship {
    position: absolute;
    right: 1rem;
    bottom: 0.6rem;
    width: 190px;
    fill: var(--magenta-deep);
    opacity: 0.08;
    pointer-events: none;
  }

  .ship .windows {
    fill: var(--paper);
  }

  .ship .wave {
    fill: none;
    stroke: var(--magenta-deep);
    stroke-width: 3;
  }

  h3 {
    position: relative;
    font-size: var(--t-xl);
    font-weight: 800;
    letter-spacing: -0.025em;
    font-variation-settings: "MONO" 0, "CASL" 0.5;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .fields {
    position: relative;
    display: grid;
    grid-template-columns: minmax(0, 2.2fr) repeat(3, minmax(0, 1fr));
    gap: 0.4rem 1.2rem;
    margin: 0.7rem 0 0.9rem;
    padding: 0.65rem 0;
    border-top: 1px dashed var(--paper-line);
    border-bottom: 1px dashed var(--paper-line);
  }

  .fields div {
    min-width: 0;
  }

  dt {
    font-size: var(--t-xs);
    color: var(--paper-soft);
  }

  dd {
    margin: 0;
    font-weight: 680;
    font-size: var(--t-sm);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .activity {
    position: relative;
    font-size: var(--t-sm);
    color: var(--paper-soft);
    margin-bottom: 0.9rem;
  }

  .live .activity {
    color: var(--magenta-deep);
    font-weight: 680;
    font-variant-numeric: tabular-nums;
  }

  .manifest {
    position: relative;
    margin-bottom: 0.9rem;
  }

  .manifest .activity {
    margin-bottom: 0;
  }

  /* One line per person receiving: who, bar, percent, and the numbers. */
  .passengers {
    list-style: none;
    margin: 0.5rem 0 0;
    padding: 0.55rem 0.7rem;
    display: grid;
    gap: 0.45rem;
    background: #f5ebf2;
    border-radius: 8px;
    max-height: 11rem;
    overflow-y: auto;
  }

  .passengers li {
    display: grid;
    grid-template-columns: 6.2rem minmax(4rem, 1fr) 2.6rem;
    grid-template-areas:
      "who bar pct"
      "nums nums nums";
    align-items: center;
    gap: 0.1rem 0.6rem;
    font-size: var(--t-xs);
  }

  .who {
    grid-area: who;
    font-weight: 700;
  }

  .passengers .bar {
    grid-area: bar;
    background: #e8d4e1;
  }

  .passengers .bar > :global(span) {
    box-shadow: none;
  }

  .pct {
    grid-area: pct;
    text-align: right;
    font-weight: 700;
    font-variation-settings: var(--mono);
    font-variant-numeric: tabular-nums;
  }

  .nums {
    grid-area: nums;
    color: var(--paper-soft);
    font-variant-numeric: tabular-nums;
  }

  .problem {
    position: relative;
    display: grid;
    gap: 0.35rem;
    margin: -0.3rem 0 0.9rem;
    padding: 0.6rem 0.8rem;
    font-size: var(--t-sm);
    border-radius: 8px;
    background: #fdecec;
    box-shadow: inset 3px 0 0 #c0392b;
    overflow-wrap: anywhere;
  }

  .problem-actions {
    display: flex;
    gap: 0.4rem;
  }

  .problem-actions button {
    border-radius: 9px;
    backdrop-filter: none;
  }

  /* A ticket that is off: printed in grey, the QR code covered. */
  .wrap.paused {
    filter: drop-shadow(0 14px 26px rgb(0 0 0 / 0.55));
  }

  .paused .band {
    background:
      repeating-linear-gradient(115deg, rgb(255 255 255 / 0.05) 0 2px, transparent 2px 7px),
      linear-gradient(90deg, #5d4a57, #7a6573);
  }

  .paused h3,
  .paused .fields {
    opacity: 0.6;
  }

  .paused .qr :global(svg) {
    opacity: 0.18;
  }

  .qr {
    position: relative;
  }

  .qr-off {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    padding: 0.4rem;
    font-size: var(--t-xs);
    font-weight: 700;
    color: var(--paper-ink);
    text-align: center;
  }

  .paused .number {
    color: #cfc3ca;
  }

  .end {
    margin-left: auto;
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
  }

  .skipped {
    position: relative;
    margin: -0.4rem 0 0.9rem;
    font-size: var(--t-sm);
  }

  .skipped summary {
    cursor: pointer;
    width: fit-content;
    color: var(--magenta-deep);
    font-weight: 650;
  }

  .skipped ul {
    list-style: none;
    margin: 0.4rem 0;
    padding: 0.5rem 0.7rem;
    max-height: 9.5rem;
    overflow-y: auto;
    background: #f5ebf2;
    border-radius: 8px;
    display: grid;
    gap: 0.15rem;
  }

  .skipped li {
    display: flex;
    justify-content: space-between;
    gap: 1rem;
    min-width: 0;
  }

  .skipped li .code {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .why {
    flex: none;
    color: var(--paper-soft);
    font-size: var(--t-xs);
  }

  p.why {
    max-width: 62ch;
  }

  .actions {
    position: relative;
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
  }

  /* Buttons printed on paper. */
  .actions button {
    border-radius: 9px;
    backdrop-filter: none;
  }

  .t-main {
    background: var(--paper-ink);
    border-color: var(--paper-ink);
    color: #fff;
  }

  .t-main:hover:not(:disabled) {
    background: #43142f;
    border-color: #43142f;
  }

  .t-line {
    background: transparent;
    border-color: #d8bfd0;
    color: var(--paper-ink);
  }

  .t-line:hover:not(:disabled) {
    background: #f3e6ef;
    border-color: #c9a6bd;
  }

  .t-text {
    background: transparent;
    border-color: transparent;
    color: var(--paper-soft);
  }

  .t-text:hover:not(:disabled) {
    background: transparent;
    border-color: #f0b4b4;
    color: #c0392b;
  }

  .stub {
    display: flex;
    flex-direction: column;
    border-left: 2px dashed #e2c8d8;
    background: #f7eef4;
  }

  .stub .band {
    padding: 0.55rem 0.8rem;
    justify-content: center;
    font-size: var(--t-xs);
  }

  .stub-body {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.45rem;
    padding: 0.8rem 0.9rem 0.9rem;
  }

  .qr {
    width: 112px;
    height: 112px;
    padding: 6px;
    background: #fff;
    border: 1px solid var(--paper-line);
    border-radius: 8px;
    display: grid;
    place-items: center;
  }

  .qr:hover:not(:disabled) {
    background: #fff;
    border-color: var(--magenta);
    box-shadow: 0 0 0 3px rgb(255 45 149 / 0.18);
  }

  .qr :global(svg) {
    width: 100%;
    height: 100%;
  }

  /* Gold-foil ticket number, after the wing pin on the crew uniform. */
  .number {
    font-size: var(--t-xs);
    font-weight: 700;
    padding: 0.05rem 0.55rem;
    border-radius: 5px;
    color: var(--gold);
    background: var(--paper-ink);
    box-shadow: inset 0 0 0 1px rgb(226 179 84 / 0.35);
  }

  .barcode {
    width: 112px;
    height: 26px;
    fill: var(--paper-ink);
  }

  .qr-large::backdrop {
    background: rgb(8 6 11 / 0.72);
    backdrop-filter: blur(6px);
  }

  /* Only an open dialog is laid out; a closed one stays hidden. */
  .qr-large[open] {
    display: grid;
  }

  .qr-large {
    background: var(--paper);
    color: var(--paper-ink);
    border: none;
    border-radius: 16px;
    padding: 1.5rem;
    justify-items: center;
    gap: 1rem;
    max-width: 380px;
    text-align: center;
    box-shadow: 0 0 0 1px rgb(255 45 149 / 0.4), 0 0 60px -10px var(--magenta-glow);
  }

  .qr-frame :global(svg) {
    width: 280px;
    height: 280px;
  }

  .qr-large button {
    background: var(--paper-ink);
    border-color: var(--paper-ink);
    color: #fff;
  }

  @media (max-width: 620px) {
    .ticket {
      grid-template-columns: 1fr;
      -webkit-mask: none;
      mask: none;
    }
    .stub {
      border-left: none;
      border-top: 2px dashed #e2c8d8;
    }
    .stub-body {
      flex-direction: row;
      flex-wrap: wrap;
    }
    .fields {
      grid-template-columns: repeat(3, minmax(0, 1fr));
    }
    .fields .from {
      grid-column: 1 / -1;
    }
  }
</style>
