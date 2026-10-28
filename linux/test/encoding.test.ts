import assert from 'node:assert/strict'
import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync, mkdirSync, statSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, test } from 'node:test'
import { writeFileAtomic } from '../src/core/atomic.ts'
import { strictJson, strictUtf8 } from '../src/core/encoding.ts'
import { ZipArchive } from '../src/core/zip.ts'
import { parseHealth } from '../src/core/client.ts'
import { zipOf } from './helpers/fixtures.ts'

const dirs: string[] = []
afterEach(() => { for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true }) })
const tmp = () => { const d = mkdtempSync(join(tmpdir(), 'mnema-enc-')); dirs.push(d); return d }

test('a byte order mark is kept, not dropped, and invalid UTF-8 is refused', () => {
  assert.equal(strictUtf8(Buffer.from([0xef, 0xbb, 0xbf, 0x61])), '﻿a')
  assert.equal(strictUtf8(Buffer.from([0xff])), null)
  assert.deepEqual(strictJson(Buffer.from('{"a":1}')), { a: 1 })
  assert.throws(() => strictJson(Buffer.from('﻿{"a":1}')))
  assert.throws(() => strictJson(Buffer.from([0x7b, 0xff, 0x7d])))
})

test('a BOM-prefixed document is refused like serde refuses it', () => {
  assert.throws(() => parseHealth(Buffer.from('﻿{"status":"ok"}')), /needs an update/)
})

test('a BOM-prefixed zip entry name does not alias the plain name', () => {
  const zip = new ZipArchive(zipOf([{ name: '﻿index.html', body: Buffer.from('x') }]))
  assert.equal(zip.entries[0]?.name, '﻿index.html')
})

test('an atomic write replaces the file, creates the directory and leaves no temporary file', () => {
  const d = tmp()
  const file = join(d, 'a', 'b', 'state.json')
  writeFileAtomic(file, Buffer.from('one'), { dirMode: 0o700 })
  writeFileAtomic(file, Buffer.from('two'), { dirMode: 0o700 })
  assert.equal(readFileSync(file, 'utf8'), 'two')
  assert.equal(statSync(file).mode & 0o777, 0o600)
  assert.equal(statSync(join(d, 'a', 'b')).mode & 0o777, 0o700)
  assert.deepEqual(readdirSync(join(d, 'a', 'b')), ['state.json'])
})

test('a failed atomic write removes the temporary file', () => {
  const d = tmp()
  const file = join(d, 'target')
  // The target is a non-empty directory, so the final rename fails after the temporary file was written.
  mkdirSync(file)
  writeFileSync(join(file, 'keep'), 'x')
  assert.throws(() => writeFileAtomic(file, Buffer.from('data')))
  assert.equal(existsSync(`${file}.partial`), false)
})
