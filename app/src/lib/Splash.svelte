<script lang="ts">
  import { fade } from "svelte/transition";
  import { motion } from "./motion";

  let {
    artwork,
    phone,
    ready,
    ondone,
  }: {
    /** The saved artwork setting; null until it has loaded, and the intro waits for it. */
    artwork: boolean | null;
    phone: boolean;
    /** Whether Ferry has finished starting; the intro says it's still working if not. */
    ready: boolean;
    /** Called once the intro has been on screen for its minimum time. */
    ondone: () => void;
  } = $props();

  // How long the intro stays up at least. Phones get the full ~2 s brand moment after the
  // system splash; desktops are relaunched often, so theirs is shorter and plays faster.
  const PHONE_HOLD = 1900;
  const DESKTOP_HOLD = 1250;
  const STILL_HOLD = 700;
  const DESKTOP_PACE = 0.65;

  // app.info (and so isMobile) isn't loaded when this mounts; the Android WebView says so itself.
  const android = typeof navigator !== "undefined" && navigator.userAgent.includes("Android");
  const small = $derived(phone || android);
  const reduced = motion(1) === 0;

  // The intro plays once per launch. A reload (F5 on desktop) shows a still frame only while
  // Ferry is starting, like before the intro existed.
  const PLAYED_KEY = "ferry-intro-played";
  const replay = (() => {
    try {
      return sessionStorage.getItem(PLAYED_KEY) !== null;
    } catch {
      return false;
    }
  })();

  let started = $state(false);
  let held = $state(false);

  // Starts once the artwork setting is known, so the words never jump between layouts.
  $effect(() => {
    if (started || artwork === null) return;
    started = true;
    try {
      sessionStorage.setItem(PLAYED_KEY, "1");
    } catch {
      // Without storage a reload just plays the intro again.
    }
    const hold = replay ? 0 : reduced ? STILL_HOLD : small ? PHONE_HOLD : DESKTOP_HOLD;
    setTimeout(() => {
      held = true;
      ondone();
    }, hold);
  });
</script>

<!-- Covers the UI while Ferry starts and plays the brand intro; the parent removes it once both
     the intro has had its time and init has finished. `global` lets the fade-out play when the
     parent removes this component. It is there from the first frame, on the same night colour
     as the window behind it, so launching never flashes. -->
<div
  class="splash"
  class:artwork={artwork === true}
  class:phone={small}
  class:still={replay}
  style:--pace={small ? 1 : DESKTOP_PACE}
  out:fade|global={{ duration: motion(replay ? 150 : 360) }}
