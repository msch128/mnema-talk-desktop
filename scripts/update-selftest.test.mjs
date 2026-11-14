import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { STEPS, feed, judge, parseArgs, selftestConfig } from './update-selftest.mjs';

const good = ['version 1.0.0', ...STEPS.slice(0, 6).map((s) => `ok ${s}`), `ok ${STEPS[6]}`, 'version 1.0.1', `ok ${STEPS[7]}`, 'done'].join('\n');

test('a record of the whole update passes', () => {
  assert.deepEqual(judge(`${good}\n`, '1.0.0', '1.0.1'), []);
});

test('a failure, a missing or reordered step, a wrong version or no end fails', () => {
  assert.match(judge(good.replace('ok the Talk ends', 'fail the Talk ends'), '1.0.0', '1.0.1').join(), /the Talk ends|missing/);
  assert.match(judge(good.replace(`ok ${STEPS[4]}\n`, ''), '1.0.0', '1.0.1').join(), /missing: nothing installs/);
  const swapped = good.replace(`ok ${STEPS[0]}`, 'X').replace(`ok ${STEPS[2]}`, `ok ${STEPS[0]}`).replace('X', `ok ${STEPS[2]}`);
  assert.match(judge(swapped, '1.0.0', '1.0.1').join(), /out of order/);
  assert.match(judge(good.replace('version 1.0.1', 'version 1.0.0'), '1.0.0', '1.0.1').join(), /restarted program reported 1\.0\.0/);
  assert.match(judge(good.replace(/\ndone$/, ''), '1.0.0', '1.0.1').join(), /does not end with done/);
  assert.notDeepEqual(judge('', '1.0.0', '1.0.1'), []);
});

test('the feed names each package on this machine with the signature beside it', () => {
  const dir = mkdtempSync(join(tmpdir(), 'upd-'));
  const file = join(dir, 'Mnema Desktop_1.0.1_x64-setup.exe');
  writeFileSync(file, 'x');
  writeFileSync(`${file}.sig`, 'c2ln\n');
  const body = feed('1.0.1', 47613, [{ key: 'windows-x86_64', file }]);
  assert.equal(body.version, '1.0.1');
  assert.deepEqual(body.platforms['windows-x86_64'], { signature: 'c2ln', url: 'http://127.0.0.1:47613/Mnema%20Desktop_1.0.1_x64-setup.exe' });
});

test('every argument is required once, entries may repeat', () => {
  const args = parseArgs(['--port', '1', '--version', '1.0.1', '--old', '1.0.0', '--record', 'r', '--run', 'p', '--entry', 'a=b', '--entry', 'c=d']);
  assert.deepEqual(args.entry, ['a=b', 'c=d']);
  assert.throws(() => parseArgs(['--port', '1']), /required/);
  assert.throws(() => parseArgs(['--port', '--version']), /Bad argument/);
});

test('a test build trusts the throwaway key and reads the feed on this machine only', () => {
  // The shape of a .pub file's text, made here: it is no key.
  const key = Buffer.from('untrusted comment: minisign public key: 1234\n').toString('base64');
  const config = selftestConfig(`${key}\n`, 47613, '1.0.1', true);
  assert.equal(config.version, '1.0.1');
  assert.equal(config.bundle.createUpdaterArtifacts, true);
  assert.deepEqual(config.plugins.updater, { pubkey: key, endpoints: ['http://127.0.0.1:47613/latest.json'], dangerousInsecureTransportProtocol: true });
  assert.equal(selftestConfig(key, 1, '1.0.0', false).bundle.createUpdaterArtifacts, false);
  assert.throws(() => selftestConfig(key, 1, 'v1', true), /MAJOR/);
  assert.throws(() => selftestConfig('x', 1, '1.0.0', true), /public key/);
});
