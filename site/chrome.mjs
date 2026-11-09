// Headless Google Chrome driven over the DevTools protocol. Shared by the
// screenshot and demo GIF scripts, which both render the fictional preview
// (`pnpm --dir apps/desktop exec vite --config vite.demo.config.ts`, port 1422).
import { spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

const CHROME = process.env.CHROME ?? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const PORT = 9333;

export const DEMO = "http://127.0.0.1:1422/";
/** Content width of every window render, in CSS pixels; renders are 2x. */
export const WIDTH = 1100;

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
export const button = (text) => `[...document.querySelectorAll("button")].find((b) => b.textContent.includes(${JSON.stringify(text)}))`;

/** Runs `work` against a fresh headless Chrome and always tears it down. */
export async function withChrome(work) {
  const profile = mkdtempSync(path.join(tmpdir(), "simstim-shots-"));
  const chrome = spawn(
    CHROME,
    ["--headless=new", `--remote-debugging-port=${PORT}`, `--user-data-dir=${profile}`, "--no-first-run", "--hide-scrollbars", "about:blank"],
    { stdio: "ignore" },
  );
  try {
    await waitForChrome();
    return await work();
  } finally {
    // Chrome keeps writing to its profile for a moment after SIGTERM.
    const exited = new Promise((resolve) => chrome.once("exit", resolve));
    chrome.kill();
    await exited;
    rmSync(profile, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
  }
}

async function waitForChrome() {
  for (let i = 0; i < 150; i++) {
    try {
      await fetch(`http://127.0.0.1:${PORT}/json/version`);
      return;
    } catch {
      await sleep(100);
    }
  }
  throw new Error(`Chrome did not answer on port ${PORT}; set CHROME to its binary if it is elsewhere`);
}

/**
 * One DevTools session on a fresh blank tab. The page is navigated only after
 * the viewport is set: the app sizes its sidebar on first render and keeps it.
 */
export async function connect() {
  const target = await (await fetch(`http://127.0.0.1:${PORT}/json/new?about:blank`, { method: "PUT" })).json();
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
  const shot = async (settle = 400) => {
    await sleep(settle);
    const { data } = await send("Page.captureScreenshot", { format: "png" });
    return Buffer.from(data, "base64");
  };
  const close = async () => {
    ws.close();
    await fetch(`http://127.0.0.1:${PORT}/json/close/${target.id}`);
  };
  return { send, evaluate, waitFor, viewport, click, shot, close };
}
