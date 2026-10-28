#!/usr/bin/env node
// Collect verbatim dependency license notices of the shared-client host. No downloads and no signing.
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { collectLicenseBundle } from '../../scripts/license-bundle.mjs';

const desktop = resolve(dirname(fileURLToPath(import.meta.url)), '..');
collectLicenseBundle({
  desktop,
  cargoDir: join(desktop, 'app'),
  cargoFeatures: 'shell,web-client',
  cargoLock: join(desktop, 'app', 'Cargo.lock'),
  toolchainFile: join(desktop, 'app', 'rust-toolchain.toml'),
  npmDir: resolve(desktop, '../mnema-talk/web'),
  argv: process.argv.slice(2),
});
