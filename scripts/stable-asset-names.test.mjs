import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { copyStable, plan } from './stable-asset-names.mjs';

// The files publish holds when the copies are made (names as the jobs write them).
const release = (v) => [
  `Mnema-Desktop_${v}_x64-setup.exe`,
  `Mnema-Desktop_${v}_x64-setup.exe.sig`,
  `Mnema-Desktop-v${v}-windows-x64-portable.zip`,
  `Mnema-Desktop_${v}_aarch64.dmg`,
  `Mnema-Desktop_${v}_aarch64.app.tar.gz`,
  `Mnema-Desktop_${v}_aarch64.app.tar.gz.sig`,
  `Mnema-Desktop_${v}_amd64.deb`,
  `Mnema-Desktop_${v}_amd64.deb.sig`,
  `Mnema-Desktop_${v}_arm64.deb`,
  `Mnema-Desktop_${v}_arm64.deb.sig`,
  `Mnema-Desktop_${v}_x86_64.AppImage`,
  `Mnema-Desktop_${v}_x86_64.AppImage.sig`,
  `Mnema-Desktop_${v}_arm64.AppImage`,
  `Mnema-Desktop_${v}_arm64.AppImage.sig`,
  'latest.json',
  'rehearsal-updater-linux.pub'
];

test('maps each user-facing installer to its stable name, never a .sig or the updater archive', () => {
  assert.deepEqual(plan(release('1.3.6'), '1.3.6'), [
    { from: 'Mnema-Desktop_1.3.6_x64-setup.exe', to: 'mnema-desktop-windows-x64-setup.exe' },
    { from: 'Mnema-Desktop-v1.3.6-windows-x64-portable.zip', to: 'mnema-desktop-windows-x64-portable.zip' },
    { from: 'Mnema-Desktop_1.3.6_aarch64.dmg', to: 'mnema-desktop-macos-arm64.dmg' },
    { from: 'Mnema-Desktop_1.3.6_amd64.deb', to: 'mnema-desktop-linux-x64.deb' },
    { from: 'Mnema-Desktop_1.3.6_arm64.deb', to: 'mnema-desktop-linux-arm64.deb' },
    { from: 'Mnema-Desktop_1.3.6_x86_64.AppImage', to: 'mnema-desktop-linux-x64.AppImage' },
    { from: 'Mnema-Desktop_1.3.6_arm64.AppImage', to: 'mnema-desktop-linux-arm64.AppImage' }
  ]);
});

test('refuses a missing, doubled or other-version installer and an existing stable name', () => {
  const files = release('1.3.6');
  assert.throws(() => plan(files.filter((f) => !f.endsWith('.dmg')), '1.3.6'), /macos-arm64\.dmg: expected one file .* found 0/);
  assert.throws(() => plan([...files, 'Mnema-Desktop_1.3.6_x86_64.deb'], '1.3.6'), /linux-x64\.deb: .* found 2/);
  assert.throws(() => plan(release('1.3.5'), '1.3.6'), /found 0/);
  // The names up to 1.3.5 carried DEV; a file under such a name is not a user-facing installer of this release.
  assert.throws(() => plan(files.map((f) => f.replace(/^Mnema-Desktop(?=[_-])/, 'Mnema-Desktop-DEV')), '1.3.6'), /found 0/);
  assert.throws(() => plan([...files, 'mnema-desktop-linux-x64.AppImage'], '1.3.6'), /already exists/);
  assert.throws(() => plan(files, '1.3'), /not MAJOR\.MINOR\.PATCH/);
});

test('the copies are byte-identical and the versioned files stay', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'stable-names-'));
  try {
    for (const [i, name] of release('2.0.1').entries()) writeFileSync(join(dir, name), `content ${i}\n`);
    const before = readdirSync(dir);
    const pairs = await copyStable(dir, '2.0.1');
    assert.equal(pairs.length, 7);
    for (const { from, to } of pairs) assert.deepEqual(readFileSync(join(dir, to)), readFileSync(join(dir, from)));
    assert.deepEqual(readdirSync(dir).sort(), [...before, ...pairs.map((p) => p.to)].sort());
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
