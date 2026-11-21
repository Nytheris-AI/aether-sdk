'use strict';
const crypto = require('node:crypto');

const DEFAULT_DOWNLOAD_BASE = 'https://github.com/muninn-hq/aether-sdk/releases/download';

const TARGETS = {
  'linux-x64': {triple: 'x86_64-unknown-linux-gnu', ext: 'tar.gz', exe: 'aether-mcp'},
  'linux-arm64': {triple: 'aarch64-unknown-linux-gnu', ext: 'tar.gz', exe: 'aether-mcp'},
  'darwin-x64': {triple: 'x86_64-apple-darwin', ext: 'tar.gz', exe: 'aether-mcp'},
  'darwin-arm64': {triple: 'aarch64-apple-darwin', ext: 'tar.gz', exe: 'aether-mcp'},
  'win32-x64': {triple: 'x86_64-pc-windows-msvc', ext: 'zip', exe: 'aether-mcp.exe'},
};

/** The release target for a Node `process.platform` and `process.arch`. */
function targetFor(platform, arch) {
  const target = TARGETS[`${platform}-${arch}`];
  if (!target) {
    const supported = Object.keys(TARGETS).join(', ');
    throw new Error(
      `aether-mcp is not supported on ${platform} ${arch} (supported: ${supported}). ` +
        'Build it from source: https://github.com/muninn-hq/aether-sdk',
    );
  }
  return target;
}

function allTargets() {
  return Object.values(TARGETS);
}

/** Archive file name used by the release workflow. */
function archiveName(version, target) {
  return `aether-mcp-v${version}-${target.triple}.${target.ext}`;
}

/** Names of the release assets (archive and checksum for every target) not in `assetNames`. */
function missingAssets(version, assetNames) {
  const have = new Set(assetNames);
  return allTargets()
    .flatMap((target) => [archiveName(version, target), `${archiveName(version, target)}.sha256`])
    .filter((name) => !have.has(name));
}

/** Directory URL holding a release's assets. `base` overrides the GitHub download URL. */
function releaseBaseUrl(version, base) {
  return `${(base || DEFAULT_DOWNLOAD_BASE).replace(/\/+$/, '')}/v${version}`;
}

/** The hex digest from a `sha256sum` style checksum file. */
function parseChecksum(text) {
  const match = /^([0-9a-fA-F]{64})(\s|$)/.exec(text.trim());
  if (!match) throw new Error('Could not read a SHA-256 checksum from the release checksum file.');
  return match[1].toLowerCase();
}

function sha256(buffer) {
  return crypto.createHash('sha256').update(buffer).digest('hex');
}

module.exports = {targetFor, allTargets, missingAssets, archiveName, releaseBaseUrl, parseChecksum, sha256};
