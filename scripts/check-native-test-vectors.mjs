#!/usr/bin/env node
// The shared test-vectors/*.json files of both clients are edited with the
// code, so they are pinned by name: .gitleaks.toml exempts exactly the files
// listed in SHARED_VECTORS, and a new file must be added here and there.
// check-native-test-vectors.test.mjs keeps .gitleaks.toml and this list equal.
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

export const SHARED_VECTORS = ['bundle', 'client', 'json', 'minisign', 'proxy', 'store', 'trust', 'zip'].map(name => `test-vectors/${name}.json`);

function main() {
  const args = process.argv.slice(2);
  if (args.length) throw new Error('Usage: check-native-test-vectors.mjs');
  const tracked = execFileSync('git', ['ls-files', 'test-vectors']).toString('utf8').split('\n').filter(f => /^test-vectors\/[a-z]+\.json$/.test(f)).sort();
  if (JSON.stringify(tracked) !== JSON.stringify([...SHARED_VECTORS].sort())) throw new Error(`test-vectors/*.json does not match SHARED_VECTORS: ${tracked.join(', ')}`);
  console.log(`Verified ${SHARED_VECTORS.length} shared test-vector files.`);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) main();
