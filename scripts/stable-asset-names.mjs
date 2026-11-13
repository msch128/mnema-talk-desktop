#!/usr/bin/env node
// Every release also carries the user-facing installers under names without the
// version, so that
// https://github.com/msch128/mnema-talk-desktop/releases/latest/download/<name>
// always serves the newest one (the website links there). Each copy is
// byte-identical to its versioned file. The updater feed (latest.json) and the
// .sig files keep pointing at the versioned files: nothing here touches them.
//
//   node scripts/stable-asset-names.mjs --version X.Y.Z <release directory>
import { copyFileSync, createReadStream, readdirSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

// Stable name and the versioned file it copies, for version `v` (X.Y.Z).
export function stableAssets(version) {
  if (!/^\d+\.\d+\.\d+$/.test(version)) throw new Error(`version ${version} is not MAJOR.MINOR.PATCH`);
  const v = version.replaceAll('.', '\\.');
  const p = (source) => new RegExp(`^${source}$`);
  return [
    ['mnema-desktop-windows-x64-setup.exe', p(`Mnema-Desktop-DEV_${v}_x64-setup\\.exe`)],
    ['mnema-desktop-windows-x64-portable.zip', p(`Mnema-Desktop-DEV-v${v}-windows-x64-portable\\.zip`)],
    ['mnema-desktop-macos-arm64.dmg', p(`Mnema-Desktop-DEV_${v}_aarch64\\.dmg`)],
    ['mnema-desktop-linux-x64.deb', p(`Mnema-Desktop-DEV_${v}_(amd64|x86_64|x64)\\.deb`)],
    ['mnema-desktop-linux-arm64.deb', p(`Mnema-Desktop-DEV_${v}_(arm64|aarch64)\\.deb`)],
    ['mnema-desktop-linux-x64.AppImage', p(`Mnema-Desktop-DEV_${v}_x86_64\\.AppImage`)],
    ['mnema-desktop-linux-arm64.AppImage', p(`Mnema-Desktop-DEV_${v}_arm64\\.AppImage`)]
  ].map(([name, source]) => ({ name, source }));
}

// Which versioned file each stable name copies. Exactly one file must match
// each, and no stable name may exist yet: a missing or doubled installer stops
// the release instead of publishing a wrong link.
export function plan(files, version) {
  return stableAssets(version).map(({ name, source }) => {
    if (files.includes(name)) throw new Error(`${name} already exists`);
    const found = files.filter((file) => source.test(file));
    if (found.length !== 1) throw new Error(`${name}: expected one file matching ${source}, found ${found.length}`);
    return { from: found[0], to: name };
  });
}

function sha256(path) {
  return new Promise((resolve, reject) => {
    const hash = createHash('sha256');
    createReadStream(path).on('error', reject).on('data', (d) => hash.update(d)).on('end', () => resolve(hash.digest('hex')));
  });
}

// Copies and checks every pair; returns the plan that was carried out.
export async function copyStable(dir, version) {
  const pairs = plan(readdirSync(dir), version);
  for (const { from, to } of pairs) {
    copyFileSync(join(dir, from), join(dir, to));
    const [a, b] = await Promise.all([sha256(join(dir, from)), sha256(join(dir, to))]);
    if (a !== b) throw new Error(`${to} differs from ${from}`);
  }
  return pairs;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const [flag, version, dir] = process.argv.slice(2);
  if (flag !== '--version' || !version || !dir) {
    console.error('usage: stable-asset-names.mjs --version X.Y.Z <release directory>');
    process.exit(2);
  }
  try {
    for (const { from, to } of await copyStable(dir, version)) console.log(`${to} <- ${from}`);
  } catch (error) {
    console.error(`::error::${error.message}`);
    process.exit(1);
  }
}
