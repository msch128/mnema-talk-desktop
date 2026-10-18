import assert from 'node:assert/strict';
import { createHash, generateKeyPairSync, sign } from 'node:crypto';
import { test } from 'node:test';
import { verifyManifest, verifyPackage } from './verify-updater-signatures.mjs';

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
