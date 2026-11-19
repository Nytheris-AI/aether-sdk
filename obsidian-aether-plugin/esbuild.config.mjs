import fs from 'node:fs';
import {createRequire} from 'node:module';
import path from 'node:path';
import {fileURLToPath} from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);
const prod = process.argv[2] === 'production';

async function loadEsbuild() {
  try {
    return await import('esbuild');
  } catch {
    return null;
  }
}

function loadLocalTypescript() {
  try {
    return require('typescript');
  } catch (err) {
    const detail = err instanceof Error ? err.message : String(err);
    throw new Error(
      `Neither local 'esbuild' nor local 'typescript' is installed. Run 'npm install' in obsidian-aether-plugin (${detail}).`,
    );
  }
}

function bundleWithTypescriptFallback() {
  const ts = loadLocalTypescript();
  const entryAbs = path.resolve(__dirname, 'src', 'obsidian-entry.ts');
  const modules = new Map();
  const order = [];

  function visit(filePath) {
    const abs = path.resolve(filePath);
    if (modules.has(abs)) {
      return;
    }
    const source = fs.readFileSync(abs, 'utf8');
    const info = ts.preProcessFile(source, true, true);
    const deps = new Map();
    for (const imp of info.importedFiles) {
      if (imp.fileName.startsWith('.')) {
        const depAbs = path.resolve(path.dirname(abs), imp.fileName);
        deps.set(imp.fileName, depAbs);
        visit(depAbs);
      }
    }
    const transpiled = ts.transpileModule(source, {
      fileName: abs,
      compilerOptions: {
        target: ts.ScriptTarget.ES2022,
        module: ts.ModuleKind.CommonJS,
        esModuleInterop: true,
      },
    });
    modules.set(abs, {code: transpiled.outputText, deps});
    order.push(abs);
  }

  visit(entryAbs);
  const ids = new Map(order.map((abs, idx) => [abs, idx]));
  const moduleEntries = order
    .map((abs) => {
      const mod = modules.get(abs);
      const depMap = JSON.stringify(
        Object.fromEntries([...mod.deps.entries()].map(([spec, depAbs]) => [spec, ids.get(depAbs)])),
      );
      return `${ids.get(abs)}: { depMap: ${depMap}, fn: function(require, module, exports) {\n${mod.code}\n} }`;
    })
    .join(',\n');

  const bundle = `'use strict';
const __modules = {
${moduleEntries}
};
const __cache = {};
function __load(id) {
  if (__cache[id]) return __cache[id].exports;
  const mod = { exports: {} };
  __cache[id] = mod;
  const entry = __modules[id];
  const localRequire = (spec) => {
    if (Object.prototype.hasOwnProperty.call(entry.depMap, spec)) {
      return __load(entry.depMap[spec]);
    }
    return require(spec);
  };
  entry.fn(localRequire, mod, mod.exports);
  return mod.exports;
}
module.exports = __load(${ids.get(entryAbs)});
`;
  fs.writeFileSync(path.resolve(__dirname, 'main.js'), bundle, 'utf8');
}

const esbuild = await loadEsbuild();
if (esbuild) {
  const ctx = await esbuild.context({
    entryPoints: [path.resolve(__dirname, 'src', 'obsidian-entry.ts')],
    bundle: true,
    external: ['obsidian', 'electron', 'crypto', 'node:crypto'],
    format: 'cjs',
    target: 'es2022',
    outfile: path.resolve(__dirname, 'main.js'),
    sourcemap: prod ? false : 'inline',
    treeShaking: true,
    minify: prod,
  });

  if (prod) {
    await ctx.rebuild();
    await ctx.dispose();
  } else {
    await ctx.watch();
  }
} else {
  if (!prod) {
    throw new Error('esbuild is required for watch mode (`npm run dev`).');
  }
  bundleWithTypescriptFallback();
}
