'use strict';
// VS Code theme and manifest files are JSON *with comments* (and the odd
// trailing comma). `JSON.parse` rejects both, so strip them first.

/**
 * @param {string} text
 * @returns {string} plain JSON
 */
function stripJsonComments(text) {
  let out = '';
  let inString = false;
  let inLineComment = false;
  let inBlockComment = false;

  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    const next = text[i + 1];

    if (inLineComment) {
      if (ch === '\n') {
        inLineComment = false;
        out += ch;
      }
      continue;
    }
    if (inBlockComment) {
      if (ch === '*' && next === '/') {
        inBlockComment = false;
        i++;
      }
      continue;
    }
    if (inString) {
      out += ch;
      if (ch === '\\') {
        out += next ?? '';
        i++;
      } else if (ch === '"') {
        inString = false;
      }
      continue;
    }
    if (ch === '"') {
      inString = true;
      out += ch;
      continue;
    }
    if (ch === '/' && next === '/') {
      inLineComment = true;
      i++;
      continue;
    }
    if (ch === '/' && next === '*') {
      inBlockComment = true;
      i++;
      continue;
    }
    out += ch;
  }
  return out;
}

/** Remove trailing commas (`[1,2,]`, `{"a":1,}`) that JSON.parse rejects. */
function stripTrailingCommas(text) {
  let out = '';
  let inString = false;
  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    if (inString) {
      out += ch;
      if (ch === '\\') {
        out += text[i + 1] ?? '';
        i++;
      } else if (ch === '"') {
        inString = false;
      }
      continue;
    }
    if (ch === '"') {
      inString = true;
      out += ch;
      continue;
    }
    if (ch === ',') {
      let j = i + 1;
      while (j < text.length && /\s/.test(text[j])) j++;
      if (text[j] === '}' || text[j] === ']') continue; // drop the comma
    }
    out += ch;
  }
  return out;
}

/**
 * Parse JSONC (JSON with comments / trailing commas).
 * @param {string|Buffer} text
 * @param {string} [label] used in error messages
 */
function parse(text, label = '<jsonc>') {
  const source = Buffer.isBuffer(text) ? text.toString('utf8') : text;
  const cleaned = stripTrailingCommas(stripJsonComments(source.replace(/^\uFEFF/, '')));
  try {
    return JSON.parse(cleaned);
  } catch (err) {
    throw new Error(`failed to parse ${label}: ${err.message}`);
  }
}

module.exports = { parse, stripJsonComments, stripTrailingCommas };
