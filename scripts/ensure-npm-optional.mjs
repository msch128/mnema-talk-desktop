#!/usr/bin/env node
// `npm ci` can finish without the platform package of an optional dependency
// (npm/cli#4828, seen as "Cannot find module '@tauri-apps/cli-win32-x64-msvc'").
// This checks, from package-lock.json, that every optional package of this
// platform is installed, and installs a missing one at exactly the locked
// version. It does not retry: if the package is still missing afterwards, or
// installed at another version, the build fails with the name.
// Linux packages also name a libc (`glibc` or `musl`). Installing the other
// one makes npm exit EBADPLATFORM, so an unknown libc (hostLibc() is null)
// skips those packages with a warning instead of guessing.
//   node scripts/ensure-npm-optional.mjs <dir with package-lock.json> [scope/prefix, default @tauri-apps/cli-]
import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Official Node builds set glibcVersionRuntime when they are linked to glibc.
// A musl build names libc.musl or ld-musl in the report's shared objects.
// Null off Linux and when the libc cannot be told.
export function hostLibc() {
  if (process.platform !== 'linux') return null;
  const header = process.report.getReport().header ?? {};
  if (header.glibcVersionRuntime || header.glibcVersion) return 'glibc';
  const objects = Array.isArray(header.sharedObjects) ? header.sharedObjects : [];
  const text = objects.join('\n');
  if (text.includes('libc.musl-') || text.includes('ld-musl-')) return 'musl';
  if (text.includes('libc.so.6') || text.includes('ld-linux')) return 'glibc';
  return null;
}

export function wanted(lock, prefix, platform = process.platform, arch = process.arch, libc = hostLibc()) {
  const out = [];
  for (const [path, entry] of Object.entries(lock.packages ?? {})) {
    const name = path.replace(/^node_modules\//, '');
    if (!path.startsWith('node_modules/') || path.indexOf('node_modules/', 13) !== -1 || !name.startsWith(prefix)) continue;
    if (!entry.optional) continue;
    if (entry.os && !entry.os.includes(platform)) continue;
    if (entry.cpu && !entry.cpu.includes(arch)) continue;
    if (entry.libc && (!libc || !entry.libc.includes(libc))) continue;
    out.push({ name, version: entry.version });
  }
  return out;
}

const installedVersion = (dir, name) => {
  const file = join(dir, 'node_modules', name, 'package.json');
  return existsSync(file) ? JSON.parse(readFileSync(file, 'utf8')).version : null;
};

export function ensure(dir, prefix, { install = defaultInstall, platform, arch, libc } = {}) {
  const lock = JSON.parse(readFileSync(join(dir, 'package-lock.json'), 'utf8'));
  const needed = wanted(lock, prefix, platform, arch, libc);
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
  const libc = hostLibc();
  const { needed, notes } = ensure(resolve(dir), prefix, { libc });
  for (const note of notes) console.log(`::warning::${note}`);
  if (process.platform === 'linux' && !libc) console.log('::warning::cannot tell glibc from musl; packages that name a libc were skipped');
  const label = [process.platform, process.arch, libc].filter(Boolean).join('-');
  console.log(`Optional packages for ${label}: ${needed.map((p) => `${p.name}@${p.version}`).join(', ') || 'none locked'}`);
}
