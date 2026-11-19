import fs from 'node:fs';
import path from 'node:path';
import {fileURLToPath} from 'node:url';

/**
 * @typedef {'main.js' | 'manifest.json' | 'versions.json' | 'styles.css' | 'README.md' | 'LICENSE'} ReleaseArtifactFileName
 * @typedef {'package.json' | 'tsconfig.json' | 'esbuild.config.mjs'} StandaloneConfigFileName
 * @typedef {ReleaseArtifactFileName | StandaloneConfigFileName} ReleaseFileName
 * @typedef {'release-artifact' | 'standalone-config'} FileRole
 * @typedef {'js' | 'json' | 'css' | 'text'} FileKind
 * @typedef {{
 *   readonly path: ReleaseFileName;
 *   readonly role: FileRole;
 *   readonly kind: FileKind;
 *   readonly forbiddenSubstrings: readonly string[];
 * }} ReleaseFileSpec
 * @typedef {{ kind: 'missing' } | { kind: 'empty' } | { kind: 'present'; text: string }} TextArtifactState
 * @typedef {{ kind: 'missing' }
 *   | { kind: 'empty' }
 *   | { kind: 'invalid-json'; error: string }
 *   | { kind: 'invalid-shape'; error: string }
 *   | { kind: 'valid'; text: string; value: Record<string, unknown> }} JsonObjectState
 * @typedef {'pass' | 'fail' | 'skip'} CheckStatus
 * @typedef {'artifacts' | 'bundle' | 'manifest' | 'versions' | 'package' | 'standalone' | 'release'} CheckCategory
 * @typedef {'main-present' | 'manifest-object' | 'versions-present'} CheckRequirement
 * @typedef {{ status: CheckStatus; message: string }} CheckOutcome
 * @typedef {{ id: string; category: CheckCategory; status: CheckStatus; message: string }} ReleaseCheckResult
 * @typedef {{ ok: boolean; tag?: string; checks: ReleaseCheckResult[] }} ReleaseVerificationReport
 * @typedef {{
 *   files: Partial<Record<ReleaseFileName, string | null>>;
 *   tag?: string;
 * }} ValidateReleaseInput
 * @typedef {{
 *   textStates: Record<ReleaseFileName, TextArtifactState>;
 *   manifestState: JsonObjectState;
 *   versionsState: JsonObjectState;
 *   packageState: JsonObjectState;
 *   tsconfigState: JsonObjectState;
 *   tag: string | undefined;
 * }} VerificationContext
 * @typedef {{ ok: true; dir: string; tag?: string; help?: boolean } | { ok: false; error: string }} CliParseResult
 * @typedef {{ stdout?: (line: string) => void; stderr?: (line: string) => void }} CliIo
 */

/** @type {readonly ReleaseFileSpec[]} */
export const RELEASE_FILE_SPECS = [
  {path: 'main.js', role: 'release-artifact', kind: 'js', forbiddenSubstrings: ['../website', '..\\website']},
  {path: 'manifest.json', role: 'release-artifact', kind: 'json', forbiddenSubstrings: []},
  {path: 'versions.json', role: 'release-artifact', kind: 'json', forbiddenSubstrings: []},
  {path: 'styles.css', role: 'release-artifact', kind: 'css', forbiddenSubstrings: []},
  {path: 'README.md', role: 'release-artifact', kind: 'text', forbiddenSubstrings: []},
  {path: 'LICENSE', role: 'release-artifact', kind: 'text', forbiddenSubstrings: []},
  {path: 'package.json', role: 'standalone-config', kind: 'json', forbiddenSubstrings: ['../website', '..\\website']},
  {path: 'tsconfig.json', role: 'standalone-config', kind: 'json', forbiddenSubstrings: ['../website', '..\\website']},
  {
    path: 'esbuild.config.mjs',
    role: 'standalone-config',
    kind: 'js',
    forbiddenSubstrings: ['../website', '..\\website', '_npx'],
  },
];

/** @type {readonly ReleaseArtifactFileName[]} */
export const REQUIRED_RELEASE_FILES = /** @type {readonly ReleaseArtifactFileName[]} */ (
  RELEASE_FILE_SPECS.filter((spec) => spec.role === 'release-artifact').map((spec) => spec.path)
);

const REQUIRED_MANIFEST_STRINGS = /** @type {const} */ ([
  'id',
  'name',
  'version',
  'minAppVersion',
  'description',
  'author',
]);

