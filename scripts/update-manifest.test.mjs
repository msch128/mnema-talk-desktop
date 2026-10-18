import assert from 'node:assert/strict';
import { test } from 'node:test';
import { buildManifest } from './update-manifest.mjs';

const signature = Buffer.from('untrusted comment: signature from tauri secret key\n'.repeat(3)).toString('base64');
const ok = { version: '0.2.0', repo: 'msch128/mnema-talk-desktop', installerName: 'Mnema-Desktop-DEV_0.2.0_x64-setup.exe', signature: `${signature}\n`, notes: ' Fixes ', date: new Date('2026-10-17T12:00:00Z') };

test('names the installer asset of the same release over https', () => {
  const m = buildManifest(ok);
  assert.equal(m.version, '0.2.0');
  assert.equal(m.notes, 'Fixes');
  assert.equal(m.pub_date, '2026-10-17T12:00:00.000Z');
  assert.deepEqual(Object.keys(m.platforms), ['windows-x86_64']);
  assert.equal(m.platforms['windows-x86_64'].url, 'https://github.com/msch128/mnema-talk-desktop/releases/download/v0.2.0/Mnema-Desktop-DEV_0.2.0_x64-setup.exe');
  assert.equal(m.platforms['windows-x86_64'].signature, signature);
});

test('refuses versions, repositories, names and signatures that are not plain', () => {
  for (const bad of [{ version: '0.2' }, { version: 'v0.2.0' }, { version: '0.2.0-beta' }, { version: '01.2.3' },
    { repo: 'a/b/c' }, { repo: 'evil.example@x/y z' }, { installerName: 'a b.exe' }, { installerName: '../a.exe' },
    { signature: 'short' }, { signature: `${signature}!` }, { signature: `${signature}\n${signature}` }, { date: new Date('x') }]) {
    assert.throws(() => buildManifest({ ...ok, ...bad }), Error, JSON.stringify(bad).slice(0, 40));
  }
});

test('caps the release notes', () => {
  assert.equal(buildManifest({ ...ok, notes: 'x'.repeat(5000) }).notes.length, 2000);
});

test('serves Apple silicon from one signed archive next to Windows', () => {
  const macSignature = Buffer.from('untrusted comment: mac signature from tauri secret key\n'.repeat(3)).toString('base64');
  const m = buildManifest({ ...ok, mac: { name: 'Mnema-Desktop-DEV_0.2.0_aarch64.app.tar.gz', signature: `${macSignature}\n` } });
  assert.deepEqual(Object.keys(m.platforms), ['windows-x86_64', 'darwin-aarch64']);
  for (const platform of ['darwin-aarch64']) {
    assert.equal(m.platforms[platform].url, 'https://github.com/msch128/mnema-talk-desktop/releases/download/v0.2.0/Mnema-Desktop-DEV_0.2.0_aarch64.app.tar.gz');
    assert.equal(m.platforms[platform].signature, macSignature);
  }
  assert.equal(m.platforms['windows-x86_64'].signature, signature);
  assert.throws(() => buildManifest({ ...ok, mac: { name: '../x.tar.gz', signature: macSignature } }), Error);
  assert.throws(() => buildManifest({ ...ok, mac: { name: 'x.tar.gz', signature: 'short' } }), Error);
});
