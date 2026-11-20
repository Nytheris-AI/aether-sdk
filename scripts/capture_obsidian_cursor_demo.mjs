#!/usr/bin/env node
// Records a split-screen demo of Obsidian (Aether Memory Companion) and the Cursor Agents window
// (aether-mcp over MCP) working on the same throwaway vault, on invisible X displays.
//
// Needs: Xvfb, flatpak Obsidian (md.obsidian.Obsidian), Cursor (signed in, NOT already running),
// ffmpeg, a built obsidian-aether-plugin/ and target/release/aether-mcp.
// Privacy: Cursor runs with a fake HOME so your global ~/.cursor/mcp.json (and any real vault) is never loaded,
// and its sidebar (account name, other projects, chat history) is hidden before recording starts.
// The agent chat uses your Cursor account and sends the demo prompts and tool results to its model provider.
//
// Usage: node scripts/capture_obsidian_cursor_demo.mjs <out_dir>
import {spawn, spawnSync} from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {Page, run, sleep} from './capture_lib.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const out = path.resolve(process.argv[2] ?? 'demo-media');
const work = path.join(os.homedir(), '.cache', 'aether-demo'); // flatpak Obsidian can read $HOME, not /tmp
const VAULT_NAME = 'vault'; // stable folder name so Cursor keeps its project approvals between runs
const vault = path.join(work, VAULT_NAME);
const PORT = 14771;
const BASE = `http://127.0.0.1:${PORT}`;
const OBS_CDP = 9333;
const CUR_CDP = 9444;
const FPS = 10;
const bin = path.join(root, 'target/release/aether-mcp');
const procs = [];
let keepWork = false;

function start(cmd, args, env = {}) {
  // Run from the demo folder so Cursor's git/Changes panel never picks up this repository.
  const p = spawn(cmd, args, {env: {...process.env, ...env}, stdio: 'ignore', cwd: work});
  procs.push(p);
  return p;
}

function writeVault() {
  fs.mkdirSync(vault, {recursive: true});
  for (const entry of fs.readdirSync(work)) {
    if (entry === VAULT_NAME) continue;
    fs.rmSync(path.join(work, entry), {recursive: true, force: true});
  }
  for (const entry of fs.readdirSync(vault)) fs.rmSync(path.join(vault, entry), {recursive: true, force: true});
  const plug = path.join(vault, '.obsidian/plugins/aether-memory');
  fs.mkdirSync(plug, {recursive: true});
  fs.mkdirSync(path.join(vault, '.cursor'), {recursive: true});
  fs.mkdirSync(path.join(work, 'home'), {recursive: true});
  fs.mkdirSync(path.join(work, 'frames'), {recursive: true});
  for (const f of ['main.js', 'manifest.json', 'styles.css']) {
    fs.copyFileSync(path.join(root, 'obsidian-aether-plugin', f), path.join(plug, f));
  }
  fs.writeFileSync(path.join(vault, '.obsidian/community-plugins.json'), '["aether-memory"]');
  fs.writeFileSync(path.join(plug, 'data.json'),
    JSON.stringify({endpointUrl: BASE, apiKey: '', liveSyncEnabled: true, syncDebounceMs: 500}));
  fs.writeFileSync(path.join(vault, '.cursor/mcp.json'),
    JSON.stringify({mcpServers: {'aether-demo': {command: bin, args: ['--vault', vault, '--watch-ms', '0']}}}));
  const notes = {
    'Profile/About-me.md': '---\naether_tier: hot\n---\n# About me\nI prefer Rust for CLI tools and short, plain-language answers.\n',
    'Projects/Roadmap.md': '# Roadmap\nShip the Obsidian plugin to the community directory.\n',
    'Meetings/2026-11-18.md': '# Standup\nDecision: use SQLite WAL mode for the local index.\n',
    'Journal-private.md': '---\naether_tier: private\n---\n# Journal\nTOPSECRET-MARKER: I am thinking about changing jobs. This entry lives only on this disk.\n',
  };
  for (const [rel, body] of Object.entries(notes)) {
    fs.mkdirSync(path.dirname(path.join(vault, rel)), {recursive: true});
    fs.writeFileSync(path.join(vault, rel), body);
  }
  fs.mkdirSync(path.join(work, 'ud'), {recursive: true});
  fs.writeFileSync(path.join(work, 'ud/obsidian.json'),
    JSON.stringify({vaults: {demo1: {path: vault, ts: Date.now(), open: true}}}));
}

