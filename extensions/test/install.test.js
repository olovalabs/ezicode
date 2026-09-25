'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const { packFolder, openVsix } = require('../src/vsix');
const { installVsix, listInstalled, setEnabled, uninstall } = require('../src/install');
const { compatibility } = require('../src/manifest');

const EXAMPLE = path.join(__dirname, '..', 'examples', 'hello-ezicode');

function tempWorkspace() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'ezicode-ext-'));
  return {
    root,
    extensionsDir: path.join(root, 'extensions'),
    themesDir: path.join(root, 'themes'),
  };
}

/** Build a theme-only extension folder (the most common kind on Open VSX). */
function makeThemeExtension(dir) {
  fs.mkdirSync(path.join(dir, 'themes'), { recursive: true });
  fs.writeFileSync(
    path.join(dir, 'package.json'),
    JSON.stringify({
      name: 'sample-theme',
      displayName: 'Sample Theme',
      publisher: 'acme',
      version: '2.1.0',
      description: 'A colour theme',
      engines: { vscode: '^1.70.0' },
      categories: ['Themes'],
      contributes: {
        themes: [
          { label: 'Sample Dark', uiTheme: 'vs-dark', path: './themes/sample-color-theme.json' },
          { label: 'Sample Light', uiTheme: 'vs', path: './themes/sample-light-color-theme.json' },
        ],
      },
    })
  );
  for (const file of ['sample-color-theme.json', 'sample-light-color-theme.json']) {
    fs.copyFileSync(path.join(__dirname, 'fixtures', file), path.join(dir, 'themes', file));
  }
  return dir;
}

test('packs an extension folder into a readable vsix', () => {
  const { buffer, manifest } = packFolder(EXAMPLE);
  assert.equal(manifest.id, 'ezicode.hello-ezicode');

  const vsix = openVsix(buffer);
  assert.equal(vsix.manifest.displayName, 'Hello ezicode');
  assert.ok(vsix.read('extension.js').toString().includes("require('vscode')"));
  assert.ok(vsix.files.has('package.json'));
});

test('installs a JavaScript extension and records metadata for the editor', () => {
  const ws = tempWorkspace();
  const { buffer } = packFolder(EXAMPLE);
  const result = installVsix(buffer, ws);

  assert.equal(result.manifest.id, 'ezicode.hello-ezicode');
  assert.ok(fs.existsSync(path.join(result.dir, 'extension.js')));

  const meta = JSON.parse(fs.readFileSync(path.join(result.dir, '.ezicode.json'), 'utf8'));
  assert.equal(meta.id, 'ezicode.hello-ezicode');
  assert.equal(meta.enabled, true);
  assert.equal(meta.declarativeOnly, false);
  assert.equal(meta.main, './extension.js');
  assert.deepEqual(meta.activationEvents, ['onStartupFinished']);
  assert.ok(meta.contributes.some((row) => row.point === 'commands' && row.level === 'full'));

  // Second install of the same version is a no-op unless forced.
  const again = installVsix(buffer, ws);
  assert.equal(again.alreadyInstalled, true);

  assert.deepEqual(listInstalled(ws.extensionsDir).map((e) => e.id), ['ezicode.hello-ezicode']);
  assert.equal(setEnabled('ezicode.hello-ezicode', false, ws), true);
  assert.equal(listInstalled(ws.extensionsDir)[0].enabled, false);
  assert.equal(uninstall('ezicode.hello-ezicode', ws), true);
  assert.deepEqual(listInstalled(ws.extensionsDir), []);
});

test('installing a theme extension writes an ezicode theme family', () => {
  const ws = tempWorkspace();
  const src = makeThemeExtension(path.join(ws.root, 'src-theme'));
  const { buffer } = packFolder(src);

  const result = installVsix(buffer, ws);
  assert.deepEqual(result.themes, ['Sample Dark', 'Sample Light']);

  const family = JSON.parse(fs.readFileSync(path.join(ws.themesDir, 'acme.sample-theme.json'), 'utf8'));
  assert.equal(family.themes.length, 2);
  assert.equal(family.themes[0].name, 'Sample Dark');
  assert.equal(family.themes[0].appearance, 'dark');
  assert.equal(family.themes[1].appearance, 'light');
  // `include` was resolved from inside the vsix, not from disk.
  assert.equal(family.themes[1].style['terminal.ansi.red'], '#ff5555ff');
  assert.equal(family.themes[0].style['editor.background'], '#282a36ff');

  // Uninstalling takes the generated theme with it.
  uninstall('acme.sample-theme', ws);
  assert.equal(fs.existsSync(path.join(ws.themesDir, 'acme.sample-theme.json')), false);
});

test('compatibility report flags surfaces ezicode cannot render', () => {
  const report = compatibility({
    id: 'x.y',
    displayName: 'X',
    main: './main.js',
    activationEvents: ['onView:myView', 'onLanguage:rust'],
    contributes: { themes: [], views: {}, commands: [], debuggers: [] },
  });

  assert.equal(report.verdict, 'partial');
  assert.equal(report.needsHost, true);
  const byPoint = Object.fromEntries(report.rows.map((r) => [r.point, r.level]));
  assert.equal(byPoint.commands, 'full');
  assert.equal(byPoint.views, 'none');
  assert.equal(byPoint.debuggers, 'none');
  assert.deepEqual(report.unsupportedActivation, ['onView:myView']);
});
