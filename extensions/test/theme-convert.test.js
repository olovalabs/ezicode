'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const jsonc = require('../src/jsonc');
const { convertTheme, normalizeColor, blend, scopeResolver } = require('../src/theme-convert');

const FIXTURES = path.join(__dirname, 'fixtures');
const readFixture = (name) => jsonc.parse(fs.readFileSync(path.join(FIXTURES, name), 'utf8'), name);

/**
 * Every style key `app/src/theme/colors.rs` (KEY_MAP) reads. If the converter
 * stops emitting one of these the theme renders with magenta placeholders, so
 * this list is the contract between the importer and the Rust side.
 */
const REQUIRED_STYLE_KEYS = [
  'background',
  'surface.background',
  'elevated_surface.background',
  'element.background',
  'element.hover',
  'element.active',
  'element.selected',
  'ghost_element.hover',
  'ghost_element.active',
  'border',
  'border.variant',
  'border.focused',
  'text',
  'text.muted',
  'text.accent',
  'icon',
  'icon.muted',
  'icon.accent',
  'title_bar.background',
  'status_bar.background',
  'panel.background',
  'toolbar.background',
  'tab_bar.background',
  'tab.active_background',
  'tab.inactive_background',
  'tab.active_foreground',
  'tab.inactive_foreground',
  'editor.background',
  'editor.foreground',
  'terminal.background',
  'version_control.added',
  'version_control.modified',
  'version_control.deleted',
];

test('colour normalisation covers every VS Code spelling', () => {
  assert.equal(normalizeColor('#abc'), '#aabbccff');
  assert.equal(normalizeColor('#abcd'), '#aabbccddff'.slice(0, 9));
  assert.equal(normalizeColor('#282a36'), '#282a36ff');
  assert.equal(normalizeColor('#282A36FF'), '#282a36ff');
  assert.equal(normalizeColor('not a colour'), null);
});

test('translucent chrome is flattened onto the editor background', () => {
  // #44475a75 over #282a36 — what VS Code paints, precomputed for ezicode.
  assert.equal(blend('#44475a75', '#282a36ff'), '#353747ff');
});

test('scope resolution prefers the most specific rule', () => {
  const resolve = scopeResolver([
    { scope: 'comment', settings: { foreground: '#111111' } },
    { scope: 'comment.block.documentation', settings: { foreground: '#222222' } },
  ]);
  assert.equal(resolve('comment').foreground, '#111111');
  assert.equal(resolve('comment.block.documentation').foreground, '#222222');
  assert.equal(resolve('comment.line.double-slash').foreground, '#111111');
  assert.equal(resolve('keyword'), null);
});

test('converts a dark VS Code theme into the ezicode theme format', () => {
  const theme = convertTheme(readFixture('sample-color-theme.json'));

  assert.equal(theme.name, 'Sample Dark');
  assert.equal(theme.appearance, 'dark');

  for (const key of REQUIRED_STYLE_KEYS) {
    assert.ok(theme.style[key], `missing style key: ${key}`);
    assert.match(theme.style[key], /^#[0-9a-f]{8}$/, `${key} must be 8-digit hex, got ${theme.style[key]}`);
  }

  assert.equal(theme.style['editor.background'], '#282a36ff');
  assert.equal(theme.style['editor.foreground'], '#f8f8f2ff');
  assert.equal(theme.style['panel.background'], '#21222cff');
  assert.equal(theme.style['tab.active_background'], '#282a36ff');
  assert.equal(theme.style['version_control.added'], '#50fa7bff');

  // Terminal palette: all 16 ANSI slots come across.
  const ansi = Object.keys(theme.style).filter((k) => k.startsWith('terminal.ansi.'));
  assert.equal(ansi.length, 16);
  assert.equal(theme.style['terminal.ansi.bright_cyan'], '#a4ffffff');

  // Cursor colour feeds the terminal palette on the Rust side.
  assert.equal(theme.players[0].cursor, '#f8f8f0ff');
});

test('maps TextMate scopes onto ezicode syntax tokens, including font style', () => {
  const { style } = convertTheme(readFixture('sample-color-theme.json'));
  const syntax = style.syntax;

  assert.equal(syntax.comment.color, '#6272a4ff');
  assert.equal(syntax.comment.font_style, 'italic');
  assert.equal(syntax['comment.doc'].color, '#6272a4ff');
  assert.equal(syntax.string.color, '#f1fa8cff');
  assert.equal(syntax.number.color, '#bd93f9ff');
  assert.equal(syntax.boolean.color, '#bd93f9ff');
  assert.equal(syntax.keyword.color, '#ff79c6ff');
  assert.equal(syntax.function.color, '#50fa7bff');
  assert.equal(syntax.function.font_weight, 700);
  assert.equal(syntax.type.color, '#8be9fdff');
  assert.equal(syntax.type.font_style, 'italic');
  assert.equal(syntax['variable.parameter'].color, '#ffb86cff');
  assert.equal(syntax.tag.color, '#ff79c6ff');
  assert.equal(syntax.attribute.color, '#50fa7bff');
  assert.equal(syntax['string.escape'].color, '#ff79c6ff');

  // Falls back to semanticTokenColors when no TextMate rule matches.
  assert.equal(syntax.property.color, '#ffb86cff');

  // Unmatched tokens still resolve, so the highlighter never sees a hole.
  assert.equal(syntax.primary.color, '#f8f8f2ff');
  assert.equal(syntax.label.color, '#f8f8f2ff');
  assert.equal(syntax.hint.color, '#6272a4ff');

  // Every syntax entry carries the three fields the Rust deserialiser expects.
  for (const [token, value] of Object.entries(syntax)) {
    assert.ok('color' in value && 'font_style' in value && 'font_weight' in value, token);
    assert.match(value.color, /^#[0-9a-f]{8}$/, token);
  }
});

test('follows `include` chains and detects light themes', () => {
  const doc = readFixture('sample-light-color-theme.json');
  const theme = convertTheme(doc, {
    readInclude: (rel) => fs.readFileSync(path.join(FIXTURES, rel), 'utf8'),
  });

  assert.equal(theme.appearance, 'light');
  assert.equal(theme.style['editor.background'], '#ffffffff');
  // Overridden by the child theme…
  assert.equal(theme.style.syntax.comment.color, '#59636eff');
  // …while the rest is inherited from the included base theme.
  assert.equal(theme.style.syntax.keyword.color, '#ff79c6ff');
  assert.equal(theme.style['terminal.ansi.red'], '#ff5555ff');
});

test('a theme with almost no colours still produces a complete style', () => {
  const theme = convertTheme({ name: 'Bare', type: 'dark', colors: { 'editor.background': '#101010' } });
  for (const key of REQUIRED_STYLE_KEYS) {
    assert.match(theme.style[key] || '', /^#[0-9a-f]{8}$/, `missing ${key}`);
  }
});