>
  {#if started}
    {#if artwork}
      <img class="art" src={small ? "/art/scene-portrait.webp" : "/art/scene-square.webp"} alt="" aria-hidden="true" />
      <div class="shade" aria-hidden="true"></div>
    {/if}
    <div class="glow" aria-hidden="true"></div>

    <div class="words">
      {#if !artwork}
        <img class="icon" src="/ferry-icon.png" alt="" width="72" height="72" />
      {/if}
      <p class="mark">Ferry</p>
      <span class="rule" aria-hidden="true"></span>
      <p class="tagline">Share files directly with anyone, anywhere</p>
      <div class="status" role="status">
        {#if held && !ready}
          <div class="waiting" in:fade={{ duration: motion(240) }}>
            <p class="muted">Getting your node ready…</p>
            <div class="bar indeterminate" aria-hidden="true"><span></span></div>
          </div>
        {/if}
      </div>
    </div>
  {/if}
</div>

<style>
  .splash {
    position: fixed;
    inset: 0;
    z-index: 10;
    overflow: hidden;
    display: grid;
    place-items: center;
    background: var(--night);
  }

  /* Each part's base style is where it ends up, and the animations play from hidden, so with
     reduced motion (app.css turns animations off) everything is simply there. */
  .art {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
    object-position: 20% 85%;
    opacity: 0.92;
    animation:
      art-in calc(650ms * var(--pace)) cubic-bezier(0, 0, 0.2, 1) both,
      settle calc(2150ms * var(--pace)) cubic-bezier(0.4, 0, 0.2, 1) both;
  }

  .phone .art {
    object-position: center bottom;
  }

  /* Keeps the words readable: a soft dark pool where they sit. */
  .shade {
    position: absolute;
    inset: 0;
    background: radial-gradient(ellipse 46% 38% at 78% 24%, rgb(8 6 11 / 0.72), transparent);
  }

  .phone .shade {
    background: linear-gradient(180deg, rgb(8 6 11 / 0.55), rgb(8 6 11 / 0.15) 45%, transparent 60%);
  }

  .glow {
    position: absolute;
    inset: 0;
    background: radial-gradient(circle at 82% 22%, rgb(255 45 149 / 0.24), transparent 55%);
    animation: art-in calc(1000ms * var(--pace)) cubic-bezier(0.4, 0, 0.2, 1) both;
  }

  /* A reload: everything is already in place, nothing animates in. */
  .still :is(.art, .glow, .icon, .mark, .rule, .tagline) {
    animation: none;
  }

  .words {
    position: relative;
    display: grid;
    justify-items: center;
    gap: 0.75rem;
    padding: 1.4rem 2rem;
    text-align: center;
  }

  /* Over the art, the words sit where the scene is empty: top right on the square scene, high
     up on the portrait one. */
  .artwork .words {
    position: absolute;
    top: 14%;
    right: 7%;
  }

  .phone.artwork .words {
    top: calc(16% + env(safe-area-inset-top));
    right: 0;
    left: 0;
  }

  .icon {
    border-radius: 16px;
    box-shadow: 0 0 0 1px rgb(255 45 149 / 0.45), 0 0 28px -4px var(--magenta-glow);
    animation: rise 520ms cubic-bezier(0.4, 0, 0.2, 1) both;
    animation-delay: calc(240ms * var(--pace));
  }

  .mark {
    font-size: 3.4rem;
    font-weight: 800;
    letter-spacing: -0.03em;
    line-height: 1;
    font-variation-settings: "MONO" 0, "CASL" 1;
    text-shadow: 0 0 24px var(--magenta-glow);
    animation: rise 520ms cubic-bezier(0.4, 0, 0.2, 1) both;
    animation-delay: calc(420ms * var(--pace));
  }

  .rule {
    display: block;
    width: 9rem;
    height: 1px;
    background: linear-gradient(90deg, transparent, var(--magenta), transparent);
    box-shadow: 0 0 10px var(--magenta-glow);
    animation: widen calc(620ms * var(--pace)) cubic-bezier(0.4, 0, 0.2, 1) both;
    animation-delay: calc(940ms * var(--pace));
  }

  .tagline {
    font-size: var(--t-sm);
    letter-spacing: 0.02em;
    color: var(--ink-soft);
    animation: rise-short calc(460ms * var(--pace)) cubic-bezier(0.4, 0, 0.2, 1) both;
    animation-delay: calc(1070ms * var(--pace));
  }

  /* Keeps its height so the words don't shift when the waiting line appears. */
  .status {
    min-height: 2.6rem;
  }

  .waiting {
    display: grid;
    justify-items: center;
    gap: 0.5rem;
    font-size: var(--t-sm);
  }

  .indeterminate {
    width: 200px;
  }

  .indeterminate span {
    width: 40%;
    animation: slide 1.1s ease-in-out infinite alternate;
  }

  @keyframes art-in {
    from {
      opacity: 0;
    }
  }

  @keyframes settle {
    from {
      transform: scale(1.09);
    }
  }

  @keyframes rise {
    from {
      opacity: 0;
      transform: translateY(18px);
    }
  }

  @keyframes rise-short {
    from {
      opacity: 0;
      transform: translateY(10px);
    }
  }

  @keyframes widen {
    from {
      transform: scaleX(0);
    }
  }

  @keyframes slide {
    from {
      transform: translateX(0);
    }
    to {
      transform: translateX(150%);
    }
  }
</style>