const REQUIRED_STANDALONE_DEV_DEPS = /** @type {const} */ ([
  'esbuild',
  'obsidian',
  'typescript',
  '@types/node',
]);

const SEMVER_PATTERN =
  /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-((?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*))?(?:\+([0-9a-zA-Z-]+(?:\.[0-9a-zA-Z-]+)*))?$/;

const MANIFEST_ID_PATTERN = /^[a-z][a-z-]*$/;

/** @type {readonly { id: string; description: string; test: (value: string) => boolean }[]} */
export const MANIFEST_ID_RULES = [
  {
    id: 'lowercase-ascii-hyphens',
    description: 'must match /^[a-z][a-z-]*$/ (lowercase ASCII letters and hyphens only)',
    test: (value) => MANIFEST_ID_PATTERN.test(value),
  },
  {
    id: 'no-obsidian-brand',
    description: 'must not contain "obsidian"',
    test: (value) => !value.includes('obsidian'),
  },
  {
    id: 'no-plugin-suffix',
    description: 'must not end with "plugin"',
    test: (value) => !value.endsWith('plugin'),
  },
];

/**
 * @param {unknown} value
 * @returns {value is Record<string, unknown>}
 */
function isPlainObject(value) {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/**
 * @param {unknown} value
 * @returns {value is string}
 */
function isNonEmptyString(value) {
  return typeof value === 'string' && value.trim().length > 0;
}

/**
 * @param {unknown} value
 * @returns {boolean}
 */
function isValidSemver(value) {
  return typeof value === 'string' && SEMVER_PATTERN.test(value);
}

/**
 * @param {string | null | undefined} raw
 * @returns {TextArtifactState}
 */
function parseTextArtifactState(raw) {
  if (raw === null || raw === undefined) {
    return {kind: 'missing'};
  }
  if (raw.trim().length === 0) {
    return {kind: 'empty'};
  }
  return {kind: 'present', text: raw};
}

/**
 * @param {TextArtifactState} state
 * @returns {JsonObjectState}
 */
function parseJsonObjectState(state) {
  if (state.kind === 'missing') {
    return {kind: 'missing'};
  }
  if (state.kind === 'empty') {
    return {kind: 'empty'};
  }
  try {
    const parsed = /** @type {unknown} */ (JSON.parse(state.text));
    if (!isPlainObject(parsed)) {
      return {kind: 'invalid-shape', error: 'Expected a top-level JSON object.'};
    }
    return {kind: 'valid', text: state.text, value: parsed};
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    return {kind: 'invalid-json', error: message};
  }
}

/**
 * @param {string} text
 * @param {readonly string[]} forbidden
 * @returns {string[]}
 */
function findForbiddenSubstrings(text, forbidden) {
  return forbidden.filter((needle) => text.includes(needle));
}

/**
 * @param {CheckRequirement} requirement
 * @param {VerificationContext} ctx
 * @returns {string | null}
 */
function checkRequirementFailure(requirement, ctx) {
  switch (requirement) {
    case 'main-present':
      return ctx.textStates['main.js'].kind === 'present'
        ? null
        : 'Skipped because main.js is missing or empty.';
    case 'manifest-object':
      return ctx.manifestState.kind === 'valid'
        ? null
        : 'Skipped because manifest.json is missing or invalid.';
    case 'versions-present':
      return ctx.textStates['versions.json'].kind === 'present'
        ? null
        : 'Skipped because versions.json is missing or empty.';
    default: {
      /** @type {never} */
      const exhaustive = requirement;
      return exhaustive;
    }
  }
}

/** @type {readonly {
 *   id: string;
 *   category: CheckCategory;
 *   requires: readonly CheckRequirement[];
 *   evaluate: (ctx: VerificationContext) => CheckOutcome;
 * }[]} */
export const RELEASE_CHECKS = [
  {
    id: 'required-release-artifacts',
    category: 'artifacts',
    requires: [],
    evaluate: (ctx) => {
      /** @type {string[]} */
      const problems = [];
      for (const fileName of REQUIRED_RELEASE_FILES) {
        const state = ctx.textStates[fileName];
        if (state.kind === 'missing') {
          problems.push(`${fileName} (missing)`);
        } else if (state.kind === 'empty') {
          problems.push(`${fileName} (empty)`);
        }
      }
      if (problems.length > 0) {
        return {status: 'fail', message: `Missing or empty release artifacts: ${problems.join(', ')}`};
      }
      return {status: 'pass', message: `All ${REQUIRED_RELEASE_FILES.length} required release artifacts exist and are non-empty.`};
    },
  },
  {
    id: 'main-bundle-sanity',
    category: 'bundle',
    requires: ['main-present'],
    evaluate: (ctx) => {
      const state = ctx.textStates['main.js'];
      if (state.kind !== 'present') {
        return {status: 'skip', message: 'Skipped because main.js is missing or empty.'};
      }
      const spec = RELEASE_FILE_SPECS.find((entry) => entry.path === 'main.js');
      const forbidden = findForbiddenSubstrings(state.text, spec ? spec.forbiddenSubstrings : []);
      if (forbidden.length > 0) {
        return {status: 'fail', message: `main.js contains forbidden references: ${forbidden.join(', ')}`};
      }
      if (!state.text.includes('module.exports')) {
        return {status: 'fail', message: 'main.js must export a CommonJS plugin entry via module.exports.'};
      }
      return {status: 'pass', message: 'main.js is a non-empty CommonJS bundle with no monorepo references.'};
    },
  },
  {
    id: 'manifest-schema',
    category: 'manifest',
    requires: [],
    evaluate: (ctx) => {
      const state = ctx.manifestState;
      if (state.kind === 'missing' || state.kind === 'empty') {
        return {status: 'fail', message: `manifest.json is ${state.kind}.`};
      }
      if (state.kind === 'invalid-json' || state.kind === 'invalid-shape') {
        return {status: 'fail', message: `manifest.json is invalid: ${state.error}`};
      }
      /** @type {string[]} */
      const errors = [];
      for (const field of REQUIRED_MANIFEST_STRINGS) {
        if (!isNonEmptyString(state.value[field])) {
          errors.push(`"${field}" must be a non-empty string`);
        }
      }
      if (typeof state.value.isDesktopOnly !== 'boolean') {
        errors.push('"isDesktopOnly" must be a boolean');
      }
      if (state.value.authorUrl !== undefined && !isNonEmptyString(state.value.authorUrl)) {
        errors.push('"authorUrl" must be a non-empty string when present');
      }
      if (errors.length > 0) {
        return {status: 'fail', message: `manifest.json schema errors: ${errors.join('; ')}`};
      }
      return {status: 'pass', message: 'manifest.json contains all required fields with valid types.'};
    },
  },
  {
    id: 'manifest-id-policy',
    category: 'manifest',
    requires: ['manifest-object'],
    evaluate: (ctx) => {
      if (ctx.manifestState.kind !== 'valid') {
        return {status: 'skip', message: 'Skipped because manifest.json is invalid.'};
      }
      const rawId = ctx.manifestState.value.id;
      if (!isNonEmptyString(rawId)) {
        return {status: 'skip', message: 'Skipped because manifest.json id is missing.'};
      }
      const violations = MANIFEST_ID_RULES
        .filter((rule) => !rule.test(rawId))
        .map((rule) => rule.description);
      if (violations.length > 0) {
        return {status: 'fail', message: `manifest.json id "${rawId}" violates Community Plugin policy: ${violations.join('; ')}`};
      }
      return {status: 'pass', message: `manifest.json id "${rawId}" complies with Obsidian Community Plugin naming rules.`};
    },
  },
  {
    id: 'manifest-semver',
    category: 'manifest',
    requires: ['manifest-object'],
    evaluate: (ctx) => {
      if (ctx.manifestState.kind !== 'valid') {
        return {status: 'skip', message: 'Skipped because manifest.json is invalid.'};
      }
      /** @type {string[]} */
      const invalid = [];
      if (!isValidSemver(ctx.manifestState.value.version)) {
        invalid.push(`version="${String(ctx.manifestState.value.version)}"`);
      }
      if (!isValidSemver(ctx.manifestState.value.minAppVersion)) {
        invalid.push(`minAppVersion="${String(ctx.manifestState.value.minAppVersion)}"`);
      }
      if (invalid.length > 0) {
        return {status: 'fail', message: `manifest.json fields must be valid SemVer without leading "v": ${invalid.join(', ')}`};
      }
      return {
        status: 'pass',
        message: `manifest.json versions are valid SemVer (version=${String(ctx.manifestState.value.version)}, minAppVersion=${String(ctx.manifestState.value.minAppVersion)}).`,
      };
    },
  },
  {
    id: 'versions-json-parity',
    category: 'versions',
    requires: ['versions-present'],
    evaluate: (ctx) => {
      const state = ctx.versionsState;
      if (state.kind === 'missing' || state.kind === 'empty') {
        return {status: 'skip', message: `Skipped because versions.json is ${state.kind}.`};
      }
      if (state.kind === 'invalid-json' || state.kind === 'invalid-shape') {
        return {status: 'fail', message: `versions.json is invalid: ${state.error}`};
      }
      const entries = Object.entries(state.value);
      if (entries.length === 0) {
        return {status: 'fail', message: 'versions.json must map at least one plugin version to a minAppVersion.'};
      }
      for (const [pluginVer, appVer] of entries) {
        if (!isValidSemver(pluginVer) || !isValidSemver(appVer)) {
          return {status: 'fail', message: `versions.json entry "${pluginVer}": "${String(appVer)}" must use valid SemVer strings.`};
        }
      }
      if (ctx.manifestState.kind !== 'valid') {
        return {status: 'skip', message: 'Skipped manifest parity because manifest.json is invalid.'};
      }
      const manifestVersion = ctx.manifestState.value.version;
      const manifestMinApp = ctx.manifestState.value.minAppVersion;
      if (!isNonEmptyString(manifestVersion) || !isNonEmptyString(manifestMinApp)) {
        return {status: 'skip', message: 'Skipped manifest parity because manifest version fields are missing.'};
      }
      if (!Object.hasOwn(state.value, manifestVersion)) {
        return {status: 'fail', message: `versions.json is missing key "${manifestVersion}" from manifest.json.`};
      }
      if (state.value[manifestVersion] !== manifestMinApp) {
        return {
          status: 'fail',
          message: `versions.json["${manifestVersion}"] is "${String(state.value[manifestVersion])}", expected "${manifestMinApp}".`,
        };
      }
      return {status: 'pass', message: `versions.json maps "${manifestVersion}" to "${manifestMinApp}".`};
    },
  },
  {
    id: 'package-json-standalone',
    category: 'package',
    requires: [],
    evaluate: (ctx) => {
      const state = ctx.packageState;
      if (state.kind === 'missing' || state.kind === 'empty') {
        return {status: 'fail', message: `package.json is ${state.kind}.`};
      }
      if (state.kind === 'invalid-json' || state.kind === 'invalid-shape') {
        return {status: 'fail', message: `package.json is invalid: ${state.error}`};
      }
      /** @type {string[]} */
      const errors = [];
      if (!isValidSemver(state.value.version)) {
        errors.push(`version="${String(state.value.version)}" is not valid SemVer`);
      } else if (
        ctx.manifestState.kind === 'valid' &&
        isNonEmptyString(ctx.manifestState.value.version) &&
        state.value.version !== ctx.manifestState.value.version
      ) {
        errors.push(`version "${state.value.version}" does not match manifest.json version "${ctx.manifestState.value.version}"`);
      }
      const devDeps = state.value.devDependencies;
      if (!isPlainObject(devDeps)) {
        errors.push('devDependencies object is missing');
      } else {
        for (const dep of REQUIRED_STANDALONE_DEV_DEPS) {
          if (!isNonEmptyString(devDeps[dep])) {
            errors.push(`devDependencies.${dep} is missing`);
          }
        }
      }
      if (errors.length > 0) {
        return {status: 'fail', message: `package.json validation failed: ${errors.join('; ')}`};
      }
      return {status: 'pass', message: 'package.json version and standalone devDependencies are valid.'};
    },
  },
  {
    id: 'standalone-configs-clean',
    category: 'standalone',
    requires: [],
    evaluate: (ctx) => {
      /** @type {string[]} */
      const problems = [];
      for (const spec of RELEASE_FILE_SPECS) {
        if (spec.role !== 'standalone-config') {
          continue;
        }
        const state = ctx.textStates[spec.path];
        if (state.kind === 'missing') {
          problems.push(`${spec.path} is missing`);
          continue;
        }
        if (state.kind === 'empty') {
          problems.push(`${spec.path} is empty`);
          continue;
        }
        const forbidden = findForbiddenSubstrings(state.text, spec.forbiddenSubstrings);
        if (forbidden.length > 0) {
          problems.push(`${spec.path} contains forbidden reference(s): ${forbidden.join(', ')}`);
        }
      }
      if (ctx.tsconfigState.kind === 'invalid-json' || ctx.tsconfigState.kind === 'invalid-shape') {
        problems.push(`tsconfig.json is invalid: ${ctx.tsconfigState.error}`);
      }
      if (problems.length > 0) {
        return {status: 'fail', message: problems.join('; ')};
      }
      return {status: 'pass', message: 'Standalone config files contain no monorepo or npx cache references.'};
    },
  },
  {
    id: 'release-tag-match',
    category: 'release',
    requires: [],
    evaluate: (ctx) => {
      if (ctx.tag === undefined) {
        return {status: 'skip', message: 'No --tag provided; tag parity check skipped.'};
      }
      if (ctx.tag.trim().length === 0) {
        return {status: 'fail', message: 'Release tag must not be empty.'};
      }
      if (/^v/i.test(ctx.tag)) {
        return {status: 'fail', message: `Release tag "${ctx.tag}" must not have a leading "v" prefix.`};
      }
      if (!isValidSemver(ctx.tag)) {
        return {status: 'fail', message: `Release tag "${ctx.tag}" is not valid SemVer.`};
      }
      if (ctx.manifestState.kind !== 'valid' || !isNonEmptyString(ctx.manifestState.value.version)) {
        return {status: 'skip', message: 'Skipped tag comparison because manifest.json version is unavailable.'};
      }
      if (ctx.tag !== ctx.manifestState.value.version) {
        return {
          status: 'fail',
          message: `Release tag "${ctx.tag}" does not match manifest.json version "${ctx.manifestState.value.version}".`,
        };
      }
      return {status: 'pass', message: `Release tag "${ctx.tag}" matches manifest.json version.`};
    },
  },
];

/**
 * Validates release artifacts, manifest rules, version parity, and standalone config hygiene.
 * @param {ValidateReleaseInput} input
 * @returns {ReleaseVerificationReport}
 */
export function validateRelease(input) {
  /** @type {Record<ReleaseFileName, TextArtifactState>} */
  const textStates = {
    'main.js': parseTextArtifactState(input.files['main.js']),
    'manifest.json': parseTextArtifactState(input.files['manifest.json']),
    'versions.json': parseTextArtifactState(input.files['versions.json']),
    'styles.css': parseTextArtifactState(input.files['styles.css']),
    'README.md': parseTextArtifactState(input.files['README.md']),
    LICENSE: parseTextArtifactState(input.files.LICENSE),
    'package.json': parseTextArtifactState(input.files['package.json']),
    'tsconfig.json': parseTextArtifactState(input.files['tsconfig.json']),
    'esbuild.config.mjs': parseTextArtifactState(input.files['esbuild.config.mjs']),
  };

  /** @type {VerificationContext} */
  const ctx = {
    textStates,
    manifestState: parseJsonObjectState(textStates['manifest.json']),
    versionsState: parseJsonObjectState(textStates['versions.json']),
    packageState: parseJsonObjectState(textStates['package.json']),
    tsconfigState: parseJsonObjectState(textStates['tsconfig.json']),
    tag: input.tag,
  };

  /** @type {ReleaseCheckResult[]} */
  const checks = RELEASE_CHECKS.map((check) => {
    for (const req of check.requires) {
      const skipReason = checkRequirementFailure(req, ctx);
      if (skipReason !== null) {
        return {id: check.id, category: check.category, status: 'skip', message: skipReason};
      }
    }
    const outcome = check.evaluate(ctx);
    return {id: check.id, category: check.category, status: outcome.status, message: outcome.message};
  });

  return {
    ok: checks.every((check) => check.status !== 'fail'),
    ...(input.tag !== undefined ? {tag: input.tag} : {}),
    checks,
  };
}

/**
 * Reads plugin files from disk and validates them as a standalone Obsidian Community Plugin release.
 * @param {string} rootDir
 * @param {{ tag?: string }} [options]
 * @returns {ReleaseVerificationReport}
 */
export function verifyReleaseDirectory(rootDir, options = {}) {
  /** @type {Partial<Record<ReleaseFileName, string | null>>} */
  const files = {};
  for (const spec of RELEASE_FILE_SPECS) {
    const filePath = path.join(rootDir, spec.path);
    try {
      files[spec.path] = fs.readFileSync(filePath, 'utf8');
    } catch (err) {
      const code = err instanceof Error && 'code' in err ? err.code : undefined;
      if (code === 'ENOENT' || code === 'EISDIR') {
        files[spec.path] = null;
      } else {
        throw err;
      }
    }
  }
  return validateRelease({files, tag: options.tag});
}

/**
 * Parses CLI flags (`--tag`, `--dir`, `--help`) for release verification.
 * @param {readonly string[]} argv
 * @param {string} defaultDir
 * @returns {CliParseResult}
 */
export function parseCliArgs(argv, defaultDir) {
  let dir = defaultDir;
  /** @type {string | undefined} */
  let tag;
  let help = false;

  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--help' || arg === '-h') {
      help = true;
      continue;
    }
    if (arg === '--tag') {
      const next = argv[i + 1];
      if (next === undefined || next.length === 0 || next.startsWith('--')) {
        return {ok: false, error: 'Missing value for --tag.'};
      }
      tag = next;
      i += 1;
      continue;
    }
    if (arg.startsWith('--tag=')) {
      const value = arg.slice('--tag='.length);
      if (value.length === 0) {
        return {ok: false, error: 'Missing value for --tag.'};
      }
      tag = value;
      continue;
    }
    if (arg === '--dir') {
      const next = argv[i + 1];
      if (next === undefined || next.length === 0 || next.startsWith('--')) {
        return {ok: false, error: 'Missing value for --dir.'};
      }
      dir = path.resolve(next);
      i += 1;
      continue;
    }
    if (arg.startsWith('--dir=')) {
      const value = arg.slice('--dir='.length);
      if (value.length === 0) {
        return {ok: false, error: 'Missing value for --dir.'};
      }
      dir = path.resolve(value);
      continue;
    }
    return {ok: false, error: `Unknown argument: ${arg}`};
  }

  return {
    ok: true,
    dir,
    ...(tag !== undefined ? {tag} : {}),
    ...(help ? {help: true} : {}),
  };
}

