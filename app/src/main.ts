import "@fontsource-variable/recursive/full.css";
import "./app.css";
import { mount } from "svelte";
import App from "./App.svelte";

// In a plain browser during development, fake the backend so the UI can be previewed.
if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)) {
  const { installDevPreview } = await import("./lib/devPreview");
  installDevPreview();
}

export default mount(App, { target: document.getElementById("app")! });
