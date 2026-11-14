#!/usr/bin/env node
// The whole self-update of a real build, from the check to the restart into the
// new version (experimental/app/src/native_updates/selftest.rs).
//
// The CI job builds an old and a new version of the host with the test-only
// `update-selftest` feature and an updater config that names a throwaway key
// (made in the job, never a release key) and a feed on this machine. This
// script serves that feed and the signed new packages, starts the old program
// with MNEMA_UPDATE_SELFTEST naming the record, waits for the new version to
// write `done`, and checks the record: the old version found the update, asked
// first during the simulated Talk, downloaded and verified it, waited for the
// end of the Talk, installed it, and the restarted program runs the new version
// on the same data.
//
// Usage: node scripts/update-selftest.mjs --port N --version X.Y.Z --old X.Y.Z
//   --entry <feed key>=<package file> [--entry ...] --record <file> --run <program>
// The package's signature is read from <package file>.sig.
//
// node scripts/update-selftest.mjs config --pubkey-file <key.pub> --port N
//   --version X.Y.Z --signed true|false --out <file> writes the build config of
//   one of the two versions (`tauri build --config <file>`).
import { spawn } from 'node:child_process';
import { createReadStream, existsSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { basename } from 'node:path';
import { fileURLToPath } from 'node:url';

/** What the record must hold, in this order. */
export const STEPS = [
  'the check at start finds the new version',
  'a fresh trust statement is in force',
  'Update now during a Talk asks first',
  'the update is downloaded, verified and waits for the Talk',
  'nothing installs while the Talk goes on',
  'the Talk ends, the update installs and the app restarts',
  'the restarted app runs the new version',
  'the data folder was kept'
];

/** Problems with a finished record; empty when the update went through. */
export function judge(text, oldVersion, newVersion) {
  const lines = text.split(/\r?\n/).filter(Boolean);
  const problems = lines.filter((l) => l.startsWith('fail ')).map((l) => l.slice(5));
  const passed = lines.filter((l) => l.startsWith('ok ')).map((l) => l.slice(3));
  let at = -1;
  for (const step of STEPS) {
    const found = passed.indexOf(step);
    if (found < 0) problems.push(`missing: ${step}`);
    else if (found < at) problems.push(`out of order: ${step}`);
    else at = found;
  }
  const versions = lines.filter((l) => l.startsWith('version ')).map((l) => l.slice(8));
  if (versions[0] !== oldVersion) problems.push(`the old program reported ${versions[0] ?? 'no version'}, not ${oldVersion}`);
  if (versions.at(-1) !== newVersion) problems.push(`the restarted program reported ${versions.at(-1) ?? 'no version'}, not ${newVersion}`);
  if (lines.at(-1) !== 'done') problems.push('the record does not end with done');
  return problems;
}

/**
 * The config merged into tauri.conf.json for a test build: its version, the
 * throwaway key and the feed on this machine (plain HTTP on loopback, which only
 * a build with the `update-selftest` feature accepts). The old version is not
 * signed for the updater; the new one is.
 */
export function selftestConfig(pubkey, port, version, signed) {
  if (!/^\d+\.\d+\.\d+$/.test(version)) throw new Error('Version must be MAJOR.MINOR.PATCH.');
  if (!/^[A-Za-z0-9+/=]{40,}$/.test(pubkey.trim())) throw new Error('The public key must be the text of a Tauri .pub file.');
  return {
    version,
    bundle: { createUpdaterArtifacts: signed },
    plugins: {
      updater: {
        pubkey: pubkey.trim(),
        endpoints: [`http://127.0.0.1:${Number(port)}/latest.json`],
        dangerousInsecureTransportProtocol: true
      }
    }
  };
}

export function parseArgs(argv) {
  const args = { entry: [] };
  for (let i = 0; i < argv.length; i += 2) {
    const flag = argv[i];
    const value = argv[i + 1];
    if (!flag?.startsWith('--') || value === undefined || value.startsWith('--')) throw new Error(`Bad argument ${flag}.`);
    const name = flag.slice(2);
    if (name === 'entry') args.entry.push(value);
    else args[name] = value;
  }
  for (const need of ['port', 'version', 'old', 'record', 'run']) {
    if (!args[need]) throw new Error(`--${need} is required.`);
  }
  if (args.entry.length === 0) throw new Error('At least one --entry is required.');
  return args;
}

/** The feed in the format of the Tauri updater, naming the packages on this machine. */
export function feed(version, port, entries) {
  const platforms = {};
  for (const { key, file } of entries) {
    platforms[key] = {
      signature: readFileSync(`${file}.sig`, 'utf8').trim(),
      url: `http://127.0.0.1:${port}/${encodeURIComponent(basename(file))}`
    };
  }
  return { version, notes: 'Update self-test', pub_date: new Date().toISOString(), platforms };
}

function serve(port, body, entries) {
  const files = new Map(entries.map(({ file }) => [`/${encodeURIComponent(basename(file))}`, file]));
  const server = createServer((req, res) => {
    if (req.url === '/latest.json') {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify(body));
      return;
    }
    const file = files.get(req.url ?? '');
    if (!file) {
      res.writeHead(404);
      res.end();
      return;
    }
    res.writeHead(200, { 'content-type': 'application/octet-stream', 'content-length': String(statSync(file).size) });
    createReadStream(file).pipe(res);
  });
  return new Promise((resolve) => server.listen(Number(port), '127.0.0.1', () => resolve(server)));
}

async function waitForDone(record, minutes) {
  const end = Date.now() + minutes * 60_000;
  while (Date.now() < end) {
    const text = existsSync(record) ? readFileSync(record, 'utf8') : '';
    // The old program also writes `done` after a failure; the new one ends the record.
    if (/^done$/m.test(text) && (/^fail /m.test(text) || /^ok the data folder was kept$/m.test(text))) return text;
    await new Promise((resolve) => setTimeout(resolve, 1000));
  }
  return existsSync(record) ? readFileSync(record, 'utf8') : '';
}

function writeConfig(argv) {
  const args = {};
  for (let i = 0; i < argv.length; i += 2) args[argv[i].replace(/^--/, '')] = argv[i + 1];
  const config = selftestConfig(readFileSync(args['pubkey-file'], 'utf8'), args.port, args.version, args.signed === 'true');
  writeFileSync(args.out, JSON.stringify(config));
}

async function main() {
  if (process.argv[2] === 'config') {
    writeConfig(process.argv.slice(3));
    return;
  }
  const args = parseArgs(process.argv.slice(2));
  const entries = args.entry.map((e) => {
    const at = e.indexOf('=');
    const key = e.slice(0, at);
    const file = e.slice(at + 1);
    if (at < 1 || !existsSync(file) || !existsSync(`${file}.sig`)) throw new Error(`Entry ${e} needs a package and its .sig.`);
    return { key, file };
  });
  rmSync(args.record, { force: true });
  const server = await serve(args.port, feed(args.version, args.port, entries), entries);
  const child = spawn(args.run, [], {
    detached: true,
    stdio: 'ignore',
    env: { ...process.env, MNEMA_UPDATE_SELFTEST: args.record }
  });
  child.unref();
  const text = await waitForDone(args.record, Number(args.minutes ?? 15));
  server.close();
  console.log(text);
  const problems = judge(text, args.old, args.version);
  for (const problem of problems) console.error(`::error::update self-test: ${problem}`);
  process.exit(problems.length === 0 ? 0 : 1);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    console.error(`::error::${error.message}`);
    process.exit(1);
  });
}
