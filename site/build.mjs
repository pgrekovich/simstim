#!/usr/bin/env node
// Builds the static site into dist/. No dependencies, no client-side JavaScript.
//
// The page names the current stable release and /download redirects to its DMG,
// so the build reads the latest GitHub release at build time. CI provides
// GITHUB_TOKEN; locally the gh CLI token is used when present. Prereleases are
// never "latest" on GitHub, so they do not reach the page.
//
// Files in public/ are copied as-is. Files in public/assets/ get a content hash
// in their name and are referenced from the template as {{asset:name.ext}}, which
// lets _headers mark /assets/* immutable without ever serving a stale stylesheet.

import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { cp, mkdir, readdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import path from "node:path";

const REPO = "pgrekovich/simstim";
const SITE = "https://simstim.tech";
const root = import.meta.dirname;
const dist = path.join(root, "dist");

function githubToken() {
  if (process.env.GITHUB_TOKEN) return process.env.GITHUB_TOKEN;
  try {
    return execFileSync("gh", ["auth", "token"], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
  } catch {
    return "";
  }
}

async function latestRelease() {
  const headers = {
    Accept: "application/vnd.github+json",
    "X-GitHub-Api-Version": "2022-11-28",
    "User-Agent": "simstim-site-build",
  };
  const token = githubToken();
  if (token) headers.Authorization = `Bearer ${token}`;
  const response = await fetch(`https://api.github.com/repos/${REPO}/releases/latest`, { headers });
  if (!response.ok) {
    throw new Error(`GitHub API returned ${response.status} for releases/latest: ${await response.text()}`);
  }
  const release = await response.json();
  const asset = release.assets.find((a) => /^Simstim_.+_aarch64\.dmg$/.test(a.name));
  if (!asset) throw new Error(`Release ${release.tag_name} has no aarch64 DMG asset`);

  // The API exposes the asset digest; older releases only have it in the notes.
  const sha256 =
    asset.digest?.replace(/^sha256:/, "") ?? release.body?.match(/\b([0-9a-f]{64})\s+Simstim_/)?.[1] ?? "";
  if (!/^[0-9a-f]{64}$/.test(sha256)) throw new Error(`Release ${release.tag_name} has no SHA-256 for ${asset.name}`);

  const published = new Date(release.published_at);
  return {
    version: release.tag_name.replace(/^v/, ""),
    asset_name: asset.name,
    asset_url: asset.browser_download_url,
    size: `${Math.round(asset.size / 1e6)} MB`,
    sha256,
    release_url: release.html_url,
    published: published.toLocaleDateString("en-US", { year: "numeric", month: "long", day: "numeric", timeZone: "UTC" }),
    published_iso: published.toISOString().slice(0, 10),
  };
}

function escapeHtml(value) {
  return String(value).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
}

async function hashAssets() {
  const source = path.join(root, "public", "assets");
  const target = path.join(dist, "assets");
  await mkdir(target, { recursive: true });
  const map = {};
  for (const name of await readdir(source)) {
    const content = await readFile(path.join(source, name));
    const hash = createHash("sha256").update(content).digest("hex").slice(0, 10);
    const ext = path.extname(name);
    const hashed = `${name.slice(0, -ext.length)}.${hash}${ext}`;
    await writeFile(path.join(target, hashed), content);
    map[name] = `/assets/${hashed}`;
  }
  return map;
}

async function copyPublicRoot() {
  const source = path.join(root, "public");
  for (const name of await readdir(source)) {
    if (name === "assets") continue;
    const from = path.join(source, name);
    if ((await stat(from)).isFile()) await cp(from, path.join(dist, name));
  }
}

function render(template, values, assets) {
  const out = template.replace(/\{\{(asset:)?([\w.@-]+)\}\}/g, (match, isAsset, key) => {
    if (isAsset) {
      if (!assets[key]) throw new Error(`Template references missing asset ${key}`);
      return assets[key];
    }
    if (!(key in values)) throw new Error(`Template references unknown value ${key}`);
    return escapeHtml(values[key]);
  });
  const leftover = out.match(/\{\{[^}]*\}\}/);
  if (leftover) throw new Error(`Unreplaced placeholder ${leftover[0]}`);
  return out;
}

const release = await latestRelease();
await rm(dist, { recursive: true, force: true });
await mkdir(dist, { recursive: true });
await copyPublicRoot();
const assets = await hashAssets();
const template = await readFile(path.join(root, "src", "index.html"), "utf8");
await writeFile(path.join(dist, "index.html"), render(template, { ...release, site: SITE }, assets));
// Stable download address; the README and the app can point here without knowing the version.
await writeFile(path.join(dist, "_redirects"), `/download ${release.asset_url} 302\n`);
console.log(`Built simstim.tech for ${release.version} (${release.asset_name}, ${release.size})`);