async function post(p, body) {
  const r = await fetch(BASE + p, {method: 'POST', headers: {'content-type': 'application/json'}, body: JSON.stringify(body)});
  return r.json();
}

// Frame recorder with timestamps so two panes can be resampled onto one timeline.
function record(page, dir) {
  fs.mkdirSync(dir, {recursive: true});
  const frames = [];
  let running = true;
  const loop = (async () => {
    while (running) {
      const t = Date.now();
      const file = path.join(dir, `${frames.length}.jpg`);
      fs.writeFileSync(file, await page.frame(75));
      frames.push({t, file});
    }
  })();
  return {frames, stop: async () => { running = false; await loop; }};
}

function resample(frames, t0, t1, dir) {
  fs.mkdirSync(dir, {recursive: true});
  const n = Math.floor(((t1 - t0) / 1000) * FPS);
  let j = 0;
  for (let k = 0; k < n; k++) {
    const t = t0 + (k * 1000) / FPS;
    while (j + 1 < frames.length && frames[j + 1].t <= t) j++;
    fs.linkSync(frames[j].file, path.join(dir, `${String(k).padStart(5, '0')}.jpg`));
  }
  return n;
}

// Cursor helpers. The Agents window has no stable test ids, so we find elements by visible text.
const leafClick = (cur, re, last = false) => cur.eval(`(()=>{const m=[...document.querySelectorAll("div,span,button,a")].filter(e=>e.children.length===0&&${re}.test(e.textContent.trim())); const e=${last ? 'm[m.length-1]' : 'm[0]'}; e?.click(); return !!e})()`);
// Some menu items ignore synthetic DOM clicks; click the element's centre with a real mouse event instead.
async function mouseClickText(cur, re) {
  const at = await cur.eval(`(()=>{const m=[...document.querySelectorAll("div,span,button,a")].filter(e=>e.children.length===0&&${re}.test(e.textContent.trim())); const e=m[m.length-1]; if(!e) return null; const r=e.getBoundingClientRect(); return {x:r.x+r.width/2,y:r.y+r.height/2}})()`);
  if (at) await cur.mouseClick(at.x, at.y);
  return !!at;
}
const buttonClick = (cur, label) => cur.eval(`(()=>{const b=[...document.querySelectorAll("button,[role=button],a")].find(e=>(e.getAttribute("aria-label")||e.textContent).trim()==="${label}"); b?.click(); return !!b})()`);

async function selectWorkspace(cur) {
  // The repo picker is the control just before "This Computer"; its label is whatever repo was used last.
  await cur.eval('(()=>{const l=[...document.querySelectorAll("div,span,button")].filter(e=>e.children.length===0); const i=l.findIndex(e=>e.textContent.trim()==="This Computer"); l[i-1]?.click()})()');
  await sleep(800);
  // A synthetic DOM click is ignored by this menu item; a real mouse event works.
  const item = await cur.eval(`(()=>{const m=[...document.querySelectorAll("div,span,button,a")].filter(e=>e.children.length===0&&/^${VAULT_NAME}$/.test(e.textContent.trim())); const e=m[m.length-1]; if(!e) return null; const r=e.getBoundingClientRect(); return {x:r.x+r.width/2,y:r.y+r.height/2}})()`);
  if (!item) throw new Error(`workspace ${VAULT_NAME} not offered by Cursor's repo picker`);
  await cur.mouseClick(item.x, item.y);
  await sleep(1500);
}

