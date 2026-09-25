'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');

const jsonc = require('../src/jsonc');

test('parses comments and trailing commas', () => {
  const value = jsonc.parse(`{
    // line comment
    "a": 1, /* block */
    "b": [1, 2, 3,],
  }`);
  assert.deepEqual(value, { a: 1, b: [1, 2, 3] });
});

test('does not mangle comment-like text inside strings', () => {
  const value = jsonc.parse('{"url": "https://example.com/a//b", "glob": "**/*.js", "q": "a,]"}');
  assert.equal(value.url, 'https://example.com/a//b');
  assert.equal(value.glob, '**/*.js');
  assert.equal(value.q, 'a,]');
});

test('handles escaped quotes and BOM', () => {
  const value = jsonc.parse('\uFEFF{"s": "he said \\"hi\\" // not a comment"}');
  assert.equal(value.s, 'he said "hi" // not a comment');
});

test('reports the file name on failure', () => {
  assert.throws(() => jsonc.parse('{oops}', 'theme.json'), /failed to parse theme.json/);
});
