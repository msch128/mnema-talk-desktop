import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { SHARED_VECTORS } from './check-native-test-vectors.mjs';

const toml = readFileSync(new URL('../.gitleaks.toml', import.meta.url), 'utf8');
const pathRegexes = [...toml.matchAll(/^\s*'''(\^.*\$)'''\s*$/gm)].map(m => new RegExp(m[1]));

test('the shared test-vectors exemption lists exactly the files the script pins by name', () => {
  for (const path of SHARED_VECTORS) assert.ok(pathRegexes.some(re => re.test(path)), `${path} is not exempt`);
  assert.ok(!pathRegexes.some(re => re.test('test-vectors/new.json')), 'an unlisted vector must not be exempt');
});
