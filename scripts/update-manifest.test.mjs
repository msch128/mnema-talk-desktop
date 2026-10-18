import assert from 'node:assert/strict';
import { test } from 'node:test';
import { LINUX_PLATFORMS, buildManifest } from './update-manifest.mjs';

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

test('adds the Linux AppImages next to the Tauri platforms without touching their entries', () => {
  const macSignature = Buffer.from('untrusted comment: mac signature from tauri secret key\n'.repeat(3)).toString('base64');
  const linuxSignature = Buffer.from('untrusted comment: linux signature from tauri secret key\n'.repeat(3)).toString('base64');
  const base = buildManifest({ ...ok, mac: { name: 'Mnema-Desktop-DEV_0.2.0_aarch64.app.tar.gz', signature: macSignature } });
  const m = buildManifest({
    ...ok,
    mac: { name: 'Mnema-Desktop-DEV_0.2.0_aarch64.app.tar.gz', signature: macSignature },
    linux: [
      { platform: 'linux-x86_64', name: 'Mnema-Desktop-DEV_0.2.0_x86_64.AppImage', signature: `${linuxSignature}\n` },
      { platform: 'linux-aarch64', name: 'Mnema-Desktop-DEV_0.2.0_arm64.AppImage', signature: linuxSignature }
    ]
  });
  assert.deepEqual(LINUX_PLATFORMS, ['linux-x86_64', 'linux-aarch64', 'linux-x86_64-deb', 'linux-aarch64-deb']);
  assert.deepEqual(Object.keys(m.platforms), ['windows-x86_64', 'darwin-aarch64', 'linux-x86_64', 'linux-aarch64']);
  assert.equal(m.platforms['linux-x86_64'].url, 'https://github.com/msch128/mnema-talk-desktop/releases/download/v0.2.0/Mnema-Desktop-DEV_0.2.0_x86_64.AppImage');
  assert.equal(m.platforms['linux-x86_64'].signature, linuxSignature);
  // What the Windows and macOS updaters read is byte for byte what it was.
  for (const platform of ['windows-x86_64', 'darwin-aarch64']) assert.deepEqual(m.platforms[platform], base.platforms[platform]);
  assert.deepEqual({ ...m, platforms: { 'windows-x86_64': m.platforms['windows-x86_64'], 'darwin-aarch64': m.platforms['darwin-aarch64'] } }, base);
});

test('refuses Linux entries that are not plain', () => {
  const linuxSignature = Buffer.from('untrusted comment: linux signature from tauri secret key\n'.repeat(3)).toString('base64');
  const entry = { platform: 'linux-x86_64', name: 'a.AppImage', signature: linuxSignature };
  assert.doesNotThrow(() => buildManifest({ ...ok, linux: [entry] }));
  for (const bad of [{ platform: 'linux-riscv64' }, { platform: 'windows-x86_64' }, { name: 'a.deb' }, { platform: 'linux-x86_64-deb' }, { name: '../a.AppImage' }, { name: 'a b.AppImage' }, { signature: 'short' }]) {
    assert.throws(() => buildManifest({ ...ok, linux: [{ ...entry, ...bad }] }), Error, JSON.stringify(bad));
  }
  assert.throws(() => buildManifest({ ...ok, linux: [entry, entry] }), /twice/);
});

test('adds the .deb entries under their own keys and leaves every other entry byte-identical', () => {
  const sig = Buffer.from('untrusted comment: linux signature from tauri secret key\n'.repeat(3)).toString('base64');
  const base = buildManifest({ ...ok, linux: [{ platform: 'linux-x86_64', name: 'a.AppImage', signature: sig }] });
  const m = buildManifest({
    ...ok,
    linux: [
      { platform: 'linux-x86_64', name: 'a.AppImage', signature: sig },
      { platform: 'linux-x86_64-deb', name: 'a_amd64.deb', signature: sig },
      { platform: 'linux-aarch64-deb', name: 'a_arm64.deb', signature: sig }
    ]
  });
  for (const key of Object.keys(base.platforms)) assert.deepEqual(m.platforms[key], base.platforms[key]);
  assert.match(m.platforms['linux-x86_64-deb'].url, /\/a_amd64\.deb$/);
  assert.throws(() => buildManifest({ ...ok, linux: [{ platform: 'linux-x86_64-deb', name: 'a.AppImage', signature: sig }] }), /\.deb/);
  assert.throws(() => buildManifest({ ...ok, linux: [{ platform: 'linux-x86_64', name: 'a.deb', signature: sig }] }), /AppImage/);
});
