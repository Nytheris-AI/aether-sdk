'use strict';
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {spawnSync} = require('node:child_process');
const {archiveName, releaseBaseUrl, parseChecksum, sha256} = require('./release.js');

/** Where a version's binary is cached, per user and per target. */
function cacheDir(env, version, target) {
  let root = env.AETHER_MCP_CACHE_DIR;
  if (!root) {
    if (process.platform === 'win32') {
      root = path.join(env.LOCALAPPDATA || path.join(os.homedir(), 'AppData', 'Local'), 'aether-mcp', 'cache');
    } else if (process.platform === 'darwin') {
      root = path.join(os.homedir(), 'Library', 'Caches', 'aether-mcp');
    } else {
      root = path.join(env.XDG_CACHE_HOME || path.join(os.homedir(), '.cache'), 'aether-mcp');
    }
  }
  return path.join(root, version, target.triple);
}

async function download(url, what, version) {
  let res;
  try {
    res = await fetch(url);
  } catch (err) {
    throw new Error(`Could not download ${what} for aether-mcp v${version}: ${err.message}`);
  }
  if (!res.ok) {
    throw new Error(
      `Could not download ${what} for aether-mcp v${version} (HTTP ${res.status}). ` +
        `The release may not include a build for this machine: ${url}`,
    );
  }
  return Buffer.from(await res.arrayBuffer());
}

/**
 * Returns the path to the aether-mcp binary for `version`, downloading and verifying the release
 * archive on first use. Progress goes to `log` (stderr), never stdout, because stdout carries the
 * MCP protocol when an agent runs the server over stdio.
 */
async function ensureBinary({version, target, env = process.env, log = (msg) => process.stderr.write(`${msg}\n`)}) {
  const dir = cacheDir(env, version, target);
  const exe = path.join(dir, target.exe);
  if (fs.existsSync(exe)) return exe;

  const archive = archiveName(version, target);
  const base = releaseBaseUrl(version, env.AETHER_MCP_DOWNLOAD_BASE);
  log(`Downloading aether-mcp v${version} (${target.triple})...`);
  const archiveBytes = await download(`${base}/${archive}`, archive, version);
  const checksumText = (await download(`${base}/${archive}.sha256`, `${archive}.sha256`, version)).toString('utf8');

  const expected = parseChecksum(checksumText);
  const actual = sha256(archiveBytes);
  if (actual !== expected) {
    throw new Error(`Checksum mismatch for ${archive}: expected ${expected}, got ${actual}. Nothing was installed.`);
  }

  // Extract beside the final location and rename, so a crash or a concurrent run never leaves a
  // half-written binary where the cache check would find it.
  fs.mkdirSync(path.dirname(dir), {recursive: true});
  const staging = fs.mkdtempSync(`${dir}.partial-`);
  try {
    const archivePath = path.join(staging, archive);
    fs.writeFileSync(archivePath, archiveBytes);
    const run = spawnSync('tar', ['-xf', archivePath, '-C', staging], {encoding: 'utf8'});
    if (run.status !== 0) {
      throw new Error(`Could not unpack ${archive} (is \`tar\` installed?): ${run.stderr || run.error}`);
    }
    fs.rmSync(archivePath);
    if (!fs.existsSync(path.join(staging, target.exe))) {
      throw new Error(`${archive} did not contain ${target.exe}.`);
    }
    if (process.platform !== 'win32') fs.chmodSync(path.join(staging, target.exe), 0o755);
    fs.rmSync(dir, {recursive: true, force: true});
    fs.renameSync(staging, dir);
  } catch (err) {
    fs.rmSync(staging, {recursive: true, force: true});
    throw err;
  }
  return exe;
}

module.exports = {ensureBinary, cacheDir};
