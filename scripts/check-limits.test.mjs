import assert from 'node:assert/strict';
import { test } from 'node:test';
import { isTestFile, measure, offenders, parseAllowlist } from './check-limits.mjs';

const lines = (n) => `${'x\n'.repeat(n)}`;

test('a production file counts every line, a test file has its own limit', () => {
  assert.deepEqual(measure('linux/src/core/a.ts', lines(10)), { production: 10, test: 0 });
  assert.deepEqual(measure('linux/test/a.test.ts', lines(10)), { production: 0, test: 10 });
  assert.ok(isTestFile('experimental/app/src/native_updates/tests.rs'));
  assert.ok(isTestFile('experimental/app/page-tests/src/rail.ts'));
  assert.ok(!isTestFile('linux/src/main/shell.ts'));
});

test('the unit tests of a Rust file are a test module of their own', () => {
  const source = `${lines(300)}#[cfg(test)]\nmod tests {\n${lines(700)}}\n`;
  assert.deepEqual(measure('src/a.rs', source), { production: 300, test: 703 });
  assert.deepEqual(offenders([['src/a.rs', source]]), []);
  assert.deepEqual(offenders([['src/a.rs', `${lines(401)}${source}`]]), ['src/a.rs production']);
  assert.deepEqual(offenders([['src/a.rs', `${lines(10)}#[cfg(test)]\nmod tests {\n${lines(800)}}\n`]]), ['src/a.rs test']);
});

test('the limits are 400 production lines and 800 test lines', () => {
  assert.deepEqual(offenders([['a.ts', lines(400)], ['b.ts', lines(401)], ['c.test.ts', lines(800)], ['d.test.ts', lines(801)]]), ['b.ts production', 'd.test.ts test']);
});

test('the allowlist ignores comments and blank lines', () => {
  assert.deepEqual([...parseAllowlist('# why\n\nsrc/a.rs production\n')], ['src/a.rs production']);
});
