import assert from 'node:assert/strict';
import { test } from 'node:test';
import { check, versions } from './check-release-version.mjs';

const files = (v) => ({
  conf: JSON.stringify({ version: v }),
  cargoToml: `[package]\nname = "x"\nversion = "${v}"\nedition = "2024"\n[dependencies]\nfoo = { version = "9.9.9" }\n`,
  cargoLock: `[[package]]\nname = "other"\nversion = "9.9.9"\n\n[[package]]\nname = "x"\nversion = "${v}"\n`,
  crate: 'x'
});

test('reads the three versions and accepts a matching tag', () => {
  const found = versions(files('0.2.0'));
  assert.deepEqual(found, { conf: '0.2.0', cargoToml: '0.2.0', cargoLock: '0.2.0' });
  assert.equal(check('v0.2.0', found), null);
});

test('refuses a tag that differs from any of them or is not a plain version', () => {
  const found = versions(files('0.2.0'));
  assert.match(check('v0.2.1', found), /says 0\.2\.0/);
  assert.match(check('0.2.0', found), /not vMAJOR/);
  assert.match(check('v0.2.0-rc1', found), /not vMAJOR/);
  assert.match(check('v0.2.0', { ...found, cargoLock: '0.1.0' }), /cargoLock/);
  assert.match(check('v0.2.0', { ...found, cargoToml: undefined }), /nothing/);
});

test('the Linux client is part of the release when its files are given', () => {
  const linux = (v, root = v) => ({
    linuxPackage: JSON.stringify({ name: 'l', version: v }),
    linuxLock: JSON.stringify({ name: 'l', version: v, packages: { '': { name: 'l', version: root } } })
  });
  const found = versions({ ...files('0.2.0'), ...linux('0.2.0') });
  assert.deepEqual(found, { conf: '0.2.0', cargoToml: '0.2.0', cargoLock: '0.2.0', linuxPackage: '0.2.0', linuxLock: '0.2.0', linuxLockRoot: '0.2.0' });
  assert.equal(check('v0.2.0', found), null);
  assert.match(check('v0.2.0', versions({ ...files('0.2.0'), ...linux('0.1.9') })), /linuxPackage says 0\.1\.9/);
  assert.match(check('v0.2.0', versions({ ...files('0.2.0'), ...linux('0.2.0', '0.1.0') })), /linuxLockRoot says 0\.1\.0/);
});
