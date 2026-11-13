<script lang="ts" module>
  /** hero: an empty screen, so she is the illustration. ambient: content is on top, so she
   *  recedes. drop: files are being dragged over the window. */
  export type SceneMode = "hero" | "ambient" | "drop";
</script>

<script lang="ts">
  let { mode }: { mode: SceneMode } = $props();
</script>

<!-- The mascot in her chair, bottom right, fading into the neon streaks behind the glass. -->
<div class="scene" data-mode={mode} aria-hidden="true">
  <img src="/art/scene-portrait.webp" alt="" decoding="async" />
</div>

<style>
  .scene {
    position: fixed;
    right: 0;
    bottom: 0;
    height: 100%;
    aspect-ratio: 941 / 1672;
    max-width: 60vw;
    pointer-events: none;
    /* The left edge melts into the backdrop. */
    -webkit-mask-image: linear-gradient(to right, transparent, #000 48%);
    mask-image: linear-gradient(to right, transparent, #000 48%);
    transition:
      opacity 500ms ease,
      filter 500ms ease;
    animation: appear 700ms ease both;
  }

  img {
    display: block;
    width: 100%;
    height: 100%;
    object-fit: cover;
    object-position: right bottom;
    /* So does the top, where headings run across it. */
    -webkit-mask-image: linear-gradient(to bottom, transparent, #000 30%);
    mask-image: linear-gradient(to bottom, transparent, #000 30%);
  }

  .scene[data-mode="ambient"] {
    opacity: 0.18;
    filter: brightness(0.5) saturate(0.8);
  }

  /* Above the drop overlay's scrim, so she lights up while it dims everything else. */
  .scene[data-mode="drop"] {
    z-index: 3;
    filter: brightness(1.12) saturate(1.1);
  }

  @keyframes appear {
    from {
      opacity: 0;
    }
  }
</style>