// Select the demo vault, make sure its project-level MCP server is on, open a fresh chat, hide the sidebar.
async function prepareCursor(cur) {
  await buttonClick(cur, 'Show Sidebar'); // a previous run leaves it hidden
  await sleep(800);
  await buttonClick(cur, 'New Chat'); // Cursor reopens the last chat, which has no repo picker
  await sleep(1500);
  await selectWorkspace(cur);
  // Warm-up chat so Cursor treats the demo vault as the active project (not recorded).
  await ask(cur, 'Reply with the single word OK.');
  await buttonClick(cur, 'Customize');
  await sleep(2500);
  if (!(await leafClick(cur, '/^aether-demo$/'))) {
    // The project scope is unchecked: open the scope picker, expand it, and tick the demo vault workspace(s).
    await cur.eval('(()=>{const t=[...document.querySelectorAll("button")].find(b=>b.textContent.trim()==="Plugins"); t?.parentElement?.parentElement?.querySelector("button")?.click(); return !!t})()');
    await sleep(1000);
    await mouseClickText(cur, '/^\\d+ more$/');
    await sleep(1000);
    const row = await cur.eval(`(()=>{const e=[...document.querySelectorAll("div,span")].find(e=>e.children.length===0&&e.textContent.trim()==="${VAULT_NAME}"); if(!e) return null; let r=e; while(r.parentElement&&r.getBoundingClientRect().width<240) r=r.parentElement; const b=r.getBoundingClientRect(); return {x:b.right-20,y:b.top+b.height/2}})()`);
    if (!row) throw new Error(`workspace ${VAULT_NAME} not in Cursor's scope picker`);
    await cur.mouseClick(row.x, row.y);
    await sleep(1500);
    await cur.key('Escape', 'Escape', 27);
    await sleep(2500);
    if (!(await leafClick(cur, '/^aether-demo$/'))) throw new Error('aether-demo MCP server not listed in Cursor');
  }
  await sleep(2000);
  const state = await cur.eval(`(()=>{const sw=[...document.querySelectorAll('[role=switch]')]; let flipped=0; for(const s of sw){ if(s.getAttribute('aria-checked')!=='true'){ s.click(); flipped++; } } return {total:sw.length, flipped}})()`);
  console.log('cursor mcp switches:', JSON.stringify(state));
  if (!state.total) throw new Error('no MCP switches found in the aether-demo dialog');
  await sleep(3500);
  await cur.eval(`[...document.querySelectorAll("button")].find(b=>b.textContent.trim()==="Done")?.click()`);
  await sleep(800);
  await buttonClick(cur, 'New Chat');
  await sleep(1500);
  const label = await cur.eval('(()=>{const l=[...document.querySelectorAll("div,span,button")].filter(e=>e.children.length===0); const i=l.findIndex(e=>e.textContent.trim()==="This Computer"); return l[i-1]?.textContent.trim()||""})()');
  if (!label.includes(work)) await selectWorkspace(cur); // Cursor shows the full path when several repos share a name
  await cur.key('Escape', 'Escape', 27); // never record the repo menu: it lists your other workspaces
  // Cosmetic only: the repo label can show your home path, so show a neutral name in the recording.
  await cur.eval('(()=>{const l=[...document.querySelectorAll("div,span,button")].filter(e=>e.children.length===0); const i=l.findIndex(e=>e.textContent.trim()==="This Computer"); if(l[i-1]) l[i-1].textContent="aether-demo-vault"})()');
  await buttonClick(cur, 'Hide Sidebar');
  await sleep(800);
  await cur.viewport(1280, 800);
  await sleep(800);
}

const conv = (cur) => cur.eval('document.body.innerText');

