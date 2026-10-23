#!/usr/bin/env node
// `npm ci` can finish without the platform package of an optional dependency
// (npm/cli#4828, seen as "Cannot find module '@tauri-apps/cli-win32-x64-msvc'").
// This checks, from package-lock.json, that every optional package of this
// platform is installed, and installs a missing one at exactly the locked
// version. It does not retry: if the package is still missing afterwards, or
// installed at another version, the build fails with the name.
//   node scripts/ensure-npm-optional.mjs <dir with package-lock.json> [scope/prefix, default @tauri-apps/cli-]
import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export function wanted(lock, prefix, platform = process.platform, arch = process.arch) {
  const out = [];
  for (const [path, entry] of Object.entries(lock.packages ?? {})) {
    const name = path.replace(/^node_modules\//, '');
    if (!path.startsWith('node_modules/') || path.indexOf('node_modules/', 13) !== -1 || !name.startsWith(prefix)) continue;
    if (!entry.optional) continue;
    if (entry.os && !entry.os.includes(platform)) continue;
    if (entry.cpu && !entry.cpu.includes(arch)) continue;
    out.push({ name, version: entry.version });
  }
  return out;
}

const installedVersion = (dir, name) => {
  const file = join(dir, 'node_modules', name, 'package.json');
  return existsSync(file) ? JSON.parse(readFileSync(file, 'utf8')).version : null;
};

export function ensure(dir, prefix, { install = defaultInstall, platform, arch } = {}) {
  const lock = JSON.parse(readFileSync(join(dir, 'package-lock.json'), 'utf8'));
  const needed = wanted(lock, prefix, platform, arch);
  const notes = [];
  for (const { name, version } of needed) {
    if (installedVersion(dir, name) === version) continue;
    notes.push(`${name}@${version} was not installed by npm ci; installing the locked version`);
    install(dir, `${name}@${version}`);
    if (installedVersion(dir, name) !== version) {
      throw new Error(`${name}@${version} is still missing after an explicit install`);
    }
  }
  return { needed, notes };
}

function defaultInstall(dir, spec) {
  const run = spawnSync('npm', ['install', '--no-save', '--ignore-scripts', '--no-audit', '--no-fund', spec], {
    cwd: dir, stdio: 'inherit', shell: process.platform === 'win32',
  });
  if (run.status !== 0) throw new Error(`npm install ${spec} failed`);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const [dir = '.', prefix = '@tauri-apps/cli-'] = process.argv.slice(2);
  const { needed, notes } = ensure(resolve(dir), prefix);
  for (const note of notes) console.log(`::warning::${note}`);
  console.log(`Optional packages for ${process.platform}-${process.arch}: ${needed.map((p) => `${p.name}@${p.version}`).join(', ') || 'none locked'}`);
}
