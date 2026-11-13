/** A transition length that drops to 0 when the person asked for reduced motion. Svelte
 *  transitions run through the Web Animations API, which app.css's reduced-motion rule
 *  doesn't reach. */
export function motion(ms: number): number {
  return typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches ? 0 : ms;
}
