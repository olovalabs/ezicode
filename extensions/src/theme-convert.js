'use strict';
// Convert a VS Code colour theme (`contributes.themes[*].path`) into the
// ezicode/Zed theme format that `app/src/theme/mod.rs` already knows how to
// load. This is what makes "install any VS Code theme" work without the
// editor having to understand TextMate at all.

const { parse } = require('./jsonc');

// ---------------------------------------------------------------------------
// Colours
// ---------------------------------------------------------------------------

/** Normalise `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa` to `#rrggbbaa`. */
function normalizeColor(value) {
  if (typeof value !== 'string') return null;
  let hex = value.trim().replace(/^#/, '');
  if (!/^[0-9a-fA-F]+$/.test(hex)) return null;
  if (hex.length === 3 || hex.length === 4) {
    hex = hex
      .split('')
      .map((c) => c + c)
      .join('');
  }
  if (hex.length === 6) hex += 'ff';
  if (hex.length !== 8) return null;
  return `#${hex.toLowerCase()}`;
}

function channels(color) {
  const hex = normalizeColor(color);
  if (!hex) return null;
  return {
    r: parseInt(hex.slice(1, 3), 16),
    g: parseInt(hex.slice(3, 5), 16),
    b: parseInt(hex.slice(5, 7), 16),
    a: parseInt(hex.slice(7, 9), 16) / 255,
  };
}

function toHex({ r, g, b, a = 1 }) {
  const byte = (n) => Math.max(0, Math.min(255, Math.round(n))).toString(16).padStart(2, '0');
  return `#${byte(r)}${byte(g)}${byte(b)}${byte(a * 255)}`;
}

/** Flatten a translucent colour onto an opaque one — ezicode paints opaque chrome. */
function blend(fg, bg) {
  const f = channels(fg);
  const b = channels(bg);
  if (!f) return normalizeColor(bg);
  if (!b || f.a === 1) return normalizeColor(fg);
  return toHex({
    r: f.r * f.a + b.r * (1 - f.a),
    g: f.g * f.a + b.g * (1 - f.a),
    b: f.b * f.a + b.b * (1 - f.a),
    a: 1,
  });
}

/** Lighten (dark themes) or darken (light themes) by `amount` 0..1. */
function shift(color, amount, dark) {
  const c = channels(color);
  if (!c) return null;
  const target = dark ? 255 : 0;
  return toHex({
    r: c.r + (target - c.r) * amount,
    g: c.g + (target - c.g) * amount,
    b: c.b + (target - c.b) * amount,
    a: c.a,
  });
}

function withAlpha(color, alpha) {
  const c = channels(color);
  if (!c) return null;
  return toHex({ ...c, a: alpha });
}

// ---------------------------------------------------------------------------
// TextMate scope resolution
// ---------------------------------------------------------------------------

/**
 * Build a lookup over a theme's `tokenColors`, honouring TextMate specificity:
 * the rule with the longest matching scope prefix wins, later rules break ties.
 */
function scopeResolver(tokenColors = []) {
  /** @type {{selector: string, depth: number, order: number, settings: object}[]} */
  const rules = [];
  tokenColors.forEach((rule, order) => {
    if (!rule || !rule.settings) return;
    let scopes = rule.scope ?? '';
    if (typeof scopes === 'string') scopes = scopes.split(',');
    if (!Array.isArray(scopes)) return;
    for (const raw of scopes) {
      const selector = String(raw).trim();
      if (!selector) continue;
      // `meta.foo entity.name` is a descendant selector; the last part is the
      // one that has to match the token itself.
      const leaf = selector.split(/\s+/).pop();
      rules.push({
        selector: leaf,
        depth: leaf.split('.').length + (selector.includes(' ') ? 0.5 : 0),
        order,
        settings: rule.settings,
      });
    }
  });

  return function resolve(scope) {
    let best = null;
    for (const rule of rules) {
      if (rule.selector !== scope && !scope.startsWith(rule.selector + '.')) continue;
      if (!best || rule.depth > best.depth || (rule.depth === best.depth && rule.order > best.order)) {
        best = rule;
      }
    }
    return best ? best.settings : null;
  };
}

/** ezicode/Zed syntax token -> candidate TextMate scopes, most specific first. */
const SYNTAX_SCOPES = {
  attribute: ['entity.other.attribute-name', 'meta.attribute'],
  boolean: ['constant.language.boolean', 'constant.language'],
  comment: ['comment'],
  'comment.doc': ['comment.block.documentation', 'comment.documentation', 'comment'],
  constant: ['variable.other.constant', 'constant.other', 'constant'],
  constructor: ['entity.name.function.constructor', 'entity.name.type.class', 'support.class'],
  embedded: ['meta.embedded', 'punctuation.section.embedded'],
  emphasis: ['markup.italic'],
  'emphasis.strong': ['markup.bold'],
  enum: ['entity.name.type.enum', 'entity.name.type'],
  function: ['entity.name.function', 'support.function'],
  'function.method': ['entity.name.function.member', 'meta.function-call', 'entity.name.function'],
  'function.special.definition': ['entity.name.function', 'support.function'],
  hint: ['comment'],
  keyword: ['keyword.control', 'keyword', 'storage.type', 'storage.modifier'],
  'keyword.operator': ['keyword.operator'],
  label: ['entity.name.label', 'constant.other.label'],
  link_text: ['string.other.link', 'markup.underline.link'],
  link_uri: ['markup.underline.link', 'string.other.link'],
  number: ['constant.numeric'],
  operator: ['keyword.operator'],
  predictive: ['comment'],
  preproc: ['keyword.control.directive', 'meta.preprocessor'],
  primary: [],
  property: ['variable.other.property', 'support.variable.property', 'meta.object-literal.key'],
  'property.json_key': ['support.type.property-name.json', 'meta.object-literal.key', 'support.type.property-name'],
  punctuation: ['punctuation'],
  'punctuation.bracket': ['punctuation.definition.bracket', 'meta.brace', 'punctuation'],
  'punctuation.delimiter': ['punctuation.separator', 'punctuation.terminator', 'punctuation'],
  'punctuation.delimiter.jsx': ['punctuation.definition.tag', 'punctuation'],
  'punctuation.list_marker': ['punctuation.definition.list.begin.markdown', 'markup.list'],
  'punctuation.special': ['punctuation.definition.template-expression', 'punctuation.section.embedded'],
  string: ['string.quoted', 'string'],
  'string.escape': ['constant.character.escape'],
  'string.regex': ['string.regexp'],
  'string.special': ['string.other', 'string.unquoted'],
  'string.special.symbol': ['constant.other.symbol', 'string.other'],
  tag: ['entity.name.tag'],
  'tag.doctype': ['meta.tag.sgml.doctype', 'entity.name.tag'],
  'text.literal': ['markup.inline.raw', 'markup.raw'],
  title: ['markup.heading', 'entity.name.section'],
  type: ['entity.name.type', 'support.type', 'storage.type'],
  'type.builtin': ['support.type', 'keyword.type', 'storage.type.primitive'],
  variable: ['variable.other.readwrite', 'variable.other', 'variable'],
  'variable.builtin': ['variable.language', 'support.variable'],
  'variable.parameter': ['variable.parameter'],
  'variable.special': ['variable.language'],
  variant: ['variable.other.enummember', 'constant.other.enum', 'entity.name.type.enum'],
};

/** Tokens that should stay dimmed even when the scope lookup finds nothing. */
const DIMMED = new Set(['hint', 'predictive']);

function fontFields(settings) {
  const style = String(settings?.fontStyle ?? '').toLowerCase();
  const out = {};
  out.font_style = style.includes('italic') ? 'italic' : style.includes('underline') ? 'underline' : null;
  out.font_weight = style.includes('bold') ? 700 : null;
  return out;
}

function buildSyntax(theme, editorForeground, dark) {
  const resolve = scopeResolver(theme.tokenColors);
  const semantic = theme.semanticTokenColors || {};
  const syntax = {};

  for (const [token, scopes] of Object.entries(SYNTAX_SCOPES)) {
    let settings = null;
    for (const scope of scopes) {
      settings = resolve(scope);
      if (settings && settings.foreground) break;
      settings = settings || null;
    }

    // Semantic highlighting can fill gaps TextMate rules leave behind.
    if ((!settings || !settings.foreground) && typeof semantic[token] === 'string') {
      settings = { foreground: semantic[token] };
    }

    let color = normalizeColor(settings?.foreground) || editorForeground;
    if (DIMMED.has(token)) {
      const comment = resolve('comment');
      color = normalizeColor(comment?.foreground) || shift(editorForeground, 0.45, !dark) || editorForeground;
    }
    syntax[token] = { color, ...fontFields(settings) };
  }
  return syntax;
}

// ---------------------------------------------------------------------------
// Workbench colours -> ezicode style keys
// ---------------------------------------------------------------------------

/** ezicode style key -> VS Code workbench colour ids, in priority order. */
const COLOR_MAP = {
  background: ['editor.background', 'sideBar.background'],
  'surface.background': ['sideBar.background', 'panel.background', 'editor.background'],
  'elevated_surface.background': ['editorWidget.background', 'dropdown.background', 'menu.background', 'sideBar.background'],
  'element.background': ['input.background', 'dropdown.background', 'button.secondaryBackground'],
  'element.hover': ['list.hoverBackground', 'toolbar.hoverBackground'],
  'element.active': ['list.activeSelectionBackground', 'list.focusBackground'],
  'element.selected': ['list.inactiveSelectionBackground', 'list.activeSelectionBackground'],
  'ghost_element.hover': ['list.hoverBackground', 'toolbar.hoverBackground'],
  'ghost_element.active': ['list.activeSelectionBackground', 'toolbar.activeBackground'],
  border: ['panel.border', 'editorGroup.border', 'sideBar.border', 'contrastBorder', 'widget.border'],
  'border.variant': ['editorGroup.border', 'panel.border', 'sideBar.border'],
  'border.focused': ['focusBorder', 'inputOption.activeBorder'],
  text: ['foreground', 'editor.foreground'],
  'text.muted': ['descriptionForeground', 'disabledForeground', 'tab.inactiveForeground'],
  'text.accent': ['textLink.foreground', 'focusBorder', 'button.background'],
  icon: ['icon.foreground', 'foreground'],
  'icon.muted': ['descriptionForeground', 'disabledForeground'],
  'icon.accent': ['textLink.foreground', 'focusBorder'],
  'title_bar.background': ['titleBar.activeBackground', 'editorGroupHeader.tabsBackground', 'sideBar.background'],
  'status_bar.background': ['statusBar.background', 'titleBar.activeBackground', 'sideBar.background'],
  'panel.background': ['sideBar.background', 'panel.background', 'editor.background'],
  'toolbar.background': ['breadcrumb.background', 'editorGroupHeader.tabsBackground', 'editor.background'],
  'tab_bar.background': ['editorGroupHeader.tabsBackground', 'tab.inactiveBackground', 'sideBar.background'],
  'tab.active_background': ['tab.activeBackground', 'editor.background'],
  'tab.inactive_background': ['tab.inactiveBackground', 'editorGroupHeader.tabsBackground'],
  'tab.active_foreground': ['tab.activeForeground', 'foreground'],
  'tab.inactive_foreground': ['tab.inactiveForeground', 'descriptionForeground'],
  'editor.background': ['editor.background'],
  'editor.foreground': ['editor.foreground', 'foreground'],
  'editor.line_number': ['editorLineNumber.foreground', 'descriptionForeground'],
  'editor.active_line_number': ['editorLineNumber.activeForeground', 'foreground'],
  'editor.active_line.background': ['editor.lineHighlightBackground', 'editor.background'],
  'terminal.background': ['terminal.background', 'panel.background', 'editor.background'],
  'terminal.foreground': ['terminal.foreground', 'editor.foreground', 'foreground'],
  'version_control.added': ['gitDecoration.addedResourceForeground', 'editorGutter.addedBackground'],
  'version_control.modified': ['gitDecoration.modifiedResourceForeground', 'editorGutter.modifiedBackground'],
  'version_control.deleted': ['gitDecoration.deletedResourceForeground', 'editorGutter.deletedBackground'],
  'error.background': ['inputValidation.errorBackground', 'editorWidget.background'],
  'error.border': ['inputValidation.errorBorder', 'editorError.foreground'],
  'warning.background': ['inputValidation.warningBackground', 'editorWidget.background'],
  'warning.border': ['inputValidation.warningBorder', 'editorWarning.foreground'],
  'info.background': ['inputValidation.infoBackground', 'editorWidget.background'],
  'info.border': ['inputValidation.infoBorder', 'editorInfo.foreground'],
};

const ANSI_MAP = {
  'terminal.ansi.black': 'terminal.ansiBlack',
  'terminal.ansi.red': 'terminal.ansiRed',
  'terminal.ansi.green': 'terminal.ansiGreen',
  'terminal.ansi.yellow': 'terminal.ansiYellow',
  'terminal.ansi.blue': 'terminal.ansiBlue',
  'terminal.ansi.magenta': 'terminal.ansiMagenta',
  'terminal.ansi.cyan': 'terminal.ansiCyan',
  'terminal.ansi.white': 'terminal.ansiWhite',
  'terminal.ansi.bright_black': 'terminal.ansiBrightBlack',
  'terminal.ansi.bright_red': 'terminal.ansiBrightRed',
  'terminal.ansi.bright_green': 'terminal.ansiBrightGreen',
  'terminal.ansi.bright_yellow': 'terminal.ansiBrightYellow',
  'terminal.ansi.bright_blue': 'terminal.ansiBrightBlue',
  'terminal.ansi.bright_magenta': 'terminal.ansiBrightMagenta',
  'terminal.ansi.bright_cyan': 'terminal.ansiBrightCyan',
  'terminal.ansi.bright_white': 'terminal.ansiBrightWhite',
};

const DEFAULTS = {
  dark: { bg: '#1e1e1eff', fg: '#d4d4d4ff' },
  light: { bg: '#ffffffff', fg: '#333333ff' },
};

/** Merge an `include`d base theme with the theme that includes it. */
function mergeThemes(base, child) {
  return {
    ...base,
    ...child,
    colors: { ...(base.colors || {}), ...(child.colors || {}) },
    tokenColors: [...(base.tokenColors || []), ...(child.tokenColors || [])],
    semanticTokenColors: { ...(base.semanticTokenColors || {}), ...(child.semanticTokenColors || {}) },
  };
}

/**
 * Resolve `include` chains.
 * @param {object} theme parsed theme document
 * @param {(relativePath: string) => string|Buffer|null} readInclude
 */
function resolveIncludes(theme, readInclude, depth = 0) {
  if (!theme || !theme.include || depth > 8 || typeof readInclude !== 'function') return theme;
  const raw = readInclude(theme.include);
  if (!raw) return theme;
  const base = resolveIncludes(parse(raw, theme.include), readInclude, depth + 1);
  const merged = mergeThemes(base, theme);
  delete merged.include;
  return merged;
}

/**
 * Convert one VS Code theme document into an ezicode theme entry.
 *
 * @param {object} themeDoc parsed `*-color-theme.json`
 * @param {{name?: string, uiTheme?: string, readInclude?: Function}} options
 * @returns {{name: string, appearance: 'dark'|'light', style: object}}
 */
function convertTheme(themeDoc, options = {}) {
  const theme = resolveIncludes(themeDoc, options.readInclude);
  const colors = theme.colors || {};

  const uiTheme = options.uiTheme || theme.type || 'dark';
  const dark = !(uiTheme === 'vs' || /light/i.test(uiTheme));
  const appearance = dark ? 'dark' : 'light';
  const fallback = DEFAULTS[appearance];

  const pick = (ids) => {
    for (const id of ids) {
      const value = normalizeColor(colors[id]);
      if (value) return value;
    }
    return null;
  };

  const editorBg = pick(['editor.background']) || fallback.bg;
  const editorFg = pick(['editor.foreground', 'foreground']) || fallback.fg;

  const style = {};
  for (const [key, ids] of Object.entries(COLOR_MAP)) {
    let value = pick(ids);
    if (!value) continue;
    // Chrome surfaces must be opaque; VS Code often ships them translucent.
    if (/background|_bar|surface|^background$/.test(key) && !key.includes('active_line')) {
      value = blend(value, editorBg);
    }
    style[key] = value;
  }

  // Fill the gaps the theme did not define, deriving from the editor colours so
  // every ezicode UI token resolves (the Rust side asserts completeness).
  const ensure = (key, value) => {
    if (!style[key] && value) style[key] = value;
  };
  ensure('background', editorBg);
  ensure('editor.background', editorBg);
  ensure('editor.foreground', editorFg);
  ensure('text', editorFg);
  ensure('surface.background', shift(editorBg, 0.03, dark));
  ensure('panel.background', style['surface.background']);
  ensure('elevated_surface.background', shift(editorBg, 0.06, dark));
  ensure('element.background', shift(editorBg, 0.08, dark));
  ensure('element.hover', withAlpha(shift(editorBg, 0.16, dark), 0.6));
  ensure('element.active', shift(editorBg, 0.2, dark));
  ensure('element.selected', shift(editorBg, 0.14, dark));
  ensure('ghost_element.hover', withAlpha(shift(editorBg, 0.16, dark), 0.45));
  ensure('ghost_element.active', withAlpha(shift(editorBg, 0.22, dark), 0.6));
  ensure('border', shift(editorBg, 0.14, dark));
  ensure('border.variant', shift(editorBg, 0.1, dark));
  ensure('border.focused', style['text.accent'] || shift(editorBg, 0.35, dark));
  ensure('text.muted', shift(editorFg, 0.35, !dark));
  ensure('text.accent', style['border.focused'] || editorFg);
  ensure('icon', editorFg);
  ensure('icon.muted', style['text.muted']);
  ensure('icon.accent', style['text.accent']);
  ensure('title_bar.background', style['surface.background']);
  ensure('status_bar.background', style['surface.background']);
  ensure('toolbar.background', editorBg);
  ensure('tab_bar.background', style['surface.background']);
  ensure('tab.active_background', editorBg);
  ensure('tab.inactive_background', style['tab_bar.background']);
  ensure('tab.active_foreground', editorFg);
  ensure('tab.inactive_foreground', style['text.muted']);
  ensure('terminal.background', editorBg);
  ensure('terminal.foreground', editorFg);
  ensure('editor.line_number', style['text.muted']);
  ensure('editor.active_line_number', editorFg);
  ensure('editor.active_line.background', shift(editorBg, 0.06, dark));
  ensure('version_control.added', '#27a657ff');
  ensure('version_control.modified', '#d3b020ff');
  ensure('version_control.deleted', '#e06c76ff');

  style['background.appearance'] = 'opaque';

  for (const [ezicodeKey, vscodeKey] of Object.entries(ANSI_MAP)) {
    const value = normalizeColor(colors[vscodeKey]);
    if (value) style[ezicodeKey] = value;
  }

  style.syntax = buildSyntax(theme, editorFg, dark);

  return {
    name: options.name || theme.name || 'Imported Theme',
    appearance,
    style,
    players: [
      {
        cursor: normalizeColor(colors['editorCursor.foreground']) || style['text.accent'] || editorFg,
        background: normalizeColor(colors['editorCursor.foreground']) || style['text.accent'] || editorFg,
        selection: normalizeColor(colors['editor.selectionBackground']) || withAlpha(style['text.accent'] || editorFg, 0.25),
      },
    ],
  };
}

/**
 * Build the theme *family* document ezicode loads from
 * `<data>/ezicode/themes/<id>.json`.
 */
function buildFamily({ name, author, themes }) {
  return {
    $schema: 'https://zed.dev/schema/themes/v0.1.0.json',
    name,
    author: author || 'Imported from a VS Code extension',
    themes,
  };
}

module.exports = {
  convertTheme,
  buildFamily,
  normalizeColor,
  blend,
  shift,
  withAlpha,
  scopeResolver,
  SYNTAX_SCOPES,
  COLOR_MAP,
};
