import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, copyFileSync, rmSync, unlinkSync, realpathSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';

const scripts = dirname(fileURLToPath(import.meta.url));
const source = 'registry+https://github.com/rust-lang/crates.io-index';
// A stand-in for experimental/scripts/collect-licenses.mjs that points the shared collector at the fixture tree.
const collector = `import { dirname, join, resolve } from 'node:path';
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
`;

function fixture() {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'mnema-license-check-')));
  const desktop = join(root, 'desktop');
  for (const dir of ['scripts', 'licenses', 'ui/node_modules/example', 'crate']) {
    mkdirSync(join(desktop, dir), { recursive: true });
  }
  writeFileSync(join(desktop, 'scripts', 'collect-licenses.mjs'), collector);
  copyFileSync(join(scripts, 'license-bundle.mjs'), join(desktop, 'scripts', 'license-bundle.mjs'));
  const put = (path, value) => writeFileSync(join(desktop, path), typeof value === 'string' ? value : JSON.stringify(value));
  put('Cargo.lock', `version = 4\n[[package]]\nname = "example"\nversion = "1.0.0"\nsource = "${source}"\n`);
  put('rust-toolchain.toml', 'channel = "1.99.1"\n');
  put('licenses/upstream-overrides.json', {});
  put('licenses/RUST-COPYRIGHT-library.html', 'fixture runtime notice');
  put('licenses/supplemental.json', { rust_standard_library: {
    version: '1.99.1', file: 'RUST-COPYRIGHT-library.html',
    sha256: createHash('sha256').update('fixture runtime notice').digest('hex'),
  } });
  put('crate/LICENSE', 'Fixture crate permission and copyright notice.');
  put('crate/Cargo.toml', '[package]\nname="example"\nversion="1.0.0"\n');
  put('metadata.json', { packages: [{ name: 'example', version: '1.0.0', source,
    license: 'MIT', manifest_path: join(desktop, 'crate', 'Cargo.toml') }] });
  put('ui/package-lock.json', { packages: {
    '': { name: 'probe', version: '0.0.0' },
    'node_modules/example': { version: '1.0.0', license: 'MIT' },
  } });
  put('ui/node_modules/example/package.json', { name: 'example', version: '1.0.0' });
  put('ui/node_modules/example/LICENSE', 'Fixture npm permission and copyright notice.');
  const run = (...args) => spawnSync(process.execPath, [join(desktop, 'scripts', 'collect-licenses.mjs'),
    join(desktop, 'metadata.json'), ...args], { encoding: 'utf8' });
  return { root, desktop, put, run, close: () => rmSync(root, { recursive: true, force: true }) };
}

test('license collection is reproducible and omits local paths', () => {
  const f = fixture();
  try {
    assert.equal(f.run().status, 0);
    const manifest = readFileSync(join(f.desktop, 'licenses', 'manifest.json'), 'utf8');
    assert.equal(manifest.includes(f.root), false);
    assert.equal(JSON.parse(manifest).packages.length, 2);
    assert.equal(f.run('--check').status, 0);
    assert.equal(f.run().status, 0);
    assert.equal(readFileSync(join(f.desktop, 'licenses', 'manifest.json'), 'utf8'), manifest);
    f.put('crate/LICENSE', 'changed upstream text');
    assert.notEqual(f.run('--check').status, 0);
  } finally { f.close(); }
});

test('missing license text and stale metadata fail instead of yielding an incomplete inventory', () => {
  const f = fixture();
  try {
    unlinkSync(join(f.desktop, 'crate', 'LICENSE'));
    assert.notEqual(f.run().status, 0);
    f.put('crate/LICENSE', 'Fixture crate notice.');
    f.put('Cargo.lock', `version = 4\n[[package]]\nname = "example"\nversion = "2.0.0"\nsource = "${source}"\n`);
    assert.notEqual(f.run().status, 0);
  } finally { f.close(); }
});

test('runtime notice integrity and installed npm versions are checked', () => {
  const f = fixture();
  try {
    f.put('licenses/RUST-COPYRIGHT-library.html', 'tampered runtime notice');
    assert.notEqual(f.run().status, 0);
    f.put('licenses/RUST-COPYRIGHT-library.html', 'fixture runtime notice');
    f.put('ui/node_modules/example/package.json', { name: 'example', version: '2.0.0' });
    assert.notEqual(f.run().status, 0);
  } finally { f.close(); }
});

test('original notices named MIT.LICENSE and component.h.LICENSE are preserved', () => {
  const f = fixture();
  try {
    unlinkSync(join(f.desktop, 'crate/LICENSE'));
    mkdirSync(join(f.desktop, 'crate/LICENSE'), { recursive: true });
    f.put('crate/LICENSE/MIT.LICENSE', 'Original project copyright and permission.');
    f.put('crate/LICENSE/component.h.LICENSE', 'Original component permission.');
    assert.equal(f.run().status, 0);
    const pkg = JSON.parse(readFileSync(join(f.desktop, 'licenses/manifest.json'))).packages.find(pkg => pkg.ecosystem === 'cargo');
    assert.deepEqual(pkg.texts.map(text => text.name), ['LICENSE/MIT.LICENSE', 'LICENSE/component.h.LICENSE']);
    assert.equal(f.run('--check').status, 0);
  } finally { f.close(); }
});

test('vendored fonts must match the reviewed original bytes and attribution', () => {
  const f = fixture();
  try {
    mkdirSync(join(f.desktop, 'ui', 'src', 'assets'), { recursive: true });
    f.put('ui/src/assets/font.woff2', 'font fixture');
    const supplemental = JSON.parse(readFileSync(join(f.desktop, 'licenses', 'supplemental.json'), 'utf8'));
    supplemental.rust_standard_library.source_assets = [{
      path: 'ui/src/assets/font.woff2', sha256: createHash('sha256').update('font fixture').digest('hex'),
    }];
    f.put('licenses/supplemental.json', supplemental);
    assert.equal(f.run().status, 0);
    f.put('ui/src/assets/font.woff2', 'replaced font fixture');
    assert.notEqual(f.run('--check').status, 0);
  } finally { f.close(); }
});

test('an npm package without a license file takes its pinned override text, a tampered one fails', () => {
  const f = fixture();
  try {
    unlinkSync(join(f.desktop, 'ui/node_modules/example/LICENSE'));
    assert.notEqual(f.run().status, 0);
    const text = 'Pinned npm license text.';
    const sha256 = createHash('sha256').update(text).digest('hex');
    mkdirSync(join(f.desktop, 'licenses', 'texts'), { recursive: true });
    f.put(`licenses/texts/${sha256}.txt`, text);
    f.put('licenses/upstream-overrides.json', { 'npm:example@1.0.0': [{
      name: 'LICENSE', sha256, source: 'https://www.npmjs.com/package/example/v/1.0.0', provenance: 'upstream-override',
    }] });
    const collected = f.run();
    assert.equal(collected.status, 0, collected.stderr);
    assert.equal(f.run('--check').status, 0);
    f.put(`licenses/texts/${sha256}.txt`, 'tampered');
    assert.notEqual(f.run().status, 0);
  } finally { f.close(); }
});
