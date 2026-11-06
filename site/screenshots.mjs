#!/usr/bin/env node
// Renders the site's app screenshots from the fictional UI preview and puts
// them into a real macOS window frame.
//
// Start the preview first (from apps/desktop: `pnpm exec vite --config
// vite.demo.config.ts`), install sharp here once (`pnpm --dir site install`),
// then run `node screenshots.mjs`. The renders come from headless Google
// Chrome over the DevTools protocol at 2x, so no native window is involved
// and the result does not depend on the Mac's screen or focus state.
//
// screenshot-frame.png is a capture of the real app window with the content
// punched out: macOS's shadow, corner ring and margins. It is 9-sliced around
// each render, the content gets the same corner radius and focused traffic
// lights are drawn where the app places them.
//
// Writes public/assets/app-{summary,loops}.{png,webp,@2x.webp}, public/og.png
// and ../docs/images/{summary,open-loops}.png.

import { spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import sharp from "sharp";

const root = import.meta.dirname;
const DEMO = "http://127.0.0.1:1422/";
const CHROME = process.env.CHROME ?? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const PORT = 9333;
const WIDTH = 1100;

// Geometry of the frame image, in 2x pixels: the window rect sits inside the
// shadow margins, and the chrome measurements come from the same capture.
const MARGIN = { left: 68, top: 52, right: 68, bottom: 84 };
const SLICE = { left: 120, top: 120, right: 120, bottom: 150 };
const RADIUS = 22;
const LIGHTS = { cx: [41.5, 87.5, 133.5], cy: 55.5, r: 14 };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const profile = mkdtempSync(path.join(tmpdir(), "simstim-shots-"));
const chrome = spawn(
  CHROME,
  ["--headless=new", `--remote-debugging-port=${PORT}`, `--user-data-dir=${profile}`, "--no-first-run", "--hide-scrollbars", "about:blank"],
  { stdio: "ignore" },
);
try {
  await waitForChrome();
  const renders = await render();
  for (const [name, buffer] of Object.entries(renders)) {
    await frame(buffer, path.join(root, "public", "assets", `app-${name}`));
  }
  await sharp(path.join(root, "public", "assets", "app-summary@2x.webp"))
    .png({ palette: true, quality: 92, compressionLevel: 9 })
    .toFile(path.join(root, "..", "docs", "images", "summary.png"));
  await sharp(path.join(root, "public", "assets", "app-loops@2x.webp"))
    .png({ palette: true, quality: 92, compressionLevel: 9 })
    .toFile(path.join(root, "..", "docs", "images", "open-loops.png"));
  await og(path.join(root, "public", "assets", "app-summary@2x.webp"), path.join(root, "public", "og.png"));
} finally {
  // Chrome keeps writing to its profile for a moment after SIGTERM.
  const exited = new Promise((resolve) => chrome.once("exit", resolve));
  chrome.kill();
  await exited;
  rmSync(profile, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
}

async function waitForChrome() {
  for (let i = 0; i < 50; i++) {
    try {
      await fetch(`http://127.0.0.1:${PORT}/json/version`);
      return;
    } catch {
      await sleep(100);
    }
  }
  throw new Error(`Chrome did not answer on port ${PORT}; set CHROME to its binary if it is elsewhere`);
}

/** Drives the preview: the summary of the newest call, then the Open loops pane. */
async function render() {
  const target = await (await fetch(`http://127.0.0.1:${PORT}/json/new?${DEMO}`, { method: "PUT" })).json();
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    ws.onopen = resolve;
    ws.onerror = reject;
  });
  let id = 0;
  const pending = new Map();
  ws.onmessage = (e) => {
    const m = JSON.parse(e.data);
    if (m.id && pending.has(m.id)) {
      const { resolve, reject } = pending.get(m.id);
      pending.delete(m.id);
      m.error ? reject(new Error(m.error.message)) : resolve(m.result);
    }
  };
  const send = (method, params = {}) =>
    new Promise((resolve, reject) => {
      const n = ++id;
      pending.set(n, { resolve, reject });
      ws.send(JSON.stringify({ id: n, method, params }));
    });
  const evaluate = async (expression) => {
    const r = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.text);
    return r.result.value;
  };
  const waitFor = async (expression) => {
    for (let i = 0; i < 200; i++) {
      if (await evaluate(expression)) return;
      await sleep(100);
    }
    throw new Error(`timed out waiting for ${expression}`);
  };
  const viewport = (height) => send("Emulation.setDeviceMetricsOverride", { width: WIDTH, height, deviceScaleFactor: 2, mobile: false });
  // A real click, so hover and pressed states end the way they do for a person.
  const click = async (expression) => {
    const box = await evaluate(`(() => { const el = ${expression}; if (!el) return null; const r = el.getBoundingClientRect(); return [r.x + r.width / 2, r.y + r.height / 2]; })()`);
    if (!box) throw new Error(`nothing to click for ${expression}`);
    for (const type of ["mousePressed", "mouseReleased"]) {
      await send("Input.dispatchMouseEvent", { type, x: box[0], y: box[1], button: "left", clickCount: 1 });
    }
    await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: WIDTH - 4, y: 4 });
    await evaluate("document.activeElement && document.activeElement.blur()");
  };
  const shot = async () => {
    await sleep(400);
    const { data } = await send("Page.captureScreenshot", { format: "png" });
    return Buffer.from(data, "base64");
  };
  const button = (text) => `[...document.querySelectorAll("button")].find((b) => b.textContent.includes(${JSON.stringify(text)}))`;

  try {
    await viewport(760);
    await send("Page.enable");
    await send("Page.navigate", { url: DEMO });
    await waitFor(`!!document.querySelector('button[title^="Open loops"]')`);
    await sleep(500);
    await click(button("Website design review"));
    await waitFor(`!!${button("Summary")}`);
    await click(button("Summary"));
    await waitFor(`document.body.innerText.includes("Open with Fieldwork") && document.body.innerText.includes("Open questions")`);
    const summary = await shot();

    await viewport(900);
    await sleep(300);
    await click(`document.querySelector('button[title^="Open loops"]')`);
    await waitFor(`document.body.innerText.includes("Waiting for") && document.body.innerText.includes("Done")`);
    const loops = await shot();
    return { summary, loops };
  } finally {
    ws.close();
    await fetch(`http://127.0.0.1:${PORT}/json/close/${target.id}`);
  }
}

