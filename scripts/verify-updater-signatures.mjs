#!/usr/bin/env node
// Checks every platform entry of an updater feed (latest.json) against the
// public key pinned in tauri.conf.json, the way the client does: minisign
// signature over the package, the same key id, and the signed version equal to
// the feed's version. Run on the assembled release before it is published.
// A release rehearsal signs with throwaway keys (one per signing job) and sets
// UPDATER_PUBKEYS to their public keys, separated by whitespace; a release never
// sets it and is checked against the pinned key only.
import { createHash, createPublicKey, verify } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ED25519_SPKI_PREFIX = Buffer.from('302a300506032b6570032100', 'hex');

// The text of a minisign file (public key or signature): comment, base64 line, ...
function lines(text) {
  return text.replace(/\r/g, '').split('\n');
}

function decodeText(base64) {
  return Buffer.from(base64.trim(), 'base64').toString('utf8');
}

export function parsePublicKey(pubkeyBase64) {
  const bytes = Buffer.from(lines(decodeText(pubkeyBase64))[1] ?? '', 'base64');
  if (bytes.length !== 42 || bytes.toString('latin1', 0, 2) !== 'Ed') throw new Error('The pinned key is not a minisign public key.');
  return { keyId: bytes.subarray(2, 10), raw: bytes.subarray(10) };
}

export function parseSignature(signatureBase64) {
  const [, sigLine, trustedLine, globalLine] = lines(decodeText(signatureBase64));
  const bytes = Buffer.from(sigLine ?? '', 'base64');
  const algorithm = bytes.toString('latin1', 0, 2);
  if (bytes.length !== 74 || (algorithm !== 'Ed' && algorithm !== 'ED')) throw new Error('Not a minisign signature.');
  if (!trustedLine?.startsWith('trusted comment: ')) throw new Error('The signature has no trusted comment.');
  const global = Buffer.from(globalLine ?? '', 'base64');
  if (global.length !== 64) throw new Error('The signature has no global signature.');
  return { algorithm, keyId: bytes.subarray(2, 10), signature: bytes.subarray(10), trustedComment: trustedLine.slice('trusted comment: '.length), global };
}

/** Returns the version named in the signed comment, or throws. */
export function verifyPackage(content, signatureBase64, pubkeyBase64) {
  const sig = parseSignature(signatureBase64);
  // One key, or several of which the signature names one by its key id.
  const keys = [pubkeyBase64].flat().map(parsePublicKey);
  const key = keys.find((candidate) => candidate.keyId.equals(sig.keyId)) ?? keys[0];
  if (!key.keyId.equals(sig.keyId)) throw new Error('Signed with a different key than the pinned one.');
  const publicKey = createPublicKey({ key: Buffer.concat([ED25519_SPKI_PREFIX, key.raw]), format: 'der', type: 'spki' });
  const message = sig.algorithm === 'ED' ? createHash('blake2b512').update(content).digest() : content;
  if (!verify(null, message, publicKey, sig.signature)) throw new Error('The package does not match its signature.');
  const trusted = Buffer.concat([sig.signature, Buffer.from(sig.trustedComment)]);
  if (!verify(null, trusted, publicKey, sig.global)) throw new Error('The trusted comment is not signed by the pinned key.');
  return /(?:^|\t)version:(\S+)/.exec(sig.trustedComment)?.[1];
}

/** `manifest` is the parsed feed; `read(name)` returns the bytes of a release asset. */
export function verifyManifest(manifest, pubkeyBase64, read, required = []) {
  const found = Object.keys(manifest.platforms ?? {});
  for (const platform of required) {
    if (!found.includes(platform)) throw new Error(`The feed has no ${platform} entry.`);
  }
  for (const [platform, entry] of Object.entries(manifest.platforms ?? {})) {
    const name = basename(new URL(entry.url).pathname);
    // The -deb keys are for the .deb only; no other entry may point at one.
    if (platform.endsWith('-deb') !== name.endsWith('.deb')) throw new Error(`${platform} (${name}): a .deb belongs under a -deb platform key and only there.`);
    try {
      const signed = verifyPackage(read(name), entry.signature, pubkeyBase64);
      if (signed !== manifest.version) throw new Error(`The signature covers version ${signed ?? 'none'}, the feed says ${manifest.version}.`);
    } catch (error) {
      throw new Error(`${platform} (${name}): ${error.message}`);
    }
  }
  return found;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const [manifestPath, dir, ...required] = process.argv.slice(2);
  if (!manifestPath || !dir) {
    throw new Error('Usage: verify-updater-signatures.mjs <latest.json> <dir with the release files> [platform...]');
  }
  const conf = JSON.parse(readFileSync(join(resolve(dirname(fileURLToPath(import.meta.url)), '..'), 'experimental', 'app', 'tauri.conf.json'), 'utf8'));
  const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'));
  try {
    const rehearsal = (process.env.UPDATER_PUBKEYS ?? '').split(/\s+/).filter(Boolean);
    const pinned = rehearsal.length > 0 ? rehearsal : conf.plugins.updater.pubkey;
    const platforms = verifyManifest(manifest, pinned, (name) => readFileSync(join(dir, name)), required);
    console.log(`Verified ${platforms.join(', ')} for ${manifest.version} against ${rehearsal.length > 0 ? `${rehearsal.length} rehearsal key(s)` : 'the pinned key'}.`);
  } catch (error) {
    console.error(`::error::${error.message}`);
    process.exit(1);
  }
}
