import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'

const read = (path: string) => readFileSync(new URL(path, import.meta.url), 'utf8')

test('the notices name the versions that ship, and the icon is the project icon', () => {
  const pkg = JSON.parse(read('../package.json')) as { dependencies: Record<string, string>, devDependencies: Record<string, string> }
  const notices = read('../THIRD_PARTY_NOTICES.md')
  assert.ok(notices.includes(`| ${pkg.devDependencies.electron} |`), 'Electron version')
  assert.ok(notices.includes(`| ${pkg.dependencies.ws} |`), 'ws version')
  assert.deepEqual(readFileSync(new URL('../build/icon.png', import.meta.url)), readFileSync(new URL('../../icons/icon.png', import.meta.url)))
})
