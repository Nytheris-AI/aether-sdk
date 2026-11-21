#!/usr/bin/env node
'use strict';
// Fails unless the aether-mcp release this package downloads has an archive and checksum for every
// supported target. Run before publishing so the package never points at an incomplete release.
const pkg = require('../package.json');
const {missingAssets} = require('../lib/release.js');

async function main() {
  const version = process.env.AETHER_MCP_VERSION || pkg.aetherMcpVersion;
  const headers = {Accept: 'application/vnd.github+json', 'User-Agent': 'aether-mcp-npm-release-check'};
  if (process.env.GITHUB_TOKEN) headers.Authorization = `Bearer ${process.env.GITHUB_TOKEN}`;
  const url = `https://api.github.com/repos/muninn-hq/aether-sdk/releases/tags/v${version}`;
  const res = await fetch(url, {headers});
  if (!res.ok) throw new Error(`No GitHub release v${version} found (HTTP ${res.status}): ${url}`);
  const release = await res.json();
  const missing = missingAssets(version, release.assets.map((asset) => asset.name));
  if (missing.length) {
    throw new Error(`Release v${version} is missing assets:\n  ${missing.join('\n  ')}`);
  }
  console.log(`Release v${version} has every build.`);
}

main().catch((err) => {
  console.error(err.message);
  process.exit(1);
});
