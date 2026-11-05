import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import { test } from 'node:test';

// One Rust version is pinned (rust-toolchain.toml, the app's copy and the CI
// jobs). A job that still names the previous release installs a second
// toolchain that rustup then overrides, and its cache and logs disagree with
// what was tested.
const root = new URL('../', import.meta.url);
const channel = (path) => readFileSync(new URL(path, root), 'utf8').match(/^channel\s*=\s*"([^"]+)"$/m)?.[1];

test('the two toolchain files pin the same Rust version', () => {
  const pinned = channel('rust-toolchain.toml');
  assert.match(pinned, /^\d+\.\d+\.\d+$/);
  assert.equal(channel('experimental/app/rust-toolchain.toml'), pinned);
});

test('every workflow that names a Rust version names the pinned one', () => {
  const pinned = channel('rust-toolchain.toml');
  const workflows = new URL('.github/workflows/', root);
  let mentions = 0;
  for (const file of readdirSync(workflows).filter((name) => name.endsWith('.yml'))) {
    const text = readFileSync(new URL(file, workflows), 'utf8');
    for (const [, version] of text.matchAll(/^\s*RUST_VERSION:\s*['"]?([^'"\s#]+)/gm)) {
      mentions += 1;
      assert.equal(version, pinned, `${file}: RUST_VERSION ${version}`);
    }
    for (const [, version] of text.matchAll(/(?:rustup toolchain install|cargo \+)\s*([0-9][0-9.]*)/g)) {
      mentions += 1;
      assert.equal(version, pinned, `${file}: toolchain ${version}`);
    }
  }
  assert.ok(mentions > 10, `only ${mentions} Rust version mentions found`);
});
