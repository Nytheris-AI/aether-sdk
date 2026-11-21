'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const http = require('node:http');
const os = require('node:os');
const path = require('node:path');
const {spawnSync} = require('node:child_process');
const {ensureBinary, cacheDir} = require('../lib/install.js');
const {targetFor, archiveName, sha256} = require('../lib/release.js');

const VERSION = '9.9.9';

function makeRelease(root, target, {corrupt = false} = {}) {
  const stage = path.join(root, 'stage');
  fs.mkdirSync(stage, {recursive: true});
  fs.writeFileSync(path.join(stage, target.exe), '#!/bin/sh\necho fake-aether "$@"\n[ "$1" = fail ] && exit 3\nexit 0\n', {mode: 0o755});
  const archive = archiveName(VERSION, target);
  const dist = path.join(root, `v${VERSION}`);
  fs.mkdirSync(dist, {recursive: true});
  const run = spawnSync('tar', ['-czf', path.join(dist, archive), '-C', stage, target.exe]);
  assert.equal(run.status, 0, String(run.stderr));
  const digest = sha256(fs.readFileSync(path.join(dist, archive)));
  fs.writeFileSync(path.join(dist, `${archive}.sha256`), `${corrupt ? '0'.repeat(64) : digest}  ${archive}\n`);
}

function serve(root) {
  const hits = [];
  const server = http.createServer((req, res) => {
    hits.push(req.url);
    const file = path.join(root, req.url);
    if (!fs.existsSync(file)) {
      res.writeHead(404).end();
      return;
    }
    res.end(fs.readFileSync(file));
  });
  return new Promise((resolve) =>
    server.listen(0, '127.0.0.1', () => resolve({server, hits, base: `http://127.0.0.1:${server.address().port}`})),
  );
}

const linux = targetFor('linux', 'x64');
const skip = process.platform === 'win32' ? 'tar.gz fixture needs a unix shell' : false;

test('downloads, verifies, caches, and reuses the binary', {skip}, async () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'aether-npm-'));
  makeRelease(path.join(tmp, 'srv'), linux);
  const {server, hits, base} = await serve(path.join(tmp, 'srv'));
  try {
    const env = {AETHER_MCP_DOWNLOAD_BASE: base, AETHER_MCP_CACHE_DIR: path.join(tmp, 'cache')};
    const exe = await ensureBinary({version: VERSION, target: linux, env, log: () => {}});
    assert.equal(exe, path.join(cacheDir(env, VERSION, linux), linux.exe));
    assert.equal(spawnSync(exe, ['hi']).stdout.toString().trim(), 'fake-aether hi');

    const before = hits.length;
    await ensureBinary({version: VERSION, target: linux, env, log: () => {}});
    assert.equal(hits.length, before, 'second call must use the cache');
  } finally {
    server.close();
  }
});

test('refuses a binary whose checksum does not match', {skip}, async () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'aether-npm-'));
  makeRelease(path.join(tmp, 'srv'), linux, {corrupt: true});
  const {server, base} = await serve(path.join(tmp, 'srv'));
  try {
    const env = {AETHER_MCP_DOWNLOAD_BASE: base, AETHER_MCP_CACHE_DIR: path.join(tmp, 'cache')};
    await assert.rejects(ensureBinary({version: VERSION, target: linux, env, log: () => {}}), /checksum/i);
    assert.equal(fs.existsSync(cacheDir(env, VERSION, linux)), false, 'nothing is cached after a failure');
  } finally {
    server.close();
  }
});

test('explains a missing release asset', async () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'aether-npm-'));
  fs.mkdirSync(path.join(tmp, 'srv'));
  const {server, base} = await serve(path.join(tmp, 'srv'));
  try {
    const env = {AETHER_MCP_DOWNLOAD_BASE: base, AETHER_MCP_CACHE_DIR: path.join(tmp, 'cache')};
    await assert.rejects(
      ensureBinary({version: VERSION, target: linux, env, log: () => {}}),
      /could not download.*v9\.9\.9.*404/is,
    );
  } finally {
    server.close();
  }
});

test('launcher forwards arguments and exit codes and keeps stdout clean', {skip}, async () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'aether-npm-'));
  makeRelease(path.join(tmp, 'srv'), linux);
  const {server, base} = await serve(path.join(tmp, 'srv'));
  try {
    const env = {
      ...process.env,
      AETHER_MCP_VERSION: VERSION,
      AETHER_MCP_DOWNLOAD_BASE: base,
      AETHER_MCP_CACHE_DIR: path.join(tmp, 'cache'),
    };
    const launcher = path.join(__dirname, '..', 'bin', 'aether-mcp.js');
    const ok = await runAsync(process.execPath, [launcher, 'setup', '--yes'], env);
    assert.equal(ok.stdout.trim(), 'fake-aether setup --yes');
    assert.match(ok.stderr, /Downloading aether-mcp v9\.9\.9/);
    assert.equal(ok.code, 0);

    const failed = await runAsync(process.execPath, [launcher, 'fail'], env);
    assert.equal(failed.code, 3);
    assert.doesNotMatch(failed.stderr, /Downloading/, 'cached run does not download again');
  } finally {
    server.close();
  }
});

function runAsync(cmd, args, env) {
  return new Promise((resolve) => {
    const child = require('node:child_process').spawn(cmd, args, {env});
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (d) => (stdout += d));
    child.stderr.on('data', (d) => (stderr += d));
    child.on('close', (code) => resolve({code, stdout, stderr}));
  });
}
