import React from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import "./index.css";
import App from "./App";
import BannerApp from "./banner/BannerApp";
import MenuBarApp from "./menu-bar/MenuBarApp";

// One dist serves every window; the label picks the UI. The banner window
// additionally needs a transparent body (it is a floating card on a
// transparent window), tagged on <html> before first paint.
const previewSurface = import.meta.env.DEV
  ? new URLSearchParams(window.location.search).get("surface")
  : null;
const windowLabel = previewSurface ?? getCurrentWebviewWindow().label;
const isBanner = windowLabel === "banner";
const isMenuBar = windowLabel === "menu-bar";
const previewMenuBarState = import.meta.env.DEV && isMenuBar
  ? new URLSearchParams(window.location.search).get("state") === "recording"
    ? {
        kind: "recording" as const,
        title: "Pricing forum · AI Wingman",
        path: "/preview/pricing-forum.md",
        started_at_ms: Date.now() - 252_000,
      }
    : { kind: "idle" as const }
  : undefined;
if (isBanner) {
  document.documentElement.classList.add("banner-window");
}
if (isMenuBar) {
  document.documentElement.classList.add("menu-bar-window");
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    {isBanner ? <BannerApp /> : isMenuBar ? <MenuBarApp previewState={previewMenuBarState} /> : <App />}
  </React.StrictMode>,
);