async function ask(cur, prompt) {
  const before = await conv(cur);
  await cur.eval('document.querySelector("[contenteditable=true],textarea")?.focus()');
  await cur.type(prompt, {enter: true});
  let last = before;
  let stable = 0;
  let changed = false;
  for (let i = 0; i < 150; i++) {
    await sleep(1000);
    const text = await conv(cur);
    if (text !== before) changed = true;
    stable = text === last ? stable + 1 : 0;
    last = text;
    if (changed && stable >= 6) return text;
  }
  throw new Error(`Cursor agent did not finish: ${prompt}`);
}

// Agents are nondeterministic: retry once with a more explicit prompt until `ok(text)` holds.
async function askUntil(cur, prompts, ok, what) {
  for (const prompt of prompts) {
    const text = await ask(cur, prompt);
    const reply = text.slice(text.lastIndexOf(prompt) + prompt.length); // only what came after this prompt
    if (ok(reply)) return reply;
  }
  throw new Error(`agent did not ${what}`);
}

async function main() {
  if (spawnSync('pgrep', ['-x', 'cursor']).status === 0) {
    throw new Error('Cursor is already running; quit it fully first so the debug port and fake HOME apply.');
  }
  fs.mkdirSync(out, {recursive: true});
  writeVault();
  start('Xvfb', [':99', '-screen', '0', '1280x800x24']);
  start('Xvfb', [':98', '-screen', '0', '1920x1080x24']);
  await sleep(1000);
  let mcp = start(bin, ['--vault', vault, '--http-addr', `127.0.0.1:${PORT}`, '--http-only']);
  await sleep(2500);
  start('flatpak', ['run', 'md.obsidian.Obsidian', `--user-data-dir=${path.join(work, 'ud')}`,
    `--remote-debugging-port=${OBS_CDP}`, '--no-sandbox', '--disable-gpu'], {DISPLAY: ':99'});
  start('cursor', [`--remote-debugging-port=${CUR_CDP}`, '--no-sandbox', '--disable-gpu', '--new-window', vault],
    {DISPLAY: ':98', HOME: path.join(work, 'home'), XDG_CONFIG_HOME: path.join(os.homedir(), '.config')});

  const frames = path.join(work, 'frames');
  const obs = new Page({port: OBS_CDP, out, frames});
  const cur = new Page({port: CUR_CDP, out, frames});
  await sleep(8000);
  await obs.connect('New tab');
  await cur.connect('Cursor Agents');
  await obs.viewport(1280, 800);

  // Obsidian: trust the vault, wait for the plugin, open a note.
  await sleep(1000);
  await obs.eval('app.workspace.openLinkText("Projects/Roadmap.md", "", false)');
  await sleep(1500);
  await obs.eval('[...document.querySelectorAll("button")].find(b=>b.textContent.includes("Trust author"))?.click()');
  for (let i = 0; i < 20 && !(await obs.eval('!!app.plugins.plugins["aether-memory"]')); i++) await sleep(500);
  await sleep(1500);
  await obs.palette('Sync vault with Aether');
  await sleep(2500);

  try {
    await prepareCursor(cur);
  } catch (err) {
    await cur.shot('cursor-setup-failed.png'); // delete before sharing: it can show other workspace names
    throw err;
  }
  if (process.env.SETUP_ONLY) {
    await cur.shot('setup-check.png');
    return;
  }

  const captions = [];
  const mark = (text) => captions.push({t: Date.now(), text});
  const recObs = record(obs, path.join(work, 'obs'));
  const recCur = record(cur, path.join(work, 'cur'));
  const tStart = Date.now();

  mark('1. Write in Obsidian. Aether Memory Companion syncs the note to aether-mcp automatically.');
  await sleep(1500);
  await obs.eval(`(async()=>{ const f = await app.vault.create("Projects/Release-plan.md", "# Release plan\\nDecision: ship 0.4.1 on Friday after the BRAT soak. Owner: Lia. Rollback: pull the tag.\\n"); await app.workspace.getLeaf().openFile(f); })()`);
  await sleep(4000);

  mark('2. Ask the Cursor agent. It recalls the note through the aether_recall MCP tool.');
  const t2 = await askUntil(cur, [
    'Use the aether_recall tool from the aether-demo MCP server to find our release plan, then summarize it in one sentence.',
    'Please actually call the aether_recall tool (aether-demo MCP server) with query "release plan" and quote what it returns.',
  ], (t) => /Friday|0\.4\.1/.test(t), 'recall the release plan');

  mark('3. Notes marked private never leave the vault. The agent cannot find them.');
  await obs.eval('app.workspace.openLinkText("Journal-private.md", "", false)');
  await sleep(2000);
  const t3 = await askUntil(cur, [
    'Now use aether_recall to search for my journal entry about changing jobs. Tell me what you found.',
    'Please actually call the aether_recall tool with query "journal changing jobs" and tell me exactly what it returned.',
  ], (t) => /aether_recall|recall/i.test(t), 'call aether_recall for the private note');
  if (/TOPSECRET/.test(t3)) throw new Error('private note leaked to the agent');
  const hit = await post('/v1/recall', {query: 'journal changing jobs', limit: 5});
  if (JSON.stringify(hit).includes('TOPSECRET')) throw new Error('private note leaked into recall');

  mark('4. The agent proposes a memory with aether_stage_memory. It waits for your approval.');
  const FACT = 'Release notes should be short and written in plain language.';
  const stagePrompts = [
    `Use aether_stage_memory from aether-demo to propose this fact for memory: ${FACT}`,
    `Please actually call the aether_stage_memory tool now, with proposed_fact "${FACT}". Do not just describe it.`,
  ];
  let pending = 0;
  for (const prompt of stagePrompts) {
    await ask(cur, prompt);
    for (let i = 0; i < 10 && !pending; i++) {
      pending = (await (await fetch(`${BASE}/v1/inbox`)).json()).pending.length;
      if (!pending) await sleep(1000);
    }
    if (pending) break;
  }
  if (!pending) throw new Error('no staged candidate reached the inbox');

  mark('5. You review it in Obsidian and approve. Nothing becomes memory without you.');
  await obs.palette('Open Aether inbox review');
  await sleep(2500);
  await obs.eval('[...document.querySelectorAll("button")].find(b=>b.textContent.includes("Approve"))?.click()');
  await sleep(3000);

  mark('6. The approved memory is now recallable by the agent.');
  await askUntil(cur, [
    'Use aether_recall to answer: how should release notes be written?',
    'Please actually call the aether_recall tool with query "release notes" and answer from what it returns.',
  ], (t) => /plain/i.test(t), 'recall the approved memory');
  const approved = await post('/v1/recall', {query: 'release notes', limit: 5});
  if (!JSON.stringify(approved).includes('plain language')) throw new Error('approved memory is not recallable on the server');
  await sleep(2000);
  const tEnd = Date.now();

  await recObs.stop();
  await recCur.stop();
  const timeline = {captions, obs: recObs.frames, cur: recCur.frames, tStart, tEnd};
  fs.writeFileSync(path.join(work, 'timeline.json'), JSON.stringify(timeline));
  composeOrKeep(timeline);
}

