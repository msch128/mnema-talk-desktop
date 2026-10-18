import assert from 'node:assert/strict';
import { createHash, generateKeyPairSync, sign } from 'node:crypto';
import { test } from 'node:test';
import { verifyManifest, verifyPackage } from './verify-updater-signatures.mjs';
import { buildManifest } from './update-manifest.mjs';

const b64 = (text) => Buffer.from(text).toString('base64');

// A minisign key pair and signer, in the format the Tauri CLI writes.
function signer(keyId = Buffer.from('0123456789abcdef', 'hex')) {
  const { publicKey, privateKey } = generateKeyPairSync('ed25519');
  const raw = publicKey.export({ format: 'der', type: 'spki' }).subarray(-32);
  const pubkey = b64(`untrusted comment: minisign public key\n${Buffer.concat([Buffer.from('Ed'), keyId, raw]).toString('base64')}\n`);
  return {
    pubkey,
    sign(content, version, prehashed = true) {
      const message = prehashed ? createHash('blake2b512').update(content).digest() : content;
      const signature = sign(null, message, privateKey);
      const trusted = `timestamp:1\tfile:x\tversion:${version}`;
      const global = sign(null, Buffer.concat([signature, Buffer.from(trusted)]), privateKey);
      const head = Buffer.concat([Buffer.from(prehashed ? 'ED' : 'Ed'), keyId, signature]);
      return b64(`untrusted comment: signature\n${head.toString('base64')}\ntrusted comment: ${trusted}\n${global.toString('base64')}\n`);
    }
  };
}

const feed = (version, platforms) => ({
  version,
  platforms: Object.fromEntries(Object.entries(platforms).map(([p, [name, signature]]) => [p, { signature, url: `https://github.com/o/r/releases/download/v${version}/${name}` }]))
});

test('accepts a package signed by the pinned key, prehashed or not', () => {
  const s = signer();
  const data = Buffer.from('package');
  assert.equal(verifyPackage(data, s.sign(data, '1.2.3'), s.pubkey), '1.2.3');
  assert.equal(verifyPackage(data, s.sign(data, '1.2.3', false), s.pubkey), '1.2.3');
});

test('refuses another key, changed content and a forged trusted comment', () => {
  const s = signer();
  const data = Buffer.from('package');
  assert.throws(() => verifyPackage(data, s.sign(data, '1.2.3'), signer(Buffer.from('1111111111111111', 'hex')).pubkey), /different key/);
  assert.throws(() => verifyPackage(data, s.sign(data, '1.2.3'), signer().pubkey), /does not match/);
  assert.throws(() => verifyPackage(Buffer.from('other'), s.sign(data, '1.2.3'), s.pubkey), /does not match/);
  const forged = Buffer.from(s.sign(data, '1.2.3'), 'base64').toString().replace('version:1.2.3', 'version:9.9.9');
  assert.throws(() => verifyPackage(data, b64(forged), s.pubkey), /trusted comment/);
  assert.throws(() => verifyPackage(data, b64('nonsense'), s.pubkey), /minisign/);
});

test('checks every platform of the feed and the signed version', () => {
  const s = signer();
  const files = { 'a-setup.exe': Buffer.from('exe'), 'a.app.tar.gz': Buffer.from('app') };
  const read = (name) => files[name];
  const good = feed('1.2.3', {
    'windows-x86_64': ['a-setup.exe', s.sign(files['a-setup.exe'], '1.2.3')],
    'darwin-aarch64': ['a.app.tar.gz', s.sign(files['a.app.tar.gz'], '1.2.3')]
  });
  assert.deepEqual(verifyManifest(good, s.pubkey, read, ['windows-x86_64', 'darwin-aarch64']), ['windows-x86_64', 'darwin-aarch64']);
  assert.throws(() => verifyManifest(feed('1.2.3', { 'windows-x86_64': good.platforms['windows-x86_64'] && ['a-setup.exe', s.sign(files['a-setup.exe'], '1.2.3')] }), s.pubkey, read, ['darwin-aarch64']), /no darwin-aarch64/);
  assert.throws(() => verifyManifest({ ...good, version: '1.2.4' }, s.pubkey, read), /covers version 1\.2\.3/);
  assert.throws(() => verifyManifest(good, s.pubkey, (name) => (name === 'a.app.tar.gz' ? Buffer.from('tampered') : files[name])), /darwin-aarch64 .*does not match/);
});

