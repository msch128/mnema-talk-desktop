#!/usr/bin/env node
// A release tag vX.Y.Z must match the version the app compares when it updates
// (tauri.conf.json), its crate and the lockfile entry, and the Linux client's
// package.json and lockfile; otherwise the installer would announce one version
// and carry another.
import { readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export function versions({ conf, cargoToml, cargoLock, crate, linuxPackage, linuxLock }) {
  const toml = cargoToml.match(/^\[package\][^[]*?^version\s*=\s*"([^"]+)"/ms)?.[1];
  const lock = cargoLock.match(new RegExp(`^name = "${crate}"\\nversion = "([^"]+)"`, 'm'))?.[1];
  const found = { conf: JSON.parse(conf).version, cargoToml: toml, cargoLock: lock };
  // The Linux client (Electron) is released with the same tag.
  if (linuxPackage !== undefined) {
    found.linuxPackage = JSON.parse(linuxPackage).version;
    found.linuxLock = JSON.parse(linuxLock).version;
    found.linuxLockRoot = JSON.parse(linuxLock).packages?.['']?.version;
  }
  return found;
}

export function check(tag, found) {
  const m = /^v(\d+\.\d+\.\d+)$/.exec(tag);
  if (!m) return `Tag ${tag} is not vMAJOR.MINOR.PATCH.`;
  for (const [file, version] of Object.entries(found)) {
    if (version !== m[1]) return `${file} says ${version ?? 'nothing'}, the tag says ${m[1]}.`;
  }
  return null;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const app = resolve(dirname(fileURLToPath(import.meta.url)), '..', 'experimental', 'app');
  const found = versions({
    conf: readFileSync(join(app, 'tauri.conf.json'), 'utf8'),
    cargoToml: readFileSync(join(app, 'Cargo.toml'), 'utf8'),
    cargoLock: readFileSync(join(app, 'Cargo.lock'), 'utf8'),
    crate: 'mnema-native-client-preview',
    linuxPackage: readFileSync(join(app, '..', '..', 'linux', 'package.json'), 'utf8'),
    linuxLock: readFileSync(join(app, '..', '..', 'linux', 'package-lock.json'), 'utf8')
  });
  const problem = check(process.argv[2] ?? '', found);
  if (problem) {
    console.error(`::error::${problem} Bump experimental/app/tauri.conf.json, Cargo.toml and Cargo.lock, and linux/package.json and package-lock.json, together.`);
    process.exit(1);
  }
  console.log(`Release ${process.argv[2]} matches the app version.`);
}
