import { createApp } from "vue";
import { createPinia } from "pinia";
import App from "./App.vue";
import "./lib/warTheme.css";

createApp(App).use(createPinia()).mount("#app");

// Dev-build markers so the dev window can't be confused with a packaged
// release: window/taskbar title suffix + a corner badge inside the app.
// import.meta.env.DEV is statically replaced by Vite and tree-shaken out
// of production builds, so none of this reaches release bundles.
if (import.meta.env.DEV) {
  document.title = "WarDex [开发版]";
  import("@tauri-apps/api/window").then(({ getCurrentWindow }) => {
    void getCurrentWindow().setTitle("WarDex [开发版]");
  });

  const badge = document.createElement("div");
  badge.textContent = "DEV";
  badge.style.cssText = [
    "position:fixed",
    "top:6px",
    "right:8px",
    "z-index:2147483647",
    "padding:1px 8px",
    "font:bold 12px/1.4 monospace",
    "color:#fff",
    "background:rgba(200,30,30,.85)",
    "border:1px solid #ff8080",
    "border-radius:3px",
    "pointer-events:none",
  ].join(";");
  document.body.appendChild(badge);
}