/**
 * Formats a verification report into human-readable lines.
 * @param {ReleaseVerificationReport} report
 * @returns {string}
 */
export function formatVerificationReport(report) {
  const lines = report.checks.map(
    (check) => `[${check.status.toUpperCase()}] ${check.id}: ${check.message}`,
  );
  const failed = report.checks.filter((check) => check.status === 'fail').length;
  const passed = report.checks.filter((check) => check.status === 'pass').length;
  const skipped = report.checks.filter((check) => check.status === 'skip').length;
  lines.push(
    `Result: ${report.ok ? 'PASS' : 'FAIL'} (${passed} passed, ${failed} failed, ${skipped} skipped)`,
  );
  return lines.join('\n');
}

/**
 * Runs the CLI entrypoint and returns an exit code (`0` on pass, `1` on failure).
 * @param {readonly string[]} argv
 * @param {string} defaultDir
 * @param {CliIo} [io]
 * @returns {number}
 */
export function runReleaseCli(argv, defaultDir, io = {}) {
  const stdout = io.stdout ?? ((line) => console.log(line));
  const stderr = io.stderr ?? ((line) => console.error(line));
  const parsed = parseCliArgs(argv, defaultDir);

  if (!parsed.ok) {
    stderr(`Error: ${parsed.error}`);
    stderr('Usage: node scripts/verify-release.mjs [--dir <path>] [--tag <semver>] [--help]');
    return 1;
  }

  if (parsed.help) {
    stdout('Usage: node scripts/verify-release.mjs [--dir <path>] [--tag <semver>] [--help]');
    return 0;
  }

  const report = verifyReleaseDirectory(parsed.dir, {tag: parsed.tag});
  const formatted = formatVerificationReport(report);
  if (report.ok) {
    stdout(formatted);
    return 0;
  }
  stderr(formatted);
  return 1;
}

function isMainModule() {
  const entryArg = process.argv[1];
  if (!entryArg) {
    return false;
  }
  try {
    return fs.realpathSync(fileURLToPath(import.meta.url)) === fs.realpathSync(path.resolve(entryArg));
  } catch {
    return false;
  }
}

if (isMainModule()) {
  const pluginRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
  process.exitCode = runReleaseCli(process.argv.slice(2), pluginRoot);
}
