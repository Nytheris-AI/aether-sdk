// Shared helpers for the scripted capture scripts: Chrome DevTools Protocol driving of Electron apps
// (Obsidian, Cursor), screenshots, WebM clips, and small shell helpers.
import {spawnSync} from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export function run(cmd, args) {
  const r = spawnSync(cmd, args, {encoding: 'utf8'});
  if (r.status !== 0) throw new Error(`${cmd} failed: ${r.stderr}`);
  return r.stdout;
}

export class Page {
  constructor({port, out, frames}) {
    this.port = port;
    this.out = out;
    this.frames = frames;
  }

  async connect(titlePrefix = '') {
    let page;
    for (let i = 0; i < 40 && !page; i++) {
      try {
        const pages = await (await fetch(`http://127.0.0.1:${this.port}/json`)).json();
        page = pages.find((p) => p.type === 'page' && p.title.startsWith(titlePrefix));
      } catch {
        // debug port not up yet
      }
      if (!page) await sleep(500);
    }
    if (!page) throw new Error(`no page with title prefix "${titlePrefix}" on port ${this.port}`);
    this.ws = new WebSocket(page.webSocketDebuggerUrl);
    await new Promise((r) => (this.ws.onopen = r));
    this.id = 0;
    this.waiting = new Map();
    this.ws.onmessage = (e) => {
      const m = JSON.parse(e.data);
      this.waiting.get(m.id)?.(m);
    };
  }

  send(method, params = {}) {
    const id = ++this.id;
    return new Promise((resolve) => {
      this.waiting.set(id, resolve);
      this.ws.send(JSON.stringify({id, method, params}));
    });
  }

  async eval(expression) {
    const m = await this.send('Runtime.evaluate', {expression, awaitPromise: true, returnByValue: true});
    return m.result?.result?.value;
  }

  async viewport(width, height) {
    await this.send('Emulation.setDeviceMetricsOverride', {width, height, deviceScaleFactor: 1, mobile: false});
  }

  async shot(name) {
    const m = await this.send('Page.captureScreenshot', {format: 'png'});
    fs.writeFileSync(path.join(this.out, name), Buffer.from(m.result.data, 'base64'));
  }

  async frame(quality = 80) {
    const m = await this.send('Page.captureScreenshot', {format: 'jpeg', quality});
    return Buffer.from(m.result.data, 'base64');
  }

  async key(key, code, vk, modifiers = 0) {
    for (const type of ['keyDown', 'keyUp']) {
      await this.send('Input.dispatchKeyEvent', {type, key, code, windowsVirtualKeyCode: vk, modifiers});
    }
  }

  async mouseClick(x, y) {
    for (const type of ['mousePressed', 'mouseReleased']) {
      await this.send('Input.dispatchMouseEvent', {type, x, y, button: 'left', clickCount: 1});
    }
  }

  async type(text, {enter = false} = {}) {
    await this.send('Input.insertText', {text});
    if (enter) await this.key('Enter', 'Enter', 13);
  }

  async palette(command) {
    for (let attempt = 0; attempt < 4; attempt++) {
      await this.send('Page.bringToFront');
      await this.key('p', 'KeyP', 80, 2);
      await sleep(600);
      if (await this.eval('!!document.querySelector(".prompt")')) {
        await this.type(command);
        await sleep(600);
        await this.key('Enter', 'Enter', 13);
        return;
      }
      await this.key('Escape', 'Escape', 27);
      await sleep(800);
    }
    throw new Error('command palette did not open');
  }

  // Records the viewport as a WebM clip by polling screenshots while `scene` runs.
  async clip(name, scene) {
    fs.rmSync(this.frames, {recursive: true, force: true});
    fs.mkdirSync(this.frames, {recursive: true});
    let n = 0;
    let running = true;
    const t0 = Date.now();
    const loop = (async () => {
      while (running) {
        fs.writeFileSync(path.join(this.frames, `${String(n++).padStart(5, '0')}.jpg`), await this.frame());
      }
    })();
    await scene();
    running = false;
    await loop;
    const fps = n / ((Date.now() - t0) / 1000);
    run('ffmpeg', ['-y', '-loglevel', 'error', '-framerate', fps.toFixed(2), '-i', path.join(this.frames, '%05d.jpg'),
      '-c:v', 'libvpx-vp9', '-crf', '40', '-b:v', '0', '-an', path.join(this.out, name)]);
  }
}
