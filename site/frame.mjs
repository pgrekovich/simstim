// Puts a 2x content render inside a real macOS window frame.
//
// `screenshot-frame.png` is a capture of a real window with only the fully
// opaque content punched out, so the window's own corner curve, dark ring and
// shadow survive. It is 9-sliced around the render; the render is masked with
// the hole itself, never with a synthetic radius, because macOS corners are
// not circular. The margins hold the whole shadow, already faded to nothing
// at the edge, so nothing is trimmed by alpha: a threshold would cut it off
// along a rectangle.
import path from "node:path";
import sharp from "sharp";

const framePath = path.join(import.meta.dirname, "screenshot-frame.png");

// Geometry of the frame image, in 2x pixels: the window rect sits inside the
// shadow margins, and the chrome measurements come from the same capture.
export const MARGIN = { left: 68, top: 52, right: 68, bottom: 84 };
const SLICE = { left: 120, top: 120, right: 120, bottom: 150 };
const LIGHTS = { cx: [41.5, 87.5, 133.5], cy: 55.5, r: 14 };

// A GIF composes dozens of same-sized renders; the sliced frame and its hole
// depend only on the content size, so they are built once per size.
const slicedBySize = new Map();

async function slicedFor(width, height) {
  const key = `${width}x${height}`;
  if (slicedBySize.has(key)) return slicedBySize.get(key);
  const fm = await sharp(framePath).metadata();
  const W = width + MARGIN.left + MARGIN.right;
  const H = height + MARGIN.top + MARGIN.bottom;
  const piece = (left, top, w, h, rw = w, rh = h) =>
    sharp(framePath).extract({ left, top, width: w, height: h }).resize(rw, rh, { fit: "fill", kernel: "nearest" }).png().toBuffer();
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
  const sliced = await sharp({ create: { width: W, height: H, channels: 4, background: { r: 0, g: 0, b: 0, alpha: 0 } } })
    .composite(pieces)
    .png()
    .toBuffer();
  // The hole is the window's shape: opaque where the frame is fully transparent.
  const hole = await sharp(sliced)
    .extract({ left: MARGIN.left, top: MARGIN.top, width, height })
    .extractChannel("alpha")
    .threshold(1)
    .negate()
    .raw()
    .toBuffer();
  const rgba = Buffer.alloc(width * height * 4, 255);
  for (let i = 0; i < hole.length; i++) rgba[i * 4 + 3] = hole[i];
  const mask = await sharp(rgba, { raw: { width, height, channels: 4 } }).png().toBuffer();
  const lights = Buffer.from(
    `<svg width="${width}" height="${height}">${[
      ["#ff5f57", "#e0443e"],
      ["#febc2e", "#dea123"],
      ["#28c840", "#1aab29"],
    ]
      .map(([fill, ring], i) => `<circle cx="${LIGHTS.cx[i]}" cy="${LIGHTS.cy}" r="${LIGHTS.r - 0.5}" fill="${fill}" stroke="${ring}" stroke-width="1"/>`)
      .join("")}</svg>`,
  );
  const result = { sliced, mask, lights, W, H };
  slicedBySize.set(key, result);
  return result;
}

/** The 2x render inside the window frame, as a PNG buffer with the shadow's alpha. */
export async function framed(content) {
  const cm = await sharp(content).metadata();
  const { sliced, mask, lights } = await slicedFor(cm.width, cm.height);
  const window = await sharp(content)
    .ensureAlpha()
    .composite([
      { input: lights, blend: "over" },
      { input: mask, blend: "dest-in" },
    ])
    .png()
    .toBuffer();
  return sharp(sliced)
    .composite([{ input: window, left: MARGIN.left, top: MARGIN.top }])
    .png()
    .toBuffer();
}
