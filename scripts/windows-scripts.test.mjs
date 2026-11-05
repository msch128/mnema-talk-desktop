import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

// The gaming hang check is also run by hand, where RUNNER_TEMP is not set. Join-Path throws on an
// empty root, so the fallback has to be chosen before the path is built.
test('the gaming hang script falls back to TEMP before it builds a path from RUNNER_TEMP', () => {
  const script = readFileSync(new URL('./test-windows-gaming-hang.ps1', import.meta.url), 'utf8');
  assert.doesNotMatch(script, /Join-Path\s+\$env:RUNNER_TEMP/);
  const root = script.indexOf('if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { $env:TEMP }');
  assert.ok(root > 0 && root < script.indexOf('Join-Path $tempRoot'));
});
