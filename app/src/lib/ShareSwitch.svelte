<script lang="ts">
  // The on/off switch for a share, printed on ticket paper.
  let {
    on,
    label,
    onchange,
    compact = false,
  }: { on: boolean; label: string; onchange: (on: boolean) => void; compact?: boolean } = $props();
</script>

<button
  type="button"
  class="switch"
  class:compact
  role="switch"
  aria-checked={on}
  aria-label={label}
  title={on ? "Sharing is on. Click to turn it off for now." : "Sharing is off. Click to turn it on."}
  onclick={(e) => {
    e.stopPropagation();
    onchange(!on);
  }}
>
  <span class="track" aria-hidden="true"><span class="knob"></span></span>
  {#if !compact}<span class="text">{on ? "Sharing" : "Off"}</span>{/if}
</button>

<style>
  .switch {
    display: inline-flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.3rem 0.55rem 0.3rem 0.35rem;
    border-radius: 999px;
    border-color: transparent;
    background: transparent;
    color: var(--paper-ink);
    backdrop-filter: none;
  }

  .switch.compact {
    padding: 0.25rem;
  }

  .switch:hover:not(:disabled) {
    background: #f3e6ef;
    border-color: transparent;
  }

  .track {
    position: relative;
    width: 34px;
    height: 20px;
    border-radius: 999px;
    background: #d9c6d2;
    transition: background 160ms;
  }

  .knob {
    position: absolute;
    top: 2px;
    left: 2px;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: #fff;
    box-shadow: 0 1px 3px rgb(42 13 32 / 0.35);
    transition: transform 160ms;
  }

  [aria-checked="true"] .track {
    background: linear-gradient(90deg, var(--magenta-deep), var(--magenta));
  }

  [aria-checked="true"] .knob {
    transform: translateX(14px);
  }

  .text {
    font-size: var(--t-sm);
    font-weight: 650;
    min-width: 3.6em;
    text-align: left;
  }
</style>
