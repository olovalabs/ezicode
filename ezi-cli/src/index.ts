#!/usr/bin/env node
/**
 * ezi — zero-install launcher for the ezicode native editor.
 *
 * Usage:
 *   npx ezi [file|dir] [args...]   open editor (downloads binary on first run)
 *   npx ezi --help                  show help
 *   npx ezi --version               launcher + editor version
 *
 * Env overrides:
 *   EZI_VERSION   pinned version without `v`, `latest`, or unset (default: auto-detect)
 *   EZI_REPO      `owner/repo` (default: olovalabs/ezicode)
 *   EZI_CACHE     cache dir override
 *   EZI_NO_UPDATE_CHECK  set to 1 to skip GitHub lookup, use fallback immediately
 *   EZI_GITHUB_TOKEN     optional token to raise api.github.com rate limits
 *   NO_COLOR      disable colors in the terminal UI
 *
 * Design: zero npm dependencies (node builtins only) so the published
 * package stays ~5KB. Platform binaries come from GitHub Releases:
 *   linux x64   -> ezicode-x86_64-unknown-linux-gnu.tar.gz
 *   mac arm64   -> ezicode-aarch64-apple-darwin.tar.gz
 *   win x64     -> ezicode-x86_64-pc-windows-msvc.exe (standalone, no unzip)
 *
 * All UI output goes to stderr, so stdout (e.g. `ezi --where`) stays clean.
 */

import * as fs from "node:fs";
import * as https from "node:https";
import * as os from "node:os";
import * as path from "node:path";
import { spawn, execFile } from "node:child_process";

const LAUNCHER_VERSION = "2.0.1"; // npm package version
const FALLBACK_EDITOR_VERSION = "0.1.3"; // used when offline / API fails; keep near app/Cargo.toml
const REPO = process.env.EZI_REPO || "olovalabs/ezicode";

// ---------- tiny terminal UI (no deps) ----------
const isTTY = !!process.stderr.isTTY && !process.env.CI;
const useColor = isTTY && !process.env.NO_COLOR;
const paint = (code: number) => (s: string) => (useColor ? `\x1b[${code}m${s}\x1b[0m` : s);
const bold = paint(1),
  dim = paint(2),
  green = paint(32),
  cyan = paint(36);

// Always restore the cursor, even on Ctrl+C or crashes.
process.on("exit", () => {
  if (isTTY) process.stderr.write("\x1b[?25h");
});
process.on("SIGINT", () => process.exit(130));

function fmtBytes(n: number): string {
  const u = ["B", "KB", "MB", "GB"];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) {
    n /= 1024;
    i++;
  }
  return `${n.toFixed(i === 0 ? 0 : 1)} ${u[i]}`;
}

function fmtTime(s: number): string {
  if (!isFinite(s)) return "--";
  return s >= 60
    ? `${Math.floor(s / 60)}m${String(Math.round(s % 60)).padStart(2, "0")}s`
    : `${Math.round(s)}s`;
}

function spinner(text: string) {
  if (!isTTY) {
    console.error(`ezi: ${text}`);
    return { stop(_final?: string) {} };
  }
  const frames = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏";
  let i = 0;
  process.stderr.write("\x1b[?25l");
  const timer = setInterval(() => {
    process.stderr.write(`\r\x1b[2K${cyan(frames[i++ % frames.length])} ${text}`);
  }, 80);
  return {
    stop(final?: string) {
      clearInterval(timer);
      process.stderr.write("\r\x1b[2K\x1b[?25h");
      if (final) console.error(final);
    },
  };
}

function progressBar(label: string) {
  const start = Date.now();
  let lastDraw = 0;
  if (!isTTY) console.error(`ezi: ${label}`);
  else process.stderr.write("\x1b[?25l");

  return {
    update(done: number, total: number) {
      if (!isTTY) return;
      const now = Date.now();
      if (now - lastDraw < 60 && done !== total) return; // throttle redraws
      lastDraw = now;
      const speed = done / Math.max((now - start) / 1000, 0.001);
      let line: string;
      if (total > 0) {
        const pct = Math.min(done / total, 1);
        const width = 26;
        const filled = Math.round(pct * width);
        const bar = "█".repeat(filled) + dim("░".repeat(width - filled));
        const eta = fmtTime((total - done) / speed);
        line =
          `${cyan(bar)} ${bold(String(Math.round(pct * 100)).padStart(3) + "%")}  ` +
          `${fmtBytes(done)}/${fmtBytes(total)}  ${dim(fmtBytes(speed) + "/s")}  ${dim("eta " + eta)}`;
      } else {
        line = `${cyan("downloading")} ${fmtBytes(done)}  ${dim(fmtBytes(speed) + "/s")}`;
      }
      process.stderr.write(`\r\x1b[2K${label}  ${line}`);
    },
    done(msg: string) {
      if (isTTY) process.stderr.write(`\r\x1b[2K\x1b[?25h`);
      console.error(msg);
    },
    fail() {
      if (isTTY) process.stderr.write(`\r\x1b[2K\x1b[?25h`);
    },
  };
}

