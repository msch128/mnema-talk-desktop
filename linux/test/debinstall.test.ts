import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { existsSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { test } from 'node:test'
import { type Command, type DebHost, HASH_MISMATCH, PRIVILEGED_SCRIPT, installCommand, installDeb, isDebInstall } from '../src/core/debinstall.ts'

const DEB = Buffer.concat([Buffer.from('!<arch>\n'), Buffer.from('payload')])
const always = () => true

test('a .deb install is recognised by its path, never when started as an AppImage', () => {
  assert.ok(isDebInstall('/opt/Mnema Desktop DEV/mnema-desktop-dev', {}, always))
  assert.ok(!isDebInstall('/opt/Mnema Desktop DEV/mnema-desktop-dev', { APPIMAGE: '/x.AppImage' }, always))
  assert.ok(!isDebInstall('/home/u/app/mnema-desktop-dev', {}, always))
  assert.ok(!isDebInstall('/opt/x/mnema-desktop-dev', {}, () => false))
})

test('the privileged step is the fixed script with the path and the verified hash as parameters', () => {
  const path = '/tmp/a b; rm -rf ~/x.deb'
  const sha = createHash('sha256').update(DEB).digest('hex')
  assert.deepEqual(installCommand(path, sha), { file: 'pkexec', args: ['/bin/sh', '-c', PRIVILEGED_SCRIPT, 'mnema-install', path, sha] })
  assert.ok(!PRIVILEGED_SCRIPT.includes(path))
})

function host(codes: (number | null)[], pkexec = true) {
  const seen: { command: Command, mode: number, bytes: Buffer }[] = []
  const opened: string[] = []
  const h: DebHost = {
    run: (command) => {
      const path = command.args.at(-2) ?? ''
      seen.push({ command, mode: statSync(dirname(path)).mode & 0o777, bytes: readFileSync(path) })
      return Promise.resolve(codes.shift() ?? 1)
    },
    hasPkexec: () => pkexec,
    open: (path) => { opened.push(path); return Promise.resolve() },
  }
  return { h, seen, opened }
}

test('the package is written to a private directory, handed over with the hash of the verified bytes, and removed afterwards', async () => {
  const { h, seen } = host([0])
  assert.equal(await installDeb(DEB, h), 'installed')
  assert.equal(seen.length, 1)
  assert.equal(seen[0]?.mode, 0o700)
  assert.deepEqual(seen[0]?.bytes, DEB)
  assert.equal(seen[0]?.command.args.at(-1), createHash('sha256').update(DEB).digest('hex'))
  assert.equal(existsSync(dirname(seen[0]?.command.args.at(-2) ?? '')), false)
})

test('a dismissed dialog, a changed package and a refused package each fail once, with no second attempt', async () => {
  const dismissed = host([126])
  await assert.rejects(installDeb(DEB, dismissed.h), /not authorized/)
  const changed = host([HASH_MISMATCH])
  await assert.rejects(installDeb(DEB, changed.h), /changed after it was verified/)
  assert.equal(changed.seen.length, 1)
  await assert.rejects(installDeb(DEB, host([100]).h), /refused/)
})

// Runs the real script as the current user, with stand-ins for the package tools.
function runScript(path: string, sha256: string) {
  const bin = mkdtempSync(join(tmpdir(), 'mnema-bin-'))
  const log = join(bin, 'log')
  for (const tool of ['apt-get', 'dpkg']) {
    writeFileSync(join(bin, tool), `#!/bin/sh\necho "${tool} $* $(cat "$3" 2>/dev/null | head -c 64)" >> "${log}"\n[ "${tool}" = dpkg ] || exit 100\n`, { mode: 0o755 })
  }
  const r = spawnSync('/bin/sh', ['-c', PRIVILEGED_SCRIPT, 'mnema-install', path, sha256], { env: { ...process.env, PATH: `${bin}:${process.env.PATH}` } })
  const calls = existsSync(log) ? readFileSync(log, 'utf8') : ''
  rmSync(bin, { recursive: true, force: true })
  return { status: r.status, calls }
}

test('the script installs only a copy that matches the hash, and refuses a swapped file', () => {
  const dir = mkdtempSync(join(tmpdir(), 'mnema-script-'))
  const path = join(dir, 'p.deb')
  try {
    writeFileSync(path, DEB)
    const sha = createHash('sha256').update(DEB).digest('hex')
    const ok = runScript(path, sha)
    assert.equal(ok.status, 0)
    assert.match(ok.calls, /apt-get install -y \/var\/tmp\/mnema-deb\.[^/]+\/pkg\.deb/)
    assert.match(ok.calls, /dpkg -i /, 'falls back to dpkg on the same verified copy')
    assert.ok(!ok.calls.includes(path), 'the user-writable path is never handed to the package tools')
    writeFileSync(path, Buffer.concat([Buffer.from('!<arch>\n'), Buffer.from('evil')]))
    const swapped = runScript(path, sha)
    assert.equal(swapped.status, HASH_MISMATCH)
    assert.equal(swapped.calls, '')
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test('without pkexec the file is opened for the software center; non-deb bytes are refused', async () => {
  const { h, seen, opened } = host([], false)
  assert.equal(await installDeb(DEB, h), 'handoff')
  assert.equal(seen.length, 0)
  assert.equal(opened.length, 1)
  await assert.rejects(installDeb(Buffer.from('MZ not a deb'), host([0]).h), /not a deb/)
})
