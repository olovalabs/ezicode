'use strict';
// Reads a VS Code extension manifest (`package.json`) and reports what ezicode
// can actually do with it.

/**
 * How each VS Code contribution point maps onto ezicode today.
 * `full`    — works exactly like VS Code
 * `partial` — works with caveats
 * `planned` — recognised, not wired up yet
 * `none`    — needs an HTML/webview surface ezicode does not have
 */
const CONTRIBUTION_SUPPORT = {
  themes: ['full', 'Converted to an ezicode theme at install time'],
  iconThemes: ['planned', 'File icon themes land with the icon-theme loader'],
  productIconThemes: ['planned', 'Needs SVG icon overrides in the asset source'],
  commands: ['full', 'Registered in the command palette, executed in the extension host'],
  keybindings: ['partial', 'Key chords map to GPUI bindings; `when` clauses are partially evaluated'],
  menus: ['partial', 'Command palette entries only; context menus are phase 3'],
  configuration: ['full', 'Merged into settings.json and readable via workspace.getConfiguration'],
  configurationDefaults: ['full', 'Applied as setting defaults'],
  languages: ['partial', 'File associations and comment/bracket config; grammars use ezicode tree-sitter'],
  grammars: ['none', 'ezicode highlights with tree-sitter, not TextMate/Oniguruma'],
  snippets: ['planned', 'Snippet completion source is phase 2'],
  taskDefinitions: ['planned', 'Runs through the integrated terminal'],
  problemMatchers: ['planned', ''],
  debuggers: ['none', 'Needs a DAP client and a debug UI'],
  views: ['none', 'Tree/webview panels need a view container host'],
  viewsContainers: ['none', ''],
  customEditors: ['none', 'Requires a webview surface'],
  notebooks: ['none', 'Requires a webview surface'],
  walkthroughs: ['none', ''],
  jsonValidation: ['planned', ''],
  semanticTokenScopes: ['planned', ''],
  colors: ['planned', 'Extension-defined theme colour ids'],
  terminal: ['planned', ''],
  authentication: ['none', ''],
};

/** Activation events the host understands. */
const SUPPORTED_ACTIVATION = new Set([
  '*',
  'onStartupFinished',
]);

function isSupportedActivation(event) {
  if (SUPPORTED_ACTIVATION.has(event)) return true;
  return /^(onCommand|onLanguage|workspaceContains|onFileSystem|onUri):/.test(event);
}

/**
 * @param {object} pkg parsed package.json of a VS Code extension
 * @param {{source?: string}} [meta]
 */
function readManifest(pkg, meta = {}) {
  if (!pkg || typeof pkg !== 'object') throw new Error('manifest is not an object');
  const name = pkg.name;
  const publisher = pkg.publisher || 'unknown';
  if (!name) throw new Error('manifest is missing "name"');

  const contributes = pkg.contributes || {};
  const activationEvents = Array.isArray(pkg.activationEvents) ? pkg.activationEvents : [];

  return {
    id: `${publisher}.${name}`,
    name,
    publisher,
    displayName: pkg.displayName || name,
    version: pkg.version || '0.0.0',
    description: pkg.description || '',
    license: pkg.license || '',
    repository: typeof pkg.repository === 'string' ? pkg.repository : pkg.repository?.url || '',
    engines: pkg.engines || {},
    main: pkg.main || null,
    browser: pkg.browser || null,
    categories: pkg.categories || [],
    activationEvents,
    contributes,
    extensionKind: pkg.extensionKind || [],
    source: meta.source || '',
  };
}

/**
 * Explain how well an extension will run inside ezicode.
 * @param {ReturnType<typeof readManifest>} manifest
 */
function compatibility(manifest) {
  const points = Object.keys(manifest.contributes || {});
  const rows = points.map((point) => {
    const [level, note] = CONTRIBUTION_SUPPORT[point] || ['unknown', 'Not a known VS Code contribution point'];
    return { point, level, note };
  });

  const needsHost = Boolean(manifest.main);
  const unsupportedActivation = manifest.activationEvents.filter((e) => !isSupportedActivation(e));

  let verdict = 'works';
  if (rows.some((r) => r.level === 'none')) verdict = 'partial';
  if (rows.length && rows.every((r) => r.level === 'none')) verdict = 'unsupported';
  if (!rows.length && !needsHost) verdict = 'nothing-to-do';

  // A pure theme/snippet/declarative extension never needs Node at all.
  const declarativeOnly = !needsHost;

  return {
    verdict,
    declarativeOnly,
    needsHost,
    rows,
    unsupportedActivation,
    themes: (manifest.contributes?.themes || []).map((t) => ({
      label: t.label || t.id || manifest.displayName,
      uiTheme: t.uiTheme || 'vs-dark',
      path: t.path,
    })),
  };
}

module.exports = { readManifest, compatibility, CONTRIBUTION_SUPPORT, isSupportedActivation };
