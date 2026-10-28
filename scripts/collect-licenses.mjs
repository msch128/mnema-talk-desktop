#!/usr/bin/env node
// Collect verbatim dependency license notices of the probe tree. No downloads and no signing.
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { collectLicenseBundle } from './license-bundle.mjs';

const desktop = resolve(dirname(fileURLToPath(import.meta.url)), '..');
collectLicenseBundle({
  desktop,
  cargoDir: desktop,
  cargoFeatures: 'shell',
  cargoLock: join(desktop, 'Cargo.lock'),
  toolchainFile: join(desktop, 'rust-toolchain.toml'),
  npmDir: join(desktop, 'ui'),
  argv: process.argv.slice(2),
});
