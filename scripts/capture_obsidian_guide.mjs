#!/usr/bin/env node
// Captures real footage of the Aether Memory Companion running in Obsidian Desktop.
// Needs: Xvfb, flatpak Obsidian (md.obsidian.Obsidian), ffmpeg, ImageMagick `convert`,
// a built obsidian-aether-plugin/ (npm run build) and target/release/aether-mcp.
// Usage: node scripts/capture_obsidian_guide.mjs <out_dir>
import {spawn, spawnSync} from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {Page, run, sleep} from './capture_lib.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const out = path.resolve(process.argv[2] ?? 'guide-media');
const work = path.join(os.homedir(), '.cache', 'aether-guide'); // flatpak Obsidian can read $HOME, not /tmp
const vault = path.join(work, 'vault');
const frames = path.join(work, 'frames');
const PORT = 14771;
const CDP = 9333;
const BASE = `http://127.0.0.1:${PORT}`;
const procs = [];

function start(cmd, args, env = {}) {
  const p = spawn(cmd, args, {env: {...process.env, ...env}, stdio: 'ignore'});
  procs.push(p);
  return p;
}

function writeVault() {
  fs.rmSync(work, {recursive: true, force: true});
  const plug = path.join(vault, '.obsidian/plugins/aether-memory');
  fs.mkdirSync(plug, {recursive: true});
  fs.mkdirSync(frames, {recursive: true});
  for (const f of ['main.js', 'manifest.json', 'styles.css']) {
    fs.copyFileSync(path.join(root, 'obsidian-aether-plugin', f), path.join(plug, f));
  }
  fs.writeFileSync(path.join(vault, '.obsidian/community-plugins.json'), '["aether-memory"]');
  fs.writeFileSync(
    path.join(plug, 'data.json'),
    JSON.stringify({endpointUrl: BASE, apiKey: '', liveSyncEnabled: true, syncDebounceMs: 500}),
  );
  const notes = {
    'Projects/Roadmap.md': '# Roadmap\nShip the Obsidian plugin to the community directory.\nPrefer Rust for CLI tools.\n',
    'Meetings/2026-11-18.md': '# Standup\nDecision: use SQLite WAL mode for the local index.\n',
    'Welcome.md': '# Welcome\nThis vault is shared with my AI agents through Aether.\n',
  };
  for (const [rel, body] of Object.entries(notes)) {
    fs.mkdirSync(path.dirname(path.join(vault, rel)), {recursive: true});
    fs.writeFileSync(path.join(vault, rel), body);
  }
  fs.mkdirSync(path.join(work, 'ud'), {recursive: true});
  fs.writeFileSync(
    path.join(work, 'ud/obsidian.json'),
    JSON.stringify({vaults: {guide1: {path: vault, ts: Date.now(), open: true}}}),
  );
}

async function post(p, body) {
  const r = await fetch(BASE + p, {method: 'POST', headers: {'content-type': 'application/json'}, body: JSON.stringify(body)});
  return r.json();
}

function terminalImage(name, title, text) {
  const txt = path.join(work, `${name}.txt`);
  fs.writeFileSync(txt, `$ ${title}\n${text}`.split('\n').flatMap((l) => l.match(/.{1,88}(\s+|$)|\S+/g) ?? ['']).join('\n'));
  run('convert', ['-background', '#1e1e1e', '-fill', '#d4d4d4', '-font', 'Nimbus-Mono-PS-Regular', '-pointsize', '20',
    '-bordercolor', '#1e1e1e', '-border', '24', `label:@${txt}`, path.join(out, name)]);
}

function startMcp() {
  return start(path.join(root, 'target/release/aether-mcp'), ['--vault', vault, '--http-addr', `127.0.0.1:${PORT}`, '--http-only']);
}

