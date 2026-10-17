#!/usr/bin/env node
// Builds the updater feed (latest.json) for one release. The feed is not
// trusted by the client: it only names an installer and the signature the
// client checks against the public key pinned in the app.
import { readFileSync, writeFileSync } from 'node:fs';
import { basename } from 'node:path';
import { fileURLToPath } from 'node:url';

const VERSION = /^(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})$/;
const REPO = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/;
const NAME = /^[A-Za-z0-9._-]+$/;
// The text of a minisign .sig file, base64 encoded by the Tauri CLI.
const SIGNATURE = /^[A-Za-z0-9+/]+={0,2}$/;

export function buildManifest({ version, repo, installerName, signature, notes = '', date }) {
  if (!VERSION.test(version)) throw new Error('Version must be MAJOR.MINOR.PATCH.');
  if (!REPO.test(repo)) throw new Error('Repository must be owner/name.');
  if (!NAME.test(installerName)) throw new Error('Installer name must use portable filename characters only.');
  const sig = signature.trim();
  if (!SIGNATURE.test(sig) || sig.length < 100) throw new Error('Signature must be the base64 text of a Tauri .sig file.');
  if (!(date instanceof Date) || Number.isNaN(date.getTime())) throw new Error('A valid date is required.');
  return {
    version,
    notes: notes.trim().slice(0, 2000),
    pub_date: date.toISOString(),
    platforms: {
      'windows-x86_64': {
        signature: sig,
        url: `https://github.com/${repo}/releases/download/v${version}/${installerName}`
      }
    }
  };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const args = Object.fromEntries(
    process.argv.slice(2).reduce((pairs, arg, i, all) => (arg.startsWith('--') ? [...pairs, [arg.slice(2), all[i + 1]]] : pairs), [])
  );
  const { version, repo, installer, sig, notes, out } = args;
  if (!version || !repo || !installer || !sig || !out) {
    throw new Error('Usage: update-manifest.mjs --version X.Y.Z --repo owner/name --installer <file> --sig <file> [--notes <file>] --out latest.json');
  }
  const manifest = buildManifest({
    version,
    repo,
    installerName: basename(installer),
    signature: readFileSync(sig, 'utf8'),
    notes: notes ? readFileSync(notes, 'utf8') : '',
    date: new Date()
  });
  writeFileSync(out, `${JSON.stringify(manifest, null, 2)}\n`);
}
