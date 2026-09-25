'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');

const zip = require('../src/zip');

test('zip round-trips stored and deflated entries', () => {
  const files = [
    { name: 'extension/package.json', data: Buffer.from('{"name":"x"}') },
    { name: 'extension/big.txt', data: Buffer.from('compress me '.repeat(500)) },
    { name: 'extension/nested/dir/file.bin', data: Buffer.from([0, 1, 2, 250, 255]) },
  ];
  const archive = zip.write(files);
  const read = zip.readAll(archive);

  assert.equal(read.size, 3);
  for (const file of files) {
    assert.deepEqual(read.get(file.name), file.data, file.name);
  }
  // The repetitive file must actually have been deflated.
  const entry = zip.listEntries(archive).find((e) => e.name.endsWith('big.txt'));
  assert.equal(entry.method, 8);
  assert.ok(entry.compressedSize < entry.size);
});

test('rejects data that is not a zip', () => {
  assert.throws(() => zip.readAll(Buffer.from('definitely not a zip file')), /not a zip archive/);
});

test('crc32 matches the known check value', () => {
  assert.equal(zip.crc32(Buffer.from('123456789')), 0xcbf43926);
});
