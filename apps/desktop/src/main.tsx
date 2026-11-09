import React from "react";
import ReactDOM from "react-dom/client";
import "./index.css";
import App from "./App";
import { installScrollbarReveal } from "./lib/scrollbar-reveal";

installScrollbarReveal();

// The main window's entry point. The floating call banner is a separate
// page (banner.html) so its window never loads this bundle.
ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
