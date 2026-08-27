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
const isBanner = getCurrentWebviewWindow().label === "banner";
const isMenuBar = getCurrentWebviewWindow().label === "menu-bar";
if (isBanner) {
  document.documentElement.classList.add("banner-window");
}
if (isMenuBar) {
  document.documentElement.classList.add("menu-bar-window");
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    {isBanner ? <BannerApp /> : isMenuBar ? <MenuBarApp /> : <App />}
  </React.StrictMode>,
);
