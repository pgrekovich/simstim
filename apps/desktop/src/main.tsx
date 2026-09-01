import React from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import "./index.css";
import App from "./App";
import BannerApp from "./banner/BannerApp";
import { installScrollbarReveal } from "./lib/scrollbar-reveal";

installScrollbarReveal();

// One dist serves every window; the label picks the UI. The banner window
// additionally needs a transparent body (it is a floating card on a
// transparent window), tagged on <html> before first paint.
const previewSurface = import.meta.env.DEV
  ? new URLSearchParams(window.location.search).get("surface")
  : null;
const windowLabel = previewSurface ?? getCurrentWebviewWindow().label;
const isBanner = windowLabel === "banner";
if (isBanner) {
  document.documentElement.classList.add("banner-window");
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    {isBanner ? <BannerApp /> : <App />}
  </React.StrictMode>,
);
