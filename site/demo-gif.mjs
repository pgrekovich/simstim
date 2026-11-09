// Renders docs/images/demo.gif from the fictional preview: a call is detected,
// recorded with a live transcript, stopped, and its summary and open loops
// appear. Needs the demo server on port 1422 and ffmpeg on PATH.
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import sharp from "sharp";
import { withChrome, connect, DEMO, button } from "./chrome.mjs";
import { framed, MARGIN } from "./frame.mjs";

const OUT = path.join(import.meta.dirname, "..", "docs", "images", "demo.gif");
const HEIGHT = 760;
const GIF_WIDTH = 1000;
// A GIF has no soft alpha, so the window shadow is flattened onto the README's
// light background.
const BACKGROUND = { r: 255, g: 255, b: 255 };

// The end of the Website design review call, the note the demo opens after
// Stop; the lines match its transcript and the loops its summary reports.
const LINES = [
  ["me", "Me", "00:24:41", "What about the credits? They feel a little heavy beside the opening text."],
  ["them", "Them", "00:26:27", "Let us try them at the bottom as well. We do not need to decide until both versions are in the prototype."],
  ["me", "Me", "00:28:12", "Maya, can we have the homepage and crops by Wednesday? Then I can send it to two clients."],
  ["them", "Them", "00:29:58", "Wednesday works. I will include the portrait selects too."],
  ["me", "Me", "00:31:44", "Great. We have a direction. Let us make this version feel finished before adding anything else."],
];

/** Each frame: a 2x content render, how long it shows, and whether the banner floats over it. */
const frames = [];

await withChrome(async () => {
  const banner = await renderBanner();
  await record(banner);
});
await encode();

/** The floating call banner at its real size, trimmed to the card. */
async function renderBanner() {
  const { send, waitFor, shot, close } = await connect();
  try {
    await send("Emulation.setDeviceMetricsOverride", { width: 392, height: 170, deviceScaleFactor: 2, mobile: false });
    await send("Emulation.setDefaultBackgroundColorOverride", { color: { r: 0, g: 0, b: 0, a: 0 } });
    await send("Page.enable");
    await send("Page.navigate", { url: `${DEMO}banner.html` });
    await waitFor(`document.body.innerText.includes("Call detected")`);
    return await sharp(await shot()).trim({ threshold: 1, background: { r: 0, g: 0, b: 0, alpha: 0 } }).png().toBuffer();
  } finally {
    await close();
  }
}

/** Drives the preview through one call and collects the frames. */
async function record(banner) {
  const page = await connect();
  const { send, evaluate, waitFor, viewport, click, shot } = page;
  const take = async (ms, { settle = 250, withBanner = false } = {}) => frames.push({ content: await shot(settle), ms, banner: withBanner ? banner : null });
  const emit = (name, payload) => evaluate(`window.__simstimDemo.emit(${JSON.stringify(name)}, ${JSON.stringify(payload)}); 'ok'`);
  // React listens to the native input event; the value setter bypasses its own.
  const setTitle = (text) =>
    evaluate(`(() => {
      const el = document.querySelector('textarea[placeholder="Untitled"]');
      Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value").set.call(el, ${JSON.stringify(text)});
      el.dispatchEvent(new Event("input", { bubbles: true }));
      return "ok";
    })()`);
  try {
    await viewport(HEIGHT);
    await send("Page.enable");
    await send("Page.navigate", { url: DEMO });
    await waitFor(`!!${button("Start Recording")}`);
    await take(900, { settle: 600 });
    await take(2300, { withBanner: true });

    // Start is shifted 28 minutes back so the toolbar reads a call in
    // progress, not 00:00; the clock is restored once recording runs.
    await evaluate("window.__realNow = Date.now; Date.now = () => window.__realNow() - 1690000; 'ok'");
    await click(button("Start Recording"));
    await waitFor(`!!${button("Stop")}`);
    await evaluate("Date.now = window.__realNow; 'ok'");
    await take(500);
    for (const part of ["Website de", "Website design rev", "Website design review"]) {
      await setTitle(part);
      await take(part.endsWith("review") ? 500 : 160, { settle: 80 });
    }
    await click(button("Transcript"));
    await take(500);

    for (const [channel, who, stamp, text] of LINES) {
      const words = text.split(" ");
      for (const share of [0.4, 0.75]) {
        await emit("transcript-interim", { channel, text: words.slice(0, Math.ceil(words.length * share)).join(" ") });
        await take(320, { settle: 100 });
      }
      await emit("transcript-interim", { channel, text: "" });
      await emit("transcript-line", `[${stamp}] ${who}: ${text}`);
      await take(760, { settle: 100 });
    }
    await take(700);

    await click(button("Stop"));
    await waitFor(`!!${button("Summary")}`);
    await take(900);
    await click(button("Summary"));
    await waitFor(`document.body.innerText.includes("Open with Fieldwork") && document.body.innerText.includes("Open questions")`);
    await take(3600);
    await click(`document.querySelector('button[title^="Open loops"]')`);
    await waitFor(`document.body.innerText.includes("Waiting for") && document.body.innerText.includes("Done")`);
    await take(3800);
  } finally {
    await page.close();
  }
}

/** Frames the renders, flattens them and lets ffmpeg build the palette and the GIF. */
async function encode() {
  const dir = mkdtempSync(path.join(tmpdir(), "simstim-gif-"));
  try {
    const list = ["ffconcat version 1.0"];
    for (const [i, frame] of frames.entries()) {
      let image = sharp(await framed(frame.content));
      if (frame.banner) {
        const { width: W } = await image.metadata();
        const { width: bw } = await sharp(frame.banner).metadata();
        image = sharp(await image.composite([{ input: frame.banner, left: W - MARGIN.right - bw - 48, top: MARGIN.top + 56 }]).png().toBuffer());
      }
      const file = path.join(dir, `frame-${String(i).padStart(3, "0")}.png`);
      await image.flatten({ background: BACKGROUND }).resize(GIF_WIDTH, null, { kernel: "lanczos3" }).png().toFile(file);
      list.push(`file '${file}'`, `duration ${(frame.ms / 1000).toFixed(3)}`);
    }
    // The concat demuxer drops the last duration unless the file is named again.
    list.push(`file '${path.join(dir, `frame-${String(frames.length - 1).padStart(3, "0")}.png`)}'`);
    const listFile = path.join(dir, "frames.txt");
    writeFileSync(listFile, list.join("\n") + "\n");
    execFileSync("ffmpeg", [
      "-y", "-loglevel", "error",
      "-f", "concat", "-safe", "0", "-i", listFile,
      "-vf", "split[a][b];[a]palettegen=max_colors=255:stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=5:diff_mode=rectangle",
      "-fps_mode", "passthrough", "-loop", "0", OUT,
    ], { stdio: "inherit" });
    const total = frames.reduce((sum, f) => sum + f.ms, 0);
    console.log(`demo.gif: ${frames.length} frames, ${(total / 1000).toFixed(1)}s`);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}
