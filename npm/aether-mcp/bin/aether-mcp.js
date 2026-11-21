#!/usr/bin/env node
'use strict';
const {spawn} = require('node:child_process');
const pkg = require('../package.json');
const {targetFor} = require('../lib/release.js');
const {ensureBinary} = require('../lib/install.js');

async function main() {
  // AETHER_MCP_VERSION pins a different aether-mcp release than the one this package was tested with.
  const version = process.env.AETHER_MCP_VERSION || pkg.aetherMcpVersion;
  const exe = await ensureBinary({version, target: targetFor(process.platform, process.arch)});

  const child = spawn(exe, process.argv.slice(2), {stdio: 'inherit'});
  for (const signal of ['SIGINT', 'SIGTERM']) {
    process.on(signal, () => child.kill(signal));
  }
  child.on('error', (err) => {
    process.stderr.write(`aether-mcp: could not start ${exe}: ${err.message}\n`);
    process.exit(1);
  });
  child.on('close', (code, signal) => process.exit(code ?? (signal ? 1 : 0)));
}

main().catch((err) => {
  process.stderr.write(`aether-mcp: ${err.message}\n`);
  process.exit(1);
});
