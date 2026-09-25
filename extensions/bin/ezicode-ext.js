#!/usr/bin/env node
'use strict';

const fs = require('node:fs');
const path = require('node:path');

const paths = require('../src/paths');
const jsonc = require('../src/jsonc');
const { openVsix, packFolder } = require('../src/vsix');
const { compatibility, readManifest } = require('../src/manifest');
const { installVsix, listInstalled, uninstall, setEnabled } = require('../src/install');
const { convertTheme, buildFamily } = require('../src/theme-convert');

const USAGE = `ezicode-ext — manage ezicode / VS Code-compatible extensions

  ezicode-ext search <query>              search the Open VSX registry
  ezicode-ext install <id|file.vsix>      install from Open VSX or a local .vsix
  ezicode-ext link <folder>               install an unpacked extension folder (dev loop)
  ezicode-ext list                        list installed extensions
  ezicode-ext inspect <id|file.vsix|dir>  report what ezicode supports in an extension
  ezicode-ext uninstall <id>              remove an extension
  ezicode-ext enable|disable <id>         toggle an extension
  ezicode-ext import-theme <file>         convert a VS Code theme json into an ezicode theme
  ezicode-ext pack <folder> [-o out]      build a .vsix from a folder
  ezicode-ext host                        run the extension host on stdio (used by the editor)

Extensions:  ${paths.extensionsDir()}
Themes:      ${paths.themesDir()}
Registry:    ${process.env.EZICODE_REGISTRY || 'https://open-vsx.org'}
`;

const LEVEL_ICON = { full: '✓', partial: '~', planned: '·', none: '✗', unknown: '?' };

function printReport(manifest, report) {
  console.log(`${manifest.displayName}  ${manifest.id}@${manifest.version}`);
  if (manifest.description) console.log(`  ${manifest.description}`);
  console.log(`  verdict: ${report.verdict}${report.declarativeOnly ? ' (no JavaScript: runs without the extension host)' : ''}`);
  if (report.rows.length) {
    console.log('  contributes:');
    for (const row of report.rows) {
      console.log(`    ${LEVEL_ICON[row.level] || '?'} ${row.point.padEnd(22)} ${row.level.padEnd(8)} ${row.note}`);
    }
  }
  if (report.themes.length) {
    console.log(`  themes: ${report.themes.map((t) => t.label).join(', ')}`);
  }
  if (report.unsupportedActivation.length) {
    console.log(`  unsupported activation events: ${report.unsupportedActivation.join(', ')}`);
  }
}

async function cmdSearch(query) {
  const { search } = require('../src/openvsx');
  const results = await search(query);
  if (!results.length) {
    console.log('no results');
    return;
  }
  for (const r of results) {
    const downloads = r.downloadCount ? `${(r.downloadCount / 1000).toFixed(0)}k` : '-';
    console.log(`${r.id.padEnd(40)} ${String(r.version).padEnd(10)} ${downloads.padStart(6)}  ${r.description.slice(0, 60)}`);
  }
}

async function cmdInstall(target) {
  let buffer;
  if (target.endsWith('.vsix') && fs.existsSync(target)) {
    buffer = fs.readFileSync(target);
  } else {
    const { fetchVsix } = require('../src/openvsx');
    process.stderr.write(`downloading ${target} from Open VSX…\n`);
    buffer = (await fetchVsix(target)).buffer;
  }
  const result = installVsix(buffer, { force: true });
  console.log(`installed ${result.manifest.id}@${result.manifest.version} -> ${result.dir}`);
  if (result.themes.length) {
    console.log(`themes ready in ezicode: ${result.themes.join(', ')}`);
    console.log(`  (pick one from the theme menu, or set "workbench.colorTheme" in settings.json)`);
  }
  printReport(result.manifest, result.report);
}

function cmdLink(folder) {
  const src = path.resolve(folder);
  const manifest = readManifest(jsonc.parse(fs.readFileSync(path.join(src, 'package.json'), 'utf8')));
  const { buffer } = packFolder(src);
  const result = installVsix(buffer, { force: true });
  console.log(`linked ${manifest.id} from ${src} -> ${result.dir}`);
  if (result.themes.length) console.log(`themes: ${result.themes.join(', ')}`);
}

