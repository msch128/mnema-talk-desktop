import assert from 'node:assert/strict'
import { existsSync, readFileSync, statSync } from 'node:fs'
import { dirname } from 'node:path'
import { test } from 'node:test'
import { type Command, type DebHost, installCommand, installDeb, isDebInstall } from '../src/core/debinstall.ts'

const DEB = Buffer.concat([Buffer.from('!<arch>\n'), Buffer.from('payload')])
const always = () => true

test('a .deb install is recognised by its path, never when started as an AppImage', () => {
  assert.ok(isDebInstall('/opt/Mnema Desktop DEV/mnema-desktop-dev', {}, always))
  assert.ok(!isDebInstall('/opt/Mnema Desktop DEV/mnema-desktop-dev', { APPIMAGE: '/x.AppImage' }, always))
  assert.ok(!isDebInstall('/home/u/app/mnema-desktop-dev', {}, always))
  assert.ok(!isDebInstall('/opt/x/mnema-desktop-dev', {}, () => false))
})

test('commands are argument vectors: the path is one argument, nothing goes through a shell', () => {
  const path = '/tmp/a b; rm -rf ~/x.deb'
  assert.deepEqual(installCommand(path, 'apt-get'), { file: 'pkexec', args: ['apt-get', 'install', '-y', path] })
  assert.deepEqual(installCommand(path, 'dpkg'), { file: 'pkexec', args: ['dpkg', '-i', path] })
})

function host(codes: (number | null)[], pkexec = true) {
  const seen: { command: Command, mode: number, bytes: Buffer }[] = []
  const opened: string[] = []
  const h: DebHost = {
    run: (command) => {
      const path = command.args.at(-1) ?? ''
      seen.push({ command, mode: statSync(dirname(path)).mode & 0o777, bytes: readFileSync(path) })
      return Promise.resolve(codes.shift() ?? 1)
    },
    hasPkexec: () => pkexec,
    open: (path) => { opened.push(path); return Promise.resolve() },
  }
  return { h, seen, opened }
}

test('apt-get installs from a private directory, which is removed afterwards', async () => {
  const { h, seen } = host([0])
  assert.equal(await installDeb(DEB, h), 'installed')
  assert.equal(seen.length, 1)
  assert.equal(seen[0]?.mode, 0o700)
  assert.deepEqual(seen[0]?.bytes, DEB)
  assert.equal(existsSync(dirname(seen[0]?.command.args.at(-1) ?? '')), false)
})

test('dpkg is the fallback; a dismissed dialog and refused packages fail', async () => {
  const fallback = host([100, 0])
  assert.equal(await installDeb(DEB, fallback.h), 'installed')
  assert.deepEqual(fallback.seen.map((s) => s.command.args[0]), ['apt-get', 'dpkg'])
  const dismissed = host([126])
  await assert.rejects(installDeb(DEB, dismissed.h), /not authorized/)
  assert.equal(dismissed.seen.length, 1)
  await assert.rejects(installDeb(DEB, host([100, 1]).h), /refused/)
})

test('without pkexec the file is opened for the software center; non-deb bytes are refused', async () => {
  const { h, seen, opened } = host([], false)
  assert.equal(await installDeb(DEB, h), 'handoff')
  assert.equal(seen.length, 0)
  assert.equal(opened.length, 1)
  await assert.rejects(installDeb(Buffer.from('MZ not a deb'), host([0]).h), /not a deb/)
})