// ---------- platform / cache ----------
type Target =
  | { kind: "targz"; asset: string; binRel: string }
  | { kind: "exe"; asset: string };

function resolveTarget(): Target {
  const platform = process.platform;
  const arch = process.arch;

  if (platform === "linux" && arch === "x64") {
    const t = "x86_64-unknown-linux-gnu";
    return { kind: "targz", asset: `ezicode-${t}.tar.gz`, binRel: `ezicode-${t}/ezicode` };
  }
  if (platform === "darwin" && arch === "arm64") {
    const t = "aarch64-apple-darwin";
    return { kind: "targz", asset: `ezicode-${t}.tar.gz`, binRel: `ezicode-${t}/ezicode` };
  }
  if (platform === "win32" && arch === "x64") {
    // Standalone exe on the release — no zip extraction needed.
    return { kind: "exe", asset: "ezicode-x86_64-pc-windows-msvc.exe" };
  }
  throw new Error(
    `ezi: unsupported platform ${platform}/${arch}. ` +
      `Prebuilt binaries exist for linux-x64, darwin-arm64, win32-x64 only. ` +
      `Build from source: https://github.com/${REPO}`
  );
}

function baseCacheDir(): string {
  if (process.env.EZI_CACHE) return process.env.EZI_CACHE;
  if (process.platform === "win32") {
    const base = process.env.LOCALAPPDATA || path.join(os.homedir(), "AppData", "Local");
    return path.join(base, "ezi", "cache");
  }
  const xdg = process.env.XDG_CACHE_HOME || path.join(os.homedir(), ".cache");
  return path.join(xdg, "ezi");
}

function cacheDirFor(tag: string): string {
  return path.join(baseCacheDir(), tag);
}

function latestCacheFile(): string {
  return path.join(baseCacheDir(), "latest.json");
}

function normalizeTag(raw: string): string {
  const t = raw.trim();
  return t.startsWith("v") ? t : `v${t}`;
}

// ---------- release lookup ----------
function fetchLatestTag(verbose: boolean): Promise<string | null> {
  return new Promise((resolve) => {
    const headers: Record<string, string> = {
      "User-Agent": "ezi-launcher",
      Accept: "application/vnd.github+json",
    };
    if (process.env.EZI_GITHUB_TOKEN) {
      headers.Authorization = `Bearer ${process.env.EZI_GITHUB_TOKEN}`;
    }
    const req = https.get(
      `https://api.github.com/repos/${REPO}/releases/latest`,
      { headers, timeout: 5000 },
      (res) => {
        const status = res.statusCode || 0;
        if (status !== 200) {
          if (verbose) console.log(`ezi: release lookup failed (http ${status}), using fallback`);
          res.resume();
          resolve(null);
          return;
        }
        let body = "";
        res.setEncoding("utf8");
        res.on("data", (chunk) => {
          body += chunk;
          // Guard against unexpectedly large payloads.
          if (body.length > 256 * 1024) {
            res.destroy();
            resolve(null);
          }
        });
        res.on("end", () => {
          try {
            const json = JSON.parse(body) as { tag_name?: unknown };
            if (typeof json.tag_name === "string" && json.tag_name.trim()) {
              resolve(normalizeTag(json.tag_name));
            } else {
              resolve(null);
            }
          } catch {
            resolve(null);
          }
        });
      }
    );
    req.on("timeout", () => {
      if (verbose) console.log("ezi: release lookup timed out, using fallback");
      req.destroy();
      resolve(null);
    });
    req.on("error", (err) => {
      if (verbose) console.log(`ezi: release lookup error (${err.message}), using fallback`);
      resolve(null);
    });
  });
}

