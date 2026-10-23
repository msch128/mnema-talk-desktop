import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { ensure, wanted } from './ensure-npm-optional.mjs';

const lock = {
  packages: {
    '': {},
    'node_modules/@tauri-apps/cli': { version: '2.12.4' },
    'node_modules/@tauri-apps/cli-win32-x64-msvc': { version: '2.12.4', optional: true, os: ['win32'], cpu: ['x64'] },
    'node_modules/@tauri-apps/cli-darwin-arm64': { version: '2.12.4', optional: true, os: ['darwin'], cpu: ['arm64'] },
    'node_modules/@tauri-apps/cli-win32-arm64-msvc': { version: '2.12.4', optional: true, os: ['win32'], cpu: ['arm64'] },
    'node_modules/other/node_modules/@tauri-apps/cli-win32-x64-msvc': { version: '1.0.0', optional: true, os: ['win32'], cpu: ['x64'] },
  },
};

function project(installed = {}) {
  const dir = mkdtempSync(join(tmpdir(), 'optional-'));
  writeFileSync(join(dir, 'package-lock.json'), JSON.stringify(lock));
  for (const [name, version] of Object.entries(installed)) {
    mkdirSync(join(dir, 'node_modules', name), { recursive: true });
    writeFileSync(join(dir, 'node_modules', name, 'package.json'), JSON.stringify({ name, version }));
  }
  return dir;
}

test('only the top-level optional packages of this platform and architecture are wanted', () => {
  assert.deepEqual(wanted(lock, '@tauri-apps/cli-', 'win32', 'x64'), [{ name: '@tauri-apps/cli-win32-x64-msvc', version: '2.12.4' }]);
  assert.deepEqual(wanted(lock, '@tauri-apps/cli-', 'darwin', 'arm64'), [{ name: '@tauri-apps/cli-darwin-arm64', version: '2.12.4' }]);
  assert.deepEqual(wanted(lock, '@tauri-apps/cli-', 'linux', 'x64'), []);
});

test('an installed package at the locked version is left alone', () => {
  const dir = project({ '@tauri-apps/cli-win32-x64-msvc': '2.12.4' });
  const calls = [];
  const result = ensure(dir, '@tauri-apps/cli-', { install: (...a) => calls.push(a), platform: 'win32', arch: 'x64' });
  assert.deepEqual(calls, []);
  assert.deepEqual(result.notes, []);
  rmSync(dir, { recursive: true });
});

test('a missing package is installed once, at the locked version', () => {
  const dir = project();
  const calls = [];
  const install = (d, spec) => {
    calls.push(spec);
    mkdirSync(join(d, 'node_modules/@tauri-apps/cli-win32-x64-msvc'), { recursive: true });
    writeFileSync(join(d, 'node_modules/@tauri-apps/cli-win32-x64-msvc/package.json'), JSON.stringify({ version: '2.12.4' }));
  };
  const result = ensure(dir, '@tauri-apps/cli-', { install, platform: 'win32', arch: 'x64' });
  assert.deepEqual(calls, ['@tauri-apps/cli-win32-x64-msvc@2.12.4']);
  assert.equal(result.notes.length, 1);
  rmSync(dir, { recursive: true });
});

test('a package that is still missing, or at another version, fails the build', () => {
  const dir = project({ '@tauri-apps/cli-win32-x64-msvc': '2.12.3' });
  assert.throws(() => ensure(dir, '@tauri-apps/cli-', { install: () => {}, platform: 'win32', arch: 'x64' }), /still missing/);
  rmSync(dir, { recursive: true });
});
