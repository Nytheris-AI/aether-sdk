'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const {targetFor, allTargets, archiveName, releaseBaseUrl, parseChecksum, sha256} = require('../lib/release.js');

test('maps supported platforms to release targets', () => {
  assert.deepEqual(targetFor('linux', 'x64'), {triple: 'x86_64-unknown-linux-gnu', ext: 'tar.gz', exe: 'aether-mcp'});
  assert.equal(targetFor('linux', 'arm64').triple, 'aarch64-unknown-linux-gnu');
  assert.equal(targetFor('darwin', 'arm64').triple, 'aarch64-apple-darwin');
  assert.equal(targetFor('darwin', 'x64').triple, 'x86_64-apple-darwin');
  assert.deepEqual(targetFor('win32', 'x64'), {triple: 'x86_64-pc-windows-msvc', ext: 'zip', exe: 'aether-mcp.exe'});
});

test('rejects unsupported platforms with a helpful message', () => {
  assert.throws(() => targetFor('freebsd', 'x64'), /not supported.*freebsd.*x64/s);
  assert.throws(() => targetFor('win32', 'arm64'), /win32/);
});

test('builds archive names and urls that match the release workflow', () => {
  const target = targetFor('linux', 'x64');
  assert.equal(archiveName('0.1.1', target), 'aether-mcp-v0.1.1-x86_64-unknown-linux-gnu.tar.gz');
  assert.equal(
    releaseBaseUrl('0.1.1', undefined),
    'https://github.com/muninn-hq/aether-sdk/releases/download/v0.1.1',
  );
  assert.equal(releaseBaseUrl('0.1.1', 'https://mirror.example/dl/'), 'https://mirror.example/dl/v0.1.1');
});

test('parses sha256sum and PowerShell checksum files', () => {
  const hash = 'a'.repeat(64);
  assert.equal(parseChecksum(`${hash}  aether-mcp-v0.1.1-x.tar.gz\n`), hash);
  assert.equal(parseChecksum(`${hash.toUpperCase()}  file\r\n`), hash);
  assert.throws(() => parseChecksum('not a checksum'), /checksum/i);
});

test('hashes buffers', () => {
  assert.equal(sha256(Buffer.from('abc')), 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad');
});

test('reports release assets that are missing for any supported target', () => {
  const {missingAssets} = require('../lib/release.js');
  const all = [];
  for (const target of allTargets()) {
    const archive = archiveName('1.2.3', target);
    all.push(archive, `${archive}.sha256`);
  }
  assert.deepEqual(missingAssets('1.2.3', all), []);
  const withoutArm = all.filter((name) => !name.includes('aarch64-unknown-linux-gnu'));
  assert.deepEqual(missingAssets('1.2.3', withoutArm), [
    'aether-mcp-v1.2.3-aarch64-unknown-linux-gnu.tar.gz',
    'aether-mcp-v1.2.3-aarch64-unknown-linux-gnu.tar.gz.sha256',
  ]);
});