function readLastSeenTag(verbose: boolean): string | null {
  try {
    const raw = fs.readFileSync(latestCacheFile(), "utf8");
    const cached = JSON.parse(raw) as { tag?: unknown };
    if (typeof cached.tag !== "string" || !cached.tag) return null;
    if (verbose) console.log(`ezi: last seen ${cached.tag}`);
    return cached.tag;
  } catch {
    return null;
  }
}

function writeCachedTag(tag: string): void {
  try {
    fs.mkdirSync(baseCacheDir(), { recursive: true });
    fs.writeFileSync(latestCacheFile(), JSON.stringify({ tag, checkedAt: Date.now() }), "utf8");
  } catch {
    // Cache is best-effort; a read-only homedir must not break launches.
  }
}

/**
 * Resolve which editor release to run:
 *   EZI_VERSION=<x.y.z|vX.Y.Z> -> that pin, no network.
 *   EZI_VERSION=latest (or unset) -> check GitHub every run; same tag reuses
 *     cached binary (no download), new tag downloads once, offline reuses last seen.
 *   EZI_NO_UPDATE_CHECK=1 -> skip network, use last seen or fallback.
 */
async function resolveTag(verbose: boolean): Promise<string> {
  const pinned = (process.env.EZI_VERSION || "").trim();
  if (pinned && pinned.toLowerCase() !== "latest") {
    return normalizeTag(pinned);
  }

  if (process.env.EZI_NO_UPDATE_CHECK === "1") {
    const last = readLastSeenTag(verbose);
    if (last) return last;
    if (verbose) console.log("ezi: update check disabled, using fallback");
    return `v${FALLBACK_EDITOR_VERSION}`;
  }

  // Always check: cheap API call (~200ms). Same version -> cached binary, no download.
  // Spinner only on a real terminal and not in verbose mode (logs would fight it).
  const sp = isTTY && !verbose ? spinner("checking for latest release") : null;
  const latest = await fetchLatestTag(verbose);
  sp?.stop();

  if (latest) {
    const prev = readLastSeenTag(false);
    if (prev !== latest && verbose) console.log(`ezi: new release ${latest} (was ${prev || "none"})`);
    else if (verbose) console.log(`ezi: latest is ${latest}, reusing cache`);
    writeCachedTag(latest);
    return latest;
  }

  // Offline/API failure: reuse last seen tag so repeat launches keep working.
  const stale = readLastSeenTag(verbose);
  if (stale) return stale;
  return `v${FALLBACK_EDITOR_VERSION}`;
}

// ---------- download / extract ----------
type Progress = (done: number, total: number) => void;

// Writes to `<dest>.part` and renames on success, so an interrupted download
// never leaves a corrupt file that existsSync() would later mistake for good.
function download(url: string, dest: string, onProgress?: Progress, redirects = 5): Promise<void> {
  return new Promise((resolve, reject) => {
    const req = https.get(url, { headers: { "User-Agent": "ezi-launcher" } }, (res) => {
      const status = res.statusCode || 0;
      if (status >= 300 && status < 400 && res.headers.location) {
        if (redirects === 0) {
          reject(new Error(`ezi: too many redirects downloading ${url}`));
          return;
        }
        res.resume();
        download(res.headers.location, dest, onProgress, redirects - 1).then(resolve, reject);
        return;
      }
      if (status !== 200) {
        res.resume();
        reject(new Error(`ezi: download failed (${status}) for ${url}`));
        return;
      }

      const total = Number(res.headers["content-length"] || 0);
      let done = 0;
      const part = dest + ".part";
      const out = fs.createWriteStream(part);

      res.on("data", (chunk: Buffer) => {
        done += chunk.length;
        onProgress?.(done, total);
      });
      res.pipe(out);

      out.on("finish", () =>
        out.close(() => {
          try {
            fs.renameSync(part, dest);
            resolve();
          } catch (e) {
            reject(e);
          }
        })
      );
      const fail = (err: Error) => {
        fs.rmSync(part, { force: true });
        reject(err);
      };
      out.on("error", fail);
      res.on("error", fail);
    });
    req.on("error", reject);
  });
}