test('checks the Linux AppImages of the feed like every other platform', () => {
  const s = signer();
  const files = { 'a-setup.exe': Buffer.from('exe'), 'a_x86_64.AppImage': Buffer.from('image-x64'), 'a_arm64.AppImage': Buffer.from('image-arm') };
  const read = (name) => files[name];
  const good = feed('1.2.3', {
    'windows-x86_64': ['a-setup.exe', s.sign(files['a-setup.exe'], '1.2.3')],
    'linux-x86_64': ['a_x86_64.AppImage', s.sign(files['a_x86_64.AppImage'], '1.2.3')],
    'linux-aarch64': ['a_arm64.AppImage', s.sign(files['a_arm64.AppImage'], '1.2.3')]
  });
  assert.deepEqual(verifyManifest(good, s.pubkey, read, ['windows-x86_64', 'linux-x86_64', 'linux-aarch64']), ['windows-x86_64', 'linux-x86_64', 'linux-aarch64']);
  // A required platform that the feed lacks.
  const withoutArm = { ...good, platforms: { ...good.platforms } };
  delete withoutArm.platforms['linux-aarch64'];
  assert.throws(() => verifyManifest(withoutArm, s.pubkey, read, ['linux-aarch64']), /no linux-aarch64/);
  // An AppImage that changed after signing, and one signed for another version.
  assert.throws(() => verifyManifest(good, s.pubkey, (name) => (name === 'a_x86_64.AppImage' ? Buffer.from('tampered') : files[name])), /linux-x86_64 .*does not match/);
  const stale = feed('1.2.3', { 'linux-x86_64': ['a_x86_64.AppImage', s.sign(files['a_x86_64.AppImage'], '1.2.2')] });
  assert.throws(() => verifyManifest(stale, s.pubkey, read, ['linux-x86_64']), /covers version 1\.2\.2/);
});

test('a generated feed with Linux entries still verifies for the Tauri platforms and is looked up by platform key', () => {
  const s = signer();
  const files = { 'w-setup.exe': Buffer.from('exe'), 'm.app.tar.gz': Buffer.from('app'), 'l_x86_64.AppImage': Buffer.from('l1'), 'l_arm64.AppImage': Buffer.from('l2') };
  const sig = (name) => s.sign(files[name], '0.2.0');
  const manifest = buildManifest({
    version: '0.2.0', repo: 'o/r', installerName: 'w-setup.exe', signature: sig('w-setup.exe'), date: new Date('2026-10-17T12:00:00Z'),
    mac: { name: 'm.app.tar.gz', signature: sig('m.app.tar.gz') },
    linux: [{ platform: 'linux-x86_64', name: 'l_x86_64.AppImage', signature: sig('l_x86_64.AppImage') }, { platform: 'linux-aarch64', name: 'l_arm64.AppImage', signature: sig('l_arm64.AppImage') }]
  });
  const read = (name) => files[name];
  // The Tauri updater parses `platforms` as a map and reads only the key of its own target (TargetNotFound otherwise),
  // so what Windows and macOS clients see is the entries they always saw, and they verify.
  const json = JSON.parse(JSON.stringify(manifest));
  assert.deepEqual(verifyManifest({ ...json, platforms: { 'windows-x86_64': json.platforms['windows-x86_64'], 'darwin-aarch64': json.platforms['darwin-aarch64'] } }, s.pubkey, read, ['windows-x86_64', 'darwin-aarch64']), ['windows-x86_64', 'darwin-aarch64']);
  assert.deepEqual(verifyManifest(json, s.pubkey, read, ['windows-x86_64', 'darwin-aarch64', 'linux-x86_64', 'linux-aarch64']), ['windows-x86_64', 'darwin-aarch64', 'linux-x86_64', 'linux-aarch64']);
  for (const entry of Object.values(json.platforms)) assert.deepEqual(Object.keys(entry).sort(), ['signature', 'url']);
});