function cmdList() {
  const installed = listInstalled();
  if (!installed.length) {
    console.log(`no extensions installed in ${paths.extensionsDir()}`);
    return;
  }
  for (const e of installed) {
    const flags = [e.enabled === false ? 'disabled' : 'enabled', e.declarativeOnly ? 'declarative' : 'host'].join(',');
    console.log(`${e.id.padEnd(40)} ${String(e.version).padEnd(10)} ${flags.padEnd(20)} ${e.displayName || ''}`);
  }
}

async function cmdInspect(target) {
  if (fs.existsSync(target) && fs.statSync(target).isDirectory()) {
    const manifest = readManifest(jsonc.parse(fs.readFileSync(path.join(target, 'package.json'), 'utf8')));
    printReport(manifest, compatibility(manifest));
    return;
  }
  let buffer;
  if (fs.existsSync(target)) buffer = fs.readFileSync(target);
  else {
    const { fetchVsix } = require('../src/openvsx');
    buffer = (await fetchVsix(target)).buffer;
  }
  const vsix = openVsix(buffer);
  printReport(vsix.manifest, compatibility(vsix.manifest));
}

function cmdImportTheme(file, outDir = paths.themesDir()) {
  const doc = jsonc.parse(fs.readFileSync(file, 'utf8'), file);
  const theme = convertTheme(doc, { readInclude: (rel) => {
    const p = path.resolve(path.dirname(file), rel);
    return fs.existsSync(p) ? fs.readFileSync(p, 'utf8') : null;
  } });
  const id = theme.name.toLowerCase().replace(/[^a-z0-9]+/g, '-');
  fs.mkdirSync(outDir, { recursive: true });
  const out = path.join(outDir, `${id}.json`);
  fs.writeFileSync(out, JSON.stringify(buildFamily({ name: theme.name, themes: [theme] }), null, 2) + '\n');
  console.log(`wrote ${out}\nrestart ezicode and choose "${theme.name}" from the theme menu`);
}

function cmdPack(folder, out) {
  const { buffer, manifest } = packFolder(path.resolve(folder));
  const file = out || `${manifest.name}-${manifest.version}.vsix`;
  fs.writeFileSync(file, buffer);
  console.log(`packed ${manifest.id}@${manifest.version} -> ${file} (${buffer.length} bytes)`);
}

async function main() {
  const [command, ...rest] = process.argv.slice(2);
  try {
    switch (command) {
      case 'search':
        await cmdSearch(rest.join(' '));
        break;
      case 'install':
      case 'add':
        await cmdInstall(rest[0]);
        break;
      case 'link':
      case 'dev':
        cmdLink(rest[0]);
        break;
      case 'list':
      case 'ls':
        cmdList();
        break;
      case 'inspect':
      case 'info':
        await cmdInspect(rest[0]);
        break;
      case 'uninstall':
      case 'remove':
        console.log(uninstall(rest[0]) ? `removed ${rest[0]}` : `not installed: ${rest[0]}`);
        break;
      case 'enable':
        console.log(setEnabled(rest[0], true) ? `enabled ${rest[0]}` : `not installed: ${rest[0]}`);
        break;
      case 'disable':
        console.log(setEnabled(rest[0], false) ? `disabled ${rest[0]}` : `not installed: ${rest[0]}`);
        break;
      case 'import-theme':
        cmdImportTheme(rest[0], rest[1]);
        break;
      case 'pack': {
        const outIndex = rest.indexOf('-o');
        const out = outIndex === -1 ? null : rest[outIndex + 1];
        cmdPack(rest[0], out);
        break;
      }
      case 'host':
        require('../src/host/host').main();
        break;
      default:
        process.stdout.write(USAGE);
        if (command) process.exitCode = 1;
    }
  } catch (err) {
    console.error(`error: ${err.message}`);
    process.exitCode = 1;
  }
}

main();
