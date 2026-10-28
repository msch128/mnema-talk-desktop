import assert from 'node:assert/strict';
import { chmodSync, existsSync, mkdtempSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';

const script = new URL('./stage-web-bundle.sh', import.meta.url).pathname;

// A stand-in `gh`: v3 is the newest release without a bundle, v2 has one, "broken" fails like a network error.
function stage(env = {}) {
  const root = mkdtempSync(join(tmpdir(), 'stage-bundle-'));
  const bin = join(root, 'bin');
  mkdirSync(bin);
  writeFileSync(join(bin, 'gh'), `#!/usr/bin/env bash
if [ "$1 $2" = "release list" ]; then
  case "$*" in *--exclude-pre-releases*) ;; *) echo "pre-releases must be excluded" >&2; exit 2 ;; esac
  printf 'v3\\nv2\\nv1\\n'; exit 0
fi
tag="$3"; dir=""; while [ $# -gt 0 ]; do [ "$1" = "--dir" ] && dir="$2"; shift; done
case "$tag" in
  v3) echo "no assets match the file pattern" >&2; exit 1 ;;
  broken) echo "HTTP 502" >&2; exit 1 ;;
  *) printf "bundle-$tag" > "$dir/mnema-web-$tag.zip" ;;
esac
`);
  chmodSync(join(bin, 'gh'), 0o755);
  const summary = join(root, 'summary.md');
  const dir = join(root, 'out');
  const run = spawnSync('bash', [script], {
    env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, BUNDLE_DIR: dir, GITHUB_STEP_SUMMARY: summary, ...env },
    encoding: 'utf8',
  });
  return { run, dir, summary };
}

test('takes the newest release that carries a bundle and records it', () => {
  const { run, dir, summary } = stage();
  assert.equal(run.status, 0, run.stderr);
  assert.equal(readFileSync(join(dir, 'builtin.zip'), 'utf8'), 'bundle-v2');
  assert.match(readFileSync(summary, 'utf8'), /mnema-talk v2 \(sha256 [0-9a-f]{64}\)/);
});

test('WEB_BUNDLE_TAG pins the release', () => {
  const { run, dir } = stage({ WEB_BUNDLE_TAG: 'v1' });
  assert.equal(run.status, 0, run.stderr);
  assert.equal(readFileSync(join(dir, 'builtin.zip'), 'utf8'), 'bundle-v1');
});

test('a download failure other than a missing asset is an error, not a skipped release', () => {
  const { run, dir } = stage({ WEB_BUNDLE_TAG: 'broken' });
  assert.notEqual(run.status, 0);
  assert.match(run.stdout, /could not download the web bundle/);
  assert.ok(!existsSync(join(dir, 'builtin.zip')));
});

test('a release build without any bundle fails, a plain build only warns', () => {
  assert.notEqual(stage({ WEB_BUNDLE_TAG: 'v3', RELEASE_TAG: 'v1.0.0' }).run.status, 0);
  const plain = stage({ WEB_BUNDLE_TAG: 'v3' });
  assert.equal(plain.run.status, 0);
  assert.match(plain.run.stdout, /::warning::/);
});
