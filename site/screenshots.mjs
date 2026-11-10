// Renders the site screenshots from the fictional preview
// (`pnpm --dir apps/desktop exec vite --config vite.demo.config.ts`, port 1422)
// in headless Chrome at 2x, inside a real macOS window frame.
import path from "node:path";
import sharp from "sharp";
import { withChrome, connect, DEMO, WIDTH, button, sleep } from "./chrome.mjs";
import { framed } from "./frame.mjs";

const root = import.meta.dirname;

await withChrome(async () => {
  const renders = await render();
  for (const [name, buffer] of Object.entries(renders)) {
    await frame(buffer, path.join(root, "public", "assets", `app-${name}`));
  }
  // Card crops for the feature grid, from the same renders (2x pixels).
  await crop(renders.loops, { left: 600, top: 90, width: 1600, height: 1040 }, path.join(root, "public", "assets", "loops-pane"));
  await crop(renders.recording, { left: 1560, top: 16, width: 470, height: 100 }, path.join(root, "public", "assets", "hearing"));
  await banner(await renderBanner(), path.join(root, "public", "assets", "call-banner"));
  await og(path.join(root, "public", "assets", "app-summary@2x.webp"), path.join(root, "public", "og.png"));
});

/** Drives the preview: the newest call's summary, the Open loops pane, then a recording into that call. */
async function render() {
  const { send, evaluate, waitFor, viewport, click, shot, close } = await connect();
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

    // Record continues the open note. Date.now is shifted while Record is
    // clicked so the toolbar reads twelve minutes in, not 00:00.
    await viewport(760);
    await sleep(300);
    await click(button("Website design review"));
    await waitFor(`!!${button("Transcript")}`);
    await evaluate("window.__realNow = Date.now; Date.now = () => window.__realNow() - 754000; 'ok'");
    await click(`document.querySelector('button[title^="Record into this note"]')`);
    await waitFor(`!!document.querySelector('[aria-label="Who is being heard"] [data-heard="true"]')`);
    await evaluate("Date.now = window.__realNow; 'ok'");
    await click(button("Transcript"));
    await waitFor(`document.body.innerText.includes("feel finished before adding")`);
    await sleep(1200);
    const recording = await shot();
    return { summary, loops, recording };
  } finally {
    await close();
  }
}

/** The floating call banner on a transparent window, at its real size. */
async function renderBanner() {
  const { send, waitFor, shot, close } = await connect();
  try {
    await send("Emulation.setDeviceMetricsOverride", { width: 392, height: 170, deviceScaleFactor: 2, mobile: false });
    await send("Emulation.setDefaultBackgroundColorOverride", { color: { r: 0, g: 0, b: 0, a: 0 } });
    await send("Page.enable");
    await send("Page.navigate", { url: `${DEMO}banner.html` });
    await waitFor(`document.body.innerText.includes("Call detected")`);
    return await shot();
  } finally {
    await close();
  }
}

/** A region of a 2x render as 1x and 2x WebP, for the feature cards. */
async function crop(content, region, out) {
  const part = await sharp(content).extract(region).png().toBuffer();
  await sharp(part).webp({ quality: 90, alphaQuality: 100 }).toFile(`${out}@2x.webp`);
  await sharp(part).resize(region.width / 2, region.height / 2, { kernel: "lanczos3" }).webp({ quality: 90, alphaQuality: 100 }).toFile(`${out}.webp`);
  console.log(`${path.basename(out)}: ${region.width / 2}x${region.height / 2}`);
}

/** Trims the banner window's empty space and writes 1x and 2x PNGs. */
async function banner(content, out) {
  const trimmed = await sharp(content).trim({ threshold: 1, background: { r: 0, g: 0, b: 0, alpha: 0 } }).toBuffer({ resolveWithObject: true });
  const w2 = trimmed.info.width - (trimmed.info.width % 2);
  const h2 = trimmed.info.height - (trimmed.info.height % 2);
  const even = await sharp(trimmed.data).extract({ left: 0, top: 0, width: w2, height: h2 }).png().toBuffer();
  await sharp(even).png({ compressionLevel: 9 }).toFile(`${out}@2x.png`);
  await sharp(even).resize(w2 / 2, h2 / 2, { kernel: "lanczos3" }).png({ compressionLevel: 9 }).toFile(`${out}.png`);
  console.log(`${path.basename(out)}: ${w2 / 2}x${h2 / 2}, set these on its <img> in src/index.html`);
}

/** Composes a 2x content render into the window frame and writes the three site variants. */
async function frame(content, out) {
  const full = await framed(content);
  const { width: W, height: H } = await sharp(full).metadata();
  // The margins are even, so the 1x variant is exact.
  await sharp(full).resize(W / 2, H / 2, { kernel: "lanczos3" }).png({ palette: true, quality: 90, compressionLevel: 9 }).toFile(`${out}.png`);
  await sharp(full).resize(W / 2, H / 2, { kernel: "lanczos3" }).webp({ quality: 88, alphaQuality: 100 }).toFile(`${out}.webp`);
  await sharp(full).webp({ quality: 86, alphaQuality: 100 }).toFile(`${out}@2x.webp`);
  console.log(`${path.basename(out)}: ${W / 2}x${H / 2}, set these on its <img> in src/index.html`);
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
