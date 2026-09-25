'use strict';
// A `.vsix` is a ZIP whose payload lives under `extension/`.

const fs = require('node:fs');
const path = require('node:path');

const zip = require('./zip');
const jsonc = require('./jsonc');
const { readManifest } = require('./manifest');

const PREFIX = 'extension/';

/**
 * Open a .vsix from disk or a buffer.
 * @param {string|Buffer} input path or buffer
 */
function openVsix(input) {
  const buf = Buffer.isBuffer(input) ? input : fs.readFileSync(input);
  const files = zip.readAll(buf);

  /** Files under `extension/`, keyed by their path *inside* the extension. */
  const entries = new Map();
  for (const [name, data] of files) {
    if (name.startsWith(PREFIX)) entries.set(name.slice(PREFIX.length), data);
  }
  if (!entries.size) {
    throw new Error('not a vsix: archive has no "extension/" folder');
  }

  const manifestRaw = entries.get('package.json');
  if (!manifestRaw) throw new Error('not a vsix: extension/package.json is missing');

  const manifest = readManifest(jsonc.parse(manifestRaw, 'package.json'), {
    source: typeof input === 'string' ? path.resolve(input) : 'buffer',
  });

  return {
    manifest,
    files: entries,
    /** @param {string} relativePath */
    read(relativePath) {
      const clean = relativePath.replace(/^\.\//, '').replace(/^\//, '');
      return entries.get(clean) ?? null;
    },
  };
}

/** Unpack `extension/**` into `destDir`. */
function extractTo(vsix, destDir) {
  for (const [name, data] of vsix.files) {
    const target = path.join(destDir, name);
    // Never let a crafted archive escape the destination (zip-slip).
    const resolved = path.resolve(target);
    if (!resolved.startsWith(path.resolve(destDir) + path.sep)) {
      throw new Error(`refusing to extract outside destination: ${name}`);
    }
    fs.mkdirSync(path.dirname(resolved), { recursive: true });
    fs.writeFileSync(resolved, data);
  }
  return destDir;
}

/** Build a .vsix from an extension folder (a tiny stand-in for `vsce package`). */
function packFolder(srcDir, { ignore = ['node_modules/.bin', '.git', '.vscode-test'] } = {}) {
  const files = [];
  const walk = (dir, rel = '') => {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const relPath = rel ? `${rel}/${entry.name}` : entry.name;
      if (ignore.some((i) => relPath === i || relPath.startsWith(i + '/'))) continue;
      const abs = path.join(dir, entry.name);
      if (entry.isDirectory()) walk(abs, relPath);
      else if (entry.isFile()) files.push({ name: PREFIX + relPath, data: fs.readFileSync(abs) });
    }
  };
  walk(srcDir);

  const manifestEntry = files.find((f) => f.name === PREFIX + 'package.json');
  if (!manifestEntry) throw new Error(`${srcDir} has no package.json`);
  const manifest = readManifest(jsonc.parse(manifestEntry.data, 'package.json'));

  files.unshift({
    name: '[Content_Types].xml',
    data: Buffer.from(
      '<?xml version="1.0" encoding="utf-8"?>\n' +
        '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">' +
        '<Default Extension="json" ContentType="application/json"/>' +
        '<Default Extension="js" ContentType="application/javascript"/>' +
        '<Default Extension="md" ContentType="text/markdown"/>' +
        '<Default Extension="png" ContentType="image/png"/>' +
        '<Default Extension="vsixmanifest" ContentType="text/xml"/>' +
        '</Types>\n',
      'utf8'
    ),
  });

  return { buffer: zip.write(files), manifest };
}

module.exports = { openVsix, extractTo, packFolder };