async function downloadWithUi(url: string, dest: string, what: string): Promise<void> {
  const bar = progressBar(`${bold("ezi")} ${what}`);
  try {
    await download(url, dest, bar.update);
    bar.done(`${green("✔")} downloaded ${what}`);
  } catch (e) {
    bar.fail();
    throw e;
  }
}

function extractTargz(archive: string, destDir: string): Promise<void> {
  return new Promise((resolve, reject) => {
    // System tar exists on linux + macOS runners and images. Zero-dep by design.
    execFile("tar", ["-xzf", archive, "-C", destDir], (err, _stdout, stderr) => {
      if (err) {
        reject(new Error(`ezi: failed to extract ${archive}: ${stderr || err.message}`));
        return;
      }
      resolve();
    });
  });
}

async function ensureBinary(tag: string): Promise<string> {
  const target = resolveTarget();
  const dir = cacheDirFor(tag);
  fs.mkdirSync(dir, { recursive: true });

  if (target.kind === "exe") {
    const bin = path.join(dir, "ezicode.exe");
    if (fs.existsSync(bin)) return bin;
    const url = `https://github.com/${REPO}/releases/download/${tag}/${target.asset}`;
    await downloadWithUi(url, bin, `ezicode ${tag}`);
    return bin;
  }

  const bin = path.join(dir, target.binRel);
  if (fs.existsSync(bin)) {
    fs.chmodSync(bin, 0o755);
    return bin;
  }
  const url = `https://github.com/${REPO}/releases/download/${tag}/${target.asset}`;
  const archive = path.join(dir, target.asset);
  if (!fs.existsSync(archive)) {
    await downloadWithUi(url, archive, `ezicode ${tag}`);
  }

  const sp = spinner(`extracting ${target.asset}`);
  try {
    await extractTargz(archive, dir);
    sp.stop(`${green("✔")} extracted`);
  } catch (e) {
    sp.stop();
    throw e;
  }
  if (!fs.existsSync(bin)) throw new Error(`ezi: binary not found after extract: ${bin}`);
  fs.chmodSync(bin, 0o755);
  return bin;
}

// ---------- CLI ----------
function printHelp(fallbackTag: string): void {
  console.log(`ezi ${LAUNCHER_VERSION} — launcher for ezicode (https://github.com/${REPO})

Usage:
  ezi [file|dir] [editor-args...]   open in ezicode (downloads binary first run)
  ezi --help                        this help
  ezi --version                     launcher version
  ezi --where                       print cached binary path
  ezi --clean                       remove cached binaries

Env:
  EZI_VERSION   pin release (e.g. 0.1.3), "latest", or unset for auto-detect (default ${fallbackTag})
  EZI_REPO      owner/repo (default ${REPO})
  EZI_CACHE     cache dir override
  EZI_NO_UPDATE_CHECK=1  skip GitHub lookup, use cache/fallback
  NO_COLOR=1    disable colors
`);
}

async function main(): Promise<void> {
  const args = process.argv.slice(2);
  const verbose = args.includes("--verbose") || !!process.env.EZI_VERBOSE;
  const passthrough = args.filter((a) => a !== "--verbose");

  if (passthrough.includes("--help") || passthrough.includes("-h")) {
    printHelp(`v${FALLBACK_EDITOR_VERSION}`);
    return;
  }

  const tag = await resolveTag(verbose);

  if (passthrough.includes("--version") || passthrough.includes("-V")) {
    console.log(`ezi ${LAUNCHER_VERSION} (editor ${tag} @ ${REPO})`);
    return;
  }
  if (passthrough.includes("--clean")) {
    fs.rmSync(baseCacheDir(), { recursive: true, force: true });
    console.log("ezi: cache cleared");
    return;
  }

  const bin = await ensureBinary(tag);

  if (passthrough.includes("--where")) {
    console.log(bin);
    return;
  }

  // Launch detached so `npx ezi` returns immediately while the GUI stays open.
  // stdio ignored: the editor owns its own window, not the terminal.
  const child = spawn(bin, passthrough, {
    detached: true,
    stdio: "ignore",
    windowsHide: true,
  });
  child.unref();
  if (verbose) console.log(`ezi: launched ${bin}`);
  else if (isTTY) console.error(`${green("✔")} ${bold("ezicode")} ${dim(tag)} launched`);
}

main().catch((err: unknown) => {
  console.error(err instanceof Error ? err.message : String(err));
  process.exit(1);
});
