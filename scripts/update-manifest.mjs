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

// The macOS app is Apple silicon only.
export const MAC_PLATFORMS = ['darwin-aarch64'];

// The Linux client (Electron) updates its AppImage from the same feed. Tauri
// clients look up their own platform key and ignore the rest of the map.
// The .deb has its own keys (`-deb`): the installed package is updated through
// the package manager, so Tauri clients and the AppImage never pick it up.
export const LINUX_PLATFORMS = ['linux-x86_64', 'linux-aarch64', 'linux-x86_64-deb', 'linux-aarch64-deb'];

function checkedSignature(signature, what) {
  const sig = signature.trim();
  if (!SIGNATURE.test(sig) || sig.length < 100) throw new Error(`${what} signature must be the base64 text of a Tauri .sig file.`);
  return sig;
}

export function buildManifest({ version, repo, installerName, signature, mac, linux = [], notes = '', date }) {
  if (!VERSION.test(version)) throw new Error('Version must be MAJOR.MINOR.PATCH.');
  if (!REPO.test(repo)) throw new Error('Repository must be owner/name.');
  if (!NAME.test(installerName)) throw new Error('Installer name must use portable filename characters only.');
  const sig = checkedSignature(signature, 'Installer');
  if (!(date instanceof Date) || Number.isNaN(date.getTime())) throw new Error('A valid date is required.');
  const platforms = {
    'windows-x86_64': {
      signature: sig,
      url: `https://github.com/${repo}/releases/download/v${version}/${installerName}`
    }
  };
  if (mac) {
    if (!NAME.test(mac.name)) throw new Error('macOS archive name must use portable filename characters only.');
    const macSig = checkedSignature(mac.signature, 'macOS');
    for (const platform of MAC_PLATFORMS) {
      platforms[platform] = { signature: macSig, url: `https://github.com/${repo}/releases/download/v${version}/${mac.name}` };
    }
  }
  const seen = new Set();
  for (const entry of linux) {
    if (!LINUX_PLATFORMS.includes(entry.platform)) throw new Error(`Linux platform must be one of ${LINUX_PLATFORMS.join(', ')}.`);
    if (seen.has(entry.platform)) throw new Error(`Linux platform ${entry.platform} is listed twice.`);
    seen.add(entry.platform);
    const ext = entry.platform.endsWith('-deb') ? '.deb' : '.AppImage';
    if (!NAME.test(entry.name) || !entry.name.endsWith(ext)) throw new Error(`Linux package name for ${entry.platform} must be a portable ${ext} filename.`);
    platforms[entry.platform] = {
      signature: checkedSignature(entry.signature, entry.platform),
      url: `https://github.com/${repo}/releases/download/v${version}/${entry.name}`
    };
  }
  return {
    version,
    notes: notes.trim().slice(0, 2000),
    pub_date: date.toISOString(),
    platforms
  };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const args = Object.fromEntries(
    process.argv.slice(2).reduce((pairs, arg, i, all) => (arg.startsWith('--') ? [...pairs, [arg.slice(2), all[i + 1]]] : pairs), [])
  );
  const { version, repo, installer, sig, notes, out, mac, 'mac-sig': macSig } = args;
  if (!version || !repo || !installer || !sig || !out) {
    throw new Error('Usage: update-manifest.mjs --version X.Y.Z --repo owner/name --installer <file> --sig <file> [--mac <app.tar.gz> --mac-sig <file>] [--linux-x86_64 <AppImage> --linux-x86_64-sig <file>] [--linux-aarch64 <AppImage> --linux-aarch64-sig <file>] [--linux-x86_64-deb <deb> --linux-x86_64-deb-sig <file>] [--linux-aarch64-deb <deb> --linux-aarch64-deb-sig <file>] [--notes <file>] --out latest.json');
  }
  if (Boolean(mac) !== Boolean(macSig)) throw new Error('--mac and --mac-sig go together.');
  const linux = [];
  for (const platform of LINUX_PLATFORMS) {
    const file = args[platform];
    const fileSig = args[`${platform}-sig`];
    if (Boolean(file) !== Boolean(fileSig)) throw new Error(`--${platform} and --${platform}-sig go together.`);
    if (file) linux.push({ platform, name: basename(file), signature: readFileSync(fileSig, 'utf8') });
  }
  const manifest = buildManifest({
    version,
    repo,
    installerName: basename(installer),
    signature: readFileSync(sig, 'utf8'),
    mac: mac ? { name: basename(mac), signature: readFileSync(macSig, 'utf8') } : undefined,
    linux,
    notes: notes ? readFileSync(notes, 'utf8') : '',
    date: new Date()
  });
  writeFileSync(out, `${JSON.stringify(manifest, null, 2)}\n`);
}
