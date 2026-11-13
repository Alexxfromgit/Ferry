<script lang="ts">
  import { onMount } from "svelte";
  import { fade } from "svelte/transition";
  import { motion } from "./motion";

  let { artwork }: { artwork: boolean } = $props();

  // A quick start never flashes the splash: it only appears if starting takes a moment.
  let visible = $state(false);
  onMount(() => {
    const timer = setTimeout(() => (visible = true), 250);
    return () => clearTimeout(timer);
  });
</script>

<!-- Covers the UI while Ferry starts; the parent removes it once init finishes. Nothing is
     mounted before the delay, so a fast start leaves the UI untouched. `global` lets the fade-out
     play when the parent removes this component, not only when `visible` changes. -->
{#if visible}
  <div class="splash" class:artwork transition:fade|global={{ duration: motion(300) }}>
    {#if artwork}
      <img src="/art/scene-square.webp" alt="" aria-hidden="true" />
    {/if}
    <div class="words" role="status">
      {#if !artwork}
        <img class="icon" src="/ferry-icon.png" alt="" width="72" height="72" />
      {/if}
      <p class="mark">Ferry</p>
      <p class="muted">Getting your node ready…</p>
      <div class="bar indeterminate" aria-hidden="true"><span></span></div>
    </div>
  </div>
{/if}

<style>
  .splash {
    position: fixed;
    inset: 0;
    z-index: 10;
    display: grid;
    place-items: center;
    background: var(--night);
  }

  .splash > img {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
    object-position: 20% 85%;
  }

  .words {
    position: relative;
    display: grid;
    justify-items: center;
    gap: 0.5rem;
    text-align: center;
  }

  /* Over the art, the words sit top right, where the scene is empty, on a soft shadow. */
  .artwork .words {
    position: absolute;
    top: 14%;
    right: 7%;
    padding: 1.4rem 2rem;
    background: radial-gradient(closest-side, rgb(8 6 11 / 0.75), transparent);
  }

  .icon {
    border-radius: 16px;
    box-shadow: 0 0 0 1px rgb(255 45 149 / 0.45), 0 0 28px -4px var(--magenta-glow);
    margin-bottom: 0.4rem;
  }

  .mark {
    font-size: 3rem;
    font-weight: 800;
    letter-spacing: -0.03em;
    line-height: 1;
    font-variation-settings: "MONO" 0, "CASL" 1;
    text-shadow: 0 0 24px var(--magenta-glow);
  }

  .indeterminate {
    width: 200px;
    margin-top: 0.6rem;
  }

  .indeterminate span {
    width: 40%;
    animation: slide 1.1s ease-in-out infinite alternate;
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
