import React from "react";
import ReactDOM from "react-dom/client";
import "../index.css";
import BannerApp from "./BannerApp";

// The banner window's own entry point (banner.html): one card, so it must
// not parse the whole application bundle the main window loads.
ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <BannerApp />
  </React.StrictMode>,
);
