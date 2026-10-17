#!/usr/bin/env node
// A release tag vX.Y.Z must match the version the app compares when it updates
// (tauri.conf.json), its crate and the lockfile entry; otherwise the installer
// would announce one version and carry another.
import { readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export function versions({ conf, cargoToml, cargoLock, crate }) {
  const toml = cargoToml.match(/^\[package\][^[]*?^version\s*=\s*"([^"]+)"/ms)?.[1];
  const lock = cargoLock.match(new RegExp(`^name = "${crate}"\\nversion = "([^"]+)"`, 'm'))?.[1];
  return { conf: JSON.parse(conf).version, cargoToml: toml, cargoLock: lock };
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
    crate: 'mnema-native-client-preview'
  });
  const problem = check(process.argv[2] ?? '', found);
  if (problem) {
    console.error(`::error::${problem} Bump experimental/app/tauri.conf.json, Cargo.toml and Cargo.lock together.`);
    process.exit(1);
  }
  console.log(`Release ${process.argv[2]} matches the app version.`);
}
