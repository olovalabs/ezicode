'use strict';
// Open VSX is the registry ezicode uses.
//
// The Microsoft Visual Studio Marketplace Terms of Use only allow its
// extensions to be used with "In-Scope Products and Services" (Visual Studio,
// VS Code, GitHub Codespaces, Azure DevOps) — a third-party editor may not
// pull from it. Open VSX (Eclipse Foundation) exists for exactly this case and
// is what VSCodium, Gitpod, Theia, Cursor and Windsurf use.

const https = require('node:https');

const DEFAULT_REGISTRY = process.env.EZICODE_REGISTRY || 'https://open-vsx.org';

function getJson(url) {
  return new Promise((resolve, reject) => {
    https
      .get(url, { headers: { accept: 'application/json', 'user-agent': 'ezicode-ext' } }, (res) => {
        if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
          res.resume();
          resolve(getJson(new URL(res.headers.location, url).toString()));
          return;
        }
        if (res.statusCode !== 200) {
          res.resume();
          reject(new Error(`GET ${url} -> HTTP ${res.statusCode}`));
          return;
        }
        const chunks = [];
        res.on('data', (c) => chunks.push(c));
        res.on('end', () => {
          try {
            resolve(JSON.parse(Buffer.concat(chunks).toString('utf8')));
          } catch (err) {
            reject(err);
          }
        });
      })
      .on('error', reject);
  });
}

function download(url, onProgress) {
  return new Promise((resolve, reject) => {
    https
      .get(url, { headers: { 'user-agent': 'ezicode-ext' } }, (res) => {
        if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
          res.resume();
          resolve(download(new URL(res.headers.location, url).toString(), onProgress));
          return;
        }
        if (res.statusCode !== 200) {
          res.resume();
          reject(new Error(`GET ${url} -> HTTP ${res.statusCode}`));
          return;
        }
        const total = Number(res.headers['content-length'] || 0);
        const chunks = [];
        let received = 0;
        res.on('data', (c) => {
          chunks.push(c);
          received += c.length;
          if (onProgress) onProgress(received, total);
        });
        res.on('end', () => resolve(Buffer.concat(chunks)));
      })
      .on('error', reject);
  });
}

/** `publisher.name` -> registry metadata (latest version). */
async function metadata(id, registry = DEFAULT_REGISTRY) {
  const [publisher, ...rest] = id.split('.');
  const name = rest.join('.');
  if (!publisher || !name) throw new Error(`expected an extension id like "publisher.name", got "${id}"`);
  return getJson(`${registry}/api/${encodeURIComponent(publisher)}/${encodeURIComponent(name)}`);
}

async function search(query, { registry = DEFAULT_REGISTRY, size = 20 } = {}) {
  const url = `${registry}/api/-/search?query=${encodeURIComponent(query)}&size=${size}&includeAllVersions=false`;
  const result = await getJson(url);
  return (result.extensions || []).map((e) => ({
    id: `${e.namespace}.${e.name}`,
    displayName: e.displayName || e.name,
    description: e.description || '',
    version: e.version,
    downloadCount: e.downloadCount || 0,
    files: e.files || {},
  }));
}

/** Download the .vsix bytes for `publisher.name`. */
async function fetchVsix(id, { registry = DEFAULT_REGISTRY, onProgress } = {}) {
  const meta = await metadata(id, registry);
  const url = meta.files?.download;
  if (!url) throw new Error(`no download url for ${id} on ${registry}`);
  return { buffer: await download(url, onProgress), meta };
}

module.exports = { search, metadata, fetchVsix, download, getJson, DEFAULT_REGISTRY };