async function main() {
  fs.mkdirSync(out, {recursive: true});
  writeVault();
  start('Xvfb', [':99', '-screen', '0', '1280x800x24']);
  await sleep(1000);
  let mcp = startMcp();
  await sleep(2000);
  start('flatpak', ['run', 'md.obsidian.Obsidian', `--user-data-dir=${path.join(work, 'ud')}`,
    `--remote-debugging-port=${CDP}`, '--no-sandbox', '--disable-gpu'], {DISPLAY: ':99'});
  for (let i = 0; i < 40; i++) {
    try { await fetch(`http://127.0.0.1:${CDP}/json`); break; } catch { await sleep(500); }
  }
  await sleep(8000);
  const page = new Page({port: CDP, out, frames});
  await page.connect('New tab');
  await page.send('Emulation.setDeviceMetricsOverride', {width: 1280, height: 800, deviceScaleFactor: 1, mobile: false});
  await sleep(1500);
  // First-run trust prompt for vaults that ship community plugins.
  await page.eval(`app.workspace.openLinkText("Welcome.md", "", false)`);
  await sleep(1500);
  await page.shot('00-trust-vault.png');
  await page.eval('[...document.querySelectorAll("button")].find(b=>b.textContent.includes("Trust author"))?.click()');
  for (let i = 0; i < 20 && !(await page.eval('!!app.plugins.plugins["aether-memory"]')); i++) await sleep(500);
  await sleep(1500);
  const status = () => page.eval('[...document.querySelectorAll(".status-bar-item")].map(e=>e.textContent).find(t=>t.includes("Aether"))');

  // 1-2. Settings (Obsidian opens them in their own window) and Test Connection.
  await page.eval('app.setting.open()');
  await sleep(2500);
  const settings = new Page({port: CDP, out, frames});
  await settings.connect('Settings');
  await settings.send('Emulation.setDeviceMetricsOverride', {width: 1000, height: 760, deviceScaleFactor: 1, mobile: false});
  await settings.eval('[...document.querySelectorAll(".vertical-tab-nav-item")].find(e=>e.textContent.includes("Aether Memory Companion")).click()');
  await sleep(800);
  await settings.shot('01-settings.png');
  await settings.eval('[...document.querySelectorAll("button")].find(b=>b.textContent==="Test Connection").click()');
  await sleep(1200);
  await settings.shot('02-test-connection.png');
  await page.eval('app.setting.close()');
  await sleep(500);

  // 3. First sync from the command palette.
  await page.clip('03-sync.webm', async () => {
    await sleep(800);
    await page.palette('Sync Vault with Aether');
    await sleep(3500);
  });
  await page.shot('04-synced.png');

  // 4. Private note stays on disk and out of agent recall.
  await page.clip('05-private-note.webm', async () => {
    await page.eval(`(async()=>{ const f = await app.vault.create("Journal-private.md", "---\\naether_tier: private\\n---\\n# Journal\\nTOPSECRET-MARKER lives only on this disk.\\n"); await app.workspace.getLeaf().openFile(f); })()`);
    await sleep(3500);
  });
  const hit = await post('/v1/recall', {query: 'TOPSECRET-MARKER', limit: 5});
  const miss = hit.dynamic_context.includes('TOPSECRET');
  if (miss || !fs.existsSync(path.join(vault, 'Journal-private.md'))) throw new Error('private note leaked or was deleted');
  terminalImage('06-agent-recall-private.png',
    `curl -s ${BASE}/v1/recall -d '{"query":"TOPSECRET-MARKER"}' | jq -r .dynamic_context`, hit.dynamic_context);
  const pub = await post('/v1/recall', {query: 'Rust CLI tools', limit: 3});
  terminalImage('07-agent-recall-public.png',
    `curl -s ${BASE}/v1/recall -d '{"query":"Rust CLI tools"}' | jq -r .dynamic_context`, pub.dynamic_context);

  // 5. Agent stages a memory, human approves it in the Inbox pane.
  await post('/v1/mcp', {jsonrpc: '2.0', id: 1, method: 'tools/call', params: {name: 'aether_stage_memory',
    arguments: {proposed_fact: 'User prefers short, plain-language release notes.', suggested_tier: 'warm'}}});
  await page.clip('08-inbox-approve.webm', async () => {
    await page.palette('Open Aether Inbox Review');
    await sleep(2000);
    await page.shot('09-inbox-pending.png');
    await page.eval('[...document.querySelectorAll("button")].find(b=>b.textContent.includes("Approve"))?.click()');
    await sleep(2500);
  });
  await page.shot('10-inbox-approved.png');

  // 6. Offline then recovery.
  mcp.kill();
  await sleep(800);
  await page.clip('11-offline-recovery.webm', async () => {
    await page.palette('Sync Vault with Aether');
    await sleep(2500);
    mcp = startMcp();
    await sleep(2000);
    await page.palette('Sync Vault with Aether');
    await sleep(3000);
  });
  console.log('final status:', await status());
}

try {
  await main();
} finally {
  spawnSync('flatpak', ['kill', 'md.obsidian.Obsidian']);
  for (const p of procs) p.kill();
  fs.rmSync(work, {recursive: true, force: true});
}