/** Composes a 2x content render into the window frame and writes the three site variants. */
async function frame(content, out) {
  const framePath = path.join(root, "screenshot-frame.png");
  const fm = await sharp(framePath).metadata();
  const cm = await sharp(content).metadata();
  const W = cm.width + MARGIN.left + MARGIN.right;
  const H = cm.height + MARGIN.top + MARGIN.bottom;
  const piece = (left, top, width, height, rw = width, rh = height) =>
    sharp(framePath).extract({ left, top, width, height }).resize(rw, rh, { fit: "fill" }).png().toBuffer();
  const midW = fm.width - SLICE.left - SLICE.right;
  const midH = fm.height - SLICE.top - SLICE.bottom;
  const outMidW = W - SLICE.left - SLICE.right;
  const outMidH = H - SLICE.top - SLICE.bottom;
  const pieces = [
    { input: await piece(0, 0, SLICE.left, SLICE.top), left: 0, top: 0 },
    { input: await piece(fm.width - SLICE.right, 0, SLICE.right, SLICE.top), left: W - SLICE.right, top: 0 },
    { input: await piece(0, fm.height - SLICE.bottom, SLICE.left, SLICE.bottom), left: 0, top: H - SLICE.bottom },
    { input: await piece(fm.width - SLICE.right, fm.height - SLICE.bottom, SLICE.right, SLICE.bottom), left: W - SLICE.right, top: H - SLICE.bottom },
    { input: await piece(SLICE.left, 0, midW, SLICE.top, outMidW, SLICE.top), left: SLICE.left, top: 0 },
    { input: await piece(SLICE.left, fm.height - SLICE.bottom, midW, SLICE.bottom, outMidW, SLICE.bottom), left: SLICE.left, top: H - SLICE.bottom },
    { input: await piece(0, SLICE.top, SLICE.left, midH, SLICE.left, outMidH), left: 0, top: SLICE.top },
    { input: await piece(fm.width - SLICE.right, SLICE.top, SLICE.right, midH, SLICE.right, outMidH), left: W - SLICE.right, top: SLICE.top },
  ];
  const mask = Buffer.from(
    `<svg width="${cm.width}" height="${cm.height}"><rect width="${cm.width}" height="${cm.height}" rx="${RADIUS}" ry="${RADIUS}" fill="white"/></svg>`,
  );
  const lights = Buffer.from(
    `<svg width="${cm.width}" height="${cm.height}">${[
      ["#ff5f57", "#e0443e"],
      ["#febc2e", "#dea123"],
      ["#28c840", "#1aab29"],
    ]
      .map(([fill, ring], i) => `<circle cx="${LIGHTS.cx[i]}" cy="${LIGHTS.cy}" r="${LIGHTS.r - 0.5}" fill="${fill}" stroke="${ring}" stroke-width="1"/>`)
      .join("")}</svg>`,
  );
  const window = await sharp(content)
    .ensureAlpha()
    .composite([
      { input: lights, blend: "over" },
      { input: mask, blend: "dest-in" },
    ])
    .png()
    .toBuffer();
  const full = await sharp({ create: { width: W, height: H, channels: 4, background: { r: 0, g: 0, b: 0, alpha: 0 } } })
    .composite([...pieces, { input: window, left: MARGIN.left, top: MARGIN.top }])
    .png()
    .toBuffer();
  // Trim the faintest shadow and keep even dimensions so the 1x variant is exact.
  const trimmed = await sharp(full).trim({ threshold: 12, background: { r: 0, g: 0, b: 0, alpha: 0 } }).toBuffer({ resolveWithObject: true });
  const w2 = trimmed.info.width - (trimmed.info.width % 2);
  const h2 = trimmed.info.height - (trimmed.info.height % 2);
  const even = await sharp(trimmed.data).extract({ left: 0, top: 0, width: w2, height: h2 }).png().toBuffer();
  await sharp(even).resize(w2 / 2, h2 / 2, { kernel: "lanczos3" }).png({ palette: true, quality: 90, compressionLevel: 9 }).toFile(`${out}.png`);
  await sharp(even).resize(w2 / 2, h2 / 2, { kernel: "lanczos3" }).webp({ quality: 88, alphaQuality: 90 }).toFile(`${out}.webp`);
  await sharp(even).webp({ quality: 86, alphaQuality: 90 }).toFile(`${out}@2x.webp`);
  console.log(`${path.basename(out)}: ${w2 / 2}x${h2 / 2}, set these on its <img> in src/index.html`);
}

/** The 1200x630 link preview: the hero window on the page's grey, cropped at the bottom. */
async function og(source, out) {
  const src = sharp(source);
  const m = await src.metadata();
  const scale = 942 / (WIDTH * 2);
  const w = Math.round(m.width * scale);
  const h = Math.round(m.height * scale);
  const left = 129 - Math.round(((m.width - WIDTH * 2) / 2) * scale);
  const top = 103 - Math.round(20 * scale);
  const window = await src.resize(w, h, { kernel: "lanczos3" }).png().toBuffer();
  const visible = await sharp(window)
    .extract({ left: 0, top: 0, width: Math.min(w, 1200 - left), height: Math.min(h, 630 - top) })
    .png()
    .toBuffer();
  await sharp({ create: { width: 1200, height: 630, channels: 3, background: { r: 242, g: 242, b: 247 } } })
    .composite([{ input: visible, left, top }])
    .png({ palette: true, quality: 90 })
    .toFile(out);
  console.log("og.png");
}
