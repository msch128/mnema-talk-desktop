import assert from 'node:assert/strict';
import { readFileSync, existsSync } from 'node:fs';
import { test } from 'node:test';

test('codeql workflow and config are present and valid', () => {
  const workflowPath = new URL('../.github/workflows/codeql.yml', import.meta.url);
  assert.ok(existsSync(workflowPath), 'codeql.yml must exist');

  const workflow = readFileSync(workflowPath, 'utf8');

  // Verify triggers
  assert.match(workflow, /push:\s*\n\s*branches:\s*\[main\]/, 'must trigger on push to main');
  assert.match(workflow, /pull_request:\s*\n/, 'must trigger on pull_request');
  assert.match(workflow, /schedule:\s*\n\s*-\s*cron:\s*["'][^"']+["']/, 'must have scheduled trigger');

  // Verify permissions
  assert.match(workflow, /permissions:\s*\n\s*contents:\s*read/, 'top-level permission must be contents: read');
  assert.match(workflow, /security-events:\s*write/, 'job permission must include security-events: write');
  assert.match(workflow, /actions:\s*read/, 'job permission must include actions: read');

  // Verify concurrency cancel-in-progress
  assert.match(
    workflow,
    /cancel-in-progress:\s*\$\{\{\s*github\.event_name\s*==\s*['"]pull_request['"]\s*\}\}/,
    'cancel-in-progress must only be active for pull_request'
  );

  // Verify languages and build modes
  assert.match(workflow, /language:\s*actions[\s\S]*build-mode:\s*none/, 'actions language must use build-mode none');
  assert.match(workflow, /language:\s*javascript-typescript[\s\S]*build-mode:\s*none/, 'javascript-typescript language must use build-mode none');
  assert.match(workflow, /language:\s*rust[\s\S]*build-mode:\s*none/, 'rust language must use build-mode none');

  // Verify pinned action commit SHAs with comments
  const usesLines = workflow.match(/^\s*-\s*uses:\s*[^@\n]+@[^\n]+/gm) || [];
  assert.ok(usesLines.length >= 3, 'must pin at least 3 actions');
  for (const line of usesLines) {
    assert.match(line, /@[0-9a-f]{40}\s+#\s+v[0-9]+/, `action line must be pinned to 40-char SHA with version comment: ${line}`);
  }

  // Verify codeql config file
  const configPath = new URL('../.github/codeql/codeql-config.yml', import.meta.url);
  assert.ok(existsSync(configPath), 'codeql-config.yml must exist');

  const config = readFileSync(configPath, 'utf8');
  assert.match(config, /paths-ignore:/, 'codeql-config.yml must define paths-ignore');
  assert.match(config, /node_modules/, 'paths-ignore must exclude node_modules');
  assert.match(config, /dist/, 'paths-ignore must exclude dist');
});
