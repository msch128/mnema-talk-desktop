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

// Windows grants SetForegroundWindow only some of the time, so the gaming check retries the focus
// change and verifies it. A run in which the game never took the focus must still fail.
test('the gaming hang script retries taking the focus and still fails when the game never got it', () => {
  const script = readFileSync(new URL('./test-windows-gaming-hang.ps1', import.meta.url), 'utf8');
  assert.match(script, /AttachThreadInput/);
  assert.match(script, /GetForegroundWindow\(\) == h/);
  assert.match(script, /\[Win\]::Acquire\(\$target, \d+, \d+, \[ref\]\$focusLog\)/);
  assert.match(script, /if \(\$focused -lt 1\) \{ throw .*the check proved nothing/);
});