function composeOrKeep(t) {
  try {
    compose(t.captions, t.obs, t.cur, t.tStart, t.tEnd);
  } catch (err) {
    keepWork = true; // frames and timeline stay in `work`; rerun with COMPOSE_ONLY=1 after fixing the ffmpeg error
    throw err;
  }
}

function compose(captions, obsFrames, curFrames, t0, t1) {
  const dirA = path.join(work, 'a');
  const dirB = path.join(work, 'b');
  fs.rmSync(dirA, {recursive: true, force: true});
  fs.rmSync(dirB, {recursive: true, force: true});
  resample(obsFrames, t0, t1, dirA);
  resample(curFrames, t0, t1, dirB);
  // ffmpeg filter syntax chokes on brackets in variable-font file names, so copy fonts to plain paths.
  const copyFont = (pattern, name) => {
    const dest = path.join(work, name);
    fs.copyFileSync(run('fc-match', ['-f', '%{file}', pattern]).trim(), dest);
    return dest;
  };
  const font = copyFont('sans:bold', 'font-bold.ttf');
  const fontReg = copyFont('sans', 'font-regular.ttf');
  const draw = captions.map((c, i) => {
    const file = path.join(work, `cap${i}.txt`);
    fs.writeFileSync(file, c.text);
    const a = (c.t - t0) / 1000;
    const b = ((captions[i + 1]?.t ?? t1) - t0) / 1000;
    return `drawtext=fontfile=${font}:textfile=${file}:fontcolor=white:fontsize=30:x=45:y=34:enable='between(t,${a},${b})'`;
  });
  const labels = [
    `drawtext=fontfile=${fontReg}:text='Obsidian + Aether Memory Companion':fontcolor=0xaaaaaa:fontsize=22:x=45:y=770`,
    `drawtext=fontfile=${fontReg}:text='Cursor Agent + aether-mcp (MCP)':fontcolor=0xaaaaaa:fontsize=22:x=1125:y=770`,
  ];
  // Obsidian full window; Cursor cropped to its chat column (the right half is an empty Changes panel).
  const filter = `[0:v]scale=1060:662[a];[1:v]crop=in_w/2:in_h:0:0,scale=530:662,pad=550:662:20:0[b];[a][b]hstack=inputs=2,pad=1700:832:45:100:color=0x111111,${[...draw, ...labels].join(',')}`;
  const inputs = ['-framerate', String(FPS), '-i', path.join(dirA, '%05d.jpg'), '-framerate', String(FPS), '-i', path.join(dirB, '%05d.jpg')];
  run('ffmpeg', ['-y', '-loglevel', 'error', ...inputs, '-filter_complex', filter, '-c:v', 'libvpx-vp9', '-crf', '38', '-b:v', '0', '-an', path.join(out, 'obsidian-cursor-demo.webm')]);
  run('ffmpeg', ['-y', '-loglevel', 'error', ...inputs, '-filter_complex', filter, '-c:v', 'libx264', '-crf', '30', '-pix_fmt', 'yuv420p', '-movflags', '+faststart', '-an', path.join(out, 'obsidian-cursor-demo.mp4')]);
  run('ffmpeg', ['-y', '-loglevel', 'error', ...inputs, '-filter_complex', filter, '-ss', String((captions[3].t - t0) / 1000 + 4), '-frames:v', '1', path.join(out, 'obsidian-cursor-demo-poster.png')]);
}

try {
  if (process.env.COMPOSE_ONLY) {
    fs.mkdirSync(out, {recursive: true});
    keepWork = true;
    composeOrKeep(JSON.parse(fs.readFileSync(path.join(work, 'timeline.json'), 'utf8')));
    keepWork = false;
  } else {
    await main();
  }
} finally {
  if (!process.env.COMPOSE_ONLY) {
    spawnSync('flatpak', ['kill', 'md.obsidian.Obsidian']);
    for (const p of procs) p.kill('SIGKILL');
    spawnSync('pkill', ['-9', '-x', 'cursor']); // Electron children outlive the launcher
    await sleep(2000);
  }
  if (!keepWork && !process.env.KEEP_WORK && fs.existsSync(vault)) {
    for (const entry of fs.readdirSync(work)) {
      if (entry !== VAULT_NAME) fs.rmSync(path.join(work, entry), {recursive: true, force: true});
    }
    for (const entry of fs.readdirSync(vault)) fs.rmSync(path.join(vault, entry), {recursive: true, force: true});
  }
}
process.exit(0);
