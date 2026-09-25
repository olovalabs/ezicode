'use strict';

const fs = require('node:fs');
const path = require('node:path');

const jsonc = require('./jsonc');
const paths = require('./paths');
const { openVsix, extractTo } = require('./vsix');
const { compatibility, readManifest } = require('./manifest');
const { convertTheme, buildFamily } = require('./theme-convert');

/** Metadata file ezicode's Rust side reads to populate the Extensions panel. */
const META_FILE = '.ezicode.json';

function ensureDir(dir) {
  fs.mkdirSync(dir, { recursive: true });
  return dir;
}

/**
 * Convert every theme an extension contributes and write one ezicode theme
 * family file for the whole extension.
 *
 * @returns {{file: string|null, names: string[]}}
 */
function installThemes(vsix, manifest, themesDir) {
  const report = compatibility(manifest);
  if (!report.themes.length) return { file: null, names: [] };

  const themes = [];
  for (const contrib of report.themes) {
    if (!contrib.path) continue;
    const raw = vsix.read(contrib.path);
    if (!raw) {
      process.emitWarning(`theme file missing inside vsix: ${contrib.path}`);
      continue;
    }
    const doc = jsonc.parse(raw, contrib.path);
    const baseDir = path.posix.dirname(contrib.path.replace(/^\.\//, ''));
    const converted = convertTheme(doc, {
      name: contrib.label,
      uiTheme: contrib.uiTheme,
      readInclude: (rel) => vsix.read(path.posix.normalize(path.posix.join(baseDir, rel))),
    });
    themes.push(converted);
  }
  if (!themes.length) return { file: null, names: [] };

  ensureDir(themesDir);
  const file = path.join(themesDir, `${manifest.id}.json`);
  const family = buildFamily({
    name: manifest.displayName,
    author: `${manifest.publisher} (imported from ${manifest.id})`,
    themes,
  });
  fs.writeFileSync(file, JSON.stringify(family, null, 2) + '\n');
  return { file, names: themes.map((t) => t.name) };
}

/**
 * Install a .vsix into the ezicode extensions directory.
 *
 * @param {string|Buffer} input path to a .vsix (or its bytes)
 * @param {{extensionsDir?: string, themesDir?: string, force?: boolean}} [opts]
 */
function installVsix(input, opts = {}) {
  const extensionsDir = opts.extensionsDir || paths.extensionsDir();
  const themesDir = opts.themesDir || paths.themesDir();

  const vsix = openVsix(input);
  const manifest = vsix.manifest;
  const dir = path.join(extensionsDir, manifest.id);

  if (fs.existsSync(dir)) {
    if (!opts.force) {
      const existing = readInstalled(dir);
      if (existing && existing.version === manifest.version) {
        return { manifest, dir, themes: existing.themes || [], alreadyInstalled: true, report: compatibility(manifest) };
      }
    }
    fs.rmSync(dir, { recursive: true, force: true });
  }

  ensureDir(dir);
  extractTo(vsix, dir);

  const themes = installThemes(vsix, manifest, themesDir);
  const report = compatibility(manifest);

  const meta = {
    id: manifest.id,
    name: manifest.name,
    publisher: manifest.publisher,
    displayName: manifest.displayName,
    version: manifest.version,
    description: manifest.description,
    license: manifest.license,
    repository: manifest.repository,
    main: manifest.main,
    activationEvents: manifest.activationEvents,
    categories: manifest.categories,
    enabled: true,
    installedAt: new Date().toISOString(),
    themes: themes.names,
    themeFile: themes.file,
    compatibility: report.verdict,
    declarativeOnly: report.declarativeOnly,
    contributes: report.rows,
  };
  fs.writeFileSync(path.join(dir, META_FILE), JSON.stringify(meta, null, 2) + '\n');

  return { manifest, dir, themes: themes.names, themeFile: themes.file, report, alreadyInstalled: false };
}

function readInstalled(dir) {
  try {
    return JSON.parse(fs.readFileSync(path.join(dir, META_FILE), 'utf8'));
  } catch {
    try {
      const pkg = jsonc.parse(fs.readFileSync(path.join(dir, 'package.json'), 'utf8'), 'package.json');
      const manifest = readManifest(pkg);
      return { ...manifest, enabled: true, themes: [] };
    } catch {
      return null;
    }
  }
}

/** Every installed extension, newest metadata first. */
function listInstalled(extensionsDir = paths.extensionsDir()) {
  if (!fs.existsSync(extensionsDir)) return [];
  return fs
    .readdirSync(extensionsDir, { withFileTypes: true })
    .filter((e) => e.isDirectory())
    .map((e) => readInstalled(path.join(extensionsDir, e.name)))
    .filter(Boolean)
    .sort((a, b) => a.id.localeCompare(b.id));
}

function uninstall(id, opts = {}) {
  const extensionsDir = opts.extensionsDir || paths.extensionsDir();
  const themesDir = opts.themesDir || paths.themesDir();
  const dir = path.join(extensionsDir, id);
  if (!fs.existsSync(dir)) return false;
  fs.rmSync(dir, { recursive: true, force: true });
  const themeFile = path.join(themesDir, `${id}.json`);
  if (fs.existsSync(themeFile)) fs.rmSync(themeFile);
  return true;
}

function setEnabled(id, enabled, opts = {}) {
  const extensionsDir = opts.extensionsDir || paths.extensionsDir();
  const dir = path.join(extensionsDir, id);
  const meta = readInstalled(dir);
  if (!meta) return false;
  meta.enabled = Boolean(enabled);
  fs.writeFileSync(path.join(dir, META_FILE), JSON.stringify(meta, null, 2) + '\n');
  return true;
}

module.exports = { installVsix, listInstalled, readInstalled, uninstall, setEnabled, META_FILE };
