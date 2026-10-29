import assert from 'node:assert/strict'
import { existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, test } from 'node:test'
import { RendererRecovery } from '../src/core/recovery.ts'
import { closeWhenStarted } from '../src/core/downloads.ts'
import { createVault, passwordStoreSwitch, vaultAvailable, type SecretStorage } from '../src/core/osvault.ts'

const dirs: string[] = []
afterEach(() => { for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true }) })
const tmp = () => { const d = mkdtempSync(join(tmpdir(), 'mnema-vault-')); dirs.push(d); return d }

function storage(over: Partial<SecretStorage> = {}): SecretStorage {
  return {
    isEncryptionAvailable: () => true,
    getSelectedStorageBackend: () => 'gnome_libsecret',
    encryptString: (s) => Buffer.from(`enc:${s}`),
    decryptString: (b) => b.toString('utf8').replace(/^enc:/, ''),
    ...over,
  }
}
const ACCOUNT = '0123456789abcdef'

test('a secret service is a vault, the plaintext backend and a missing key are not', () => {
  assert.equal(vaultAvailable(storage(), {}, 'linux'), true)
  assert.equal(vaultAvailable(storage({ getSelectedStorageBackend: () => 'basic_text' }), {}, 'linux'), false)
  assert.equal(vaultAvailable(storage({ getSelectedStorageBackend: () => 'basic_text' }), {}, 'darwin'), true)
  assert.equal(vaultAvailable(storage({ isEncryptionAvailable: () => false }), {}, 'linux'), false)
  assert.equal(vaultAvailable(storage({ isEncryptionAvailable: () => { throw new Error('no') } }), {}, 'linux'), false)
  assert.equal(vaultAvailable(storage(), { MNEMA_DESKTOP_NO_VAULT: '1' }, 'linux'), false)
})

test('a window-manager session asks for the libsecret store, a known desktop or the user decide otherwise', () => {
  const none = () => false
  assert.equal(passwordStoreSwitch({ XDG_CURRENT_DESKTOP: 'sway' }, none), 'gnome-libsecret')
  assert.equal(passwordStoreSwitch({}, none), 'gnome-libsecret')
  assert.equal(passwordStoreSwitch({ XDG_CURRENT_DESKTOP: 'KDE' }, none), null)
  assert.equal(passwordStoreSwitch({ XDG_CURRENT_DESKTOP: 'ubuntu:GNOME' }, none), null)
  assert.equal(passwordStoreSwitch({ XDG_CURRENT_DESKTOP: 'sway' }, (n) => n === 'password-store'), null)
})

test('the vault round-trips a secret and ignores accounts that are not ids', () => {
  const dir = tmp()
  const vault = createVault(dir, storage())
  assert.equal(vault.save(ACCOUNT, Buffer.from('secret')), true)
  assert.equal(vault.load(ACCOUNT)?.toString(), 'secret')
  assert.equal(vault.save('../x', Buffer.from('s')), false)
  assert.equal(vault.load('../x'), null)
})

test('a failed save leaves no encrypted blob behind', () => {
  const dir = tmp()
  const vault = createVault(dir, storage())
  // A directory in the way makes the rename fail after the partial file was written.
  mkdirSync(join(dir, 'vault', `${ACCOUNT}.bin`), { recursive: true })
  writeFileSync(join(dir, 'vault', `${ACCOUNT}.bin`, 'keep'), 'x')
  assert.equal(vault.save(ACCOUNT, Buffer.from('secret')), false)
  assert.equal(existsSync(join(dir, 'vault', `${ACCOUNT}.bin.partial`)), false)
})

test('remove deletes the blob and a stale partial file, and never throws', () => {
  const dir = tmp()
  const vault = createVault(dir, storage())
  assert.equal(vault.save(ACCOUNT, Buffer.from('secret')), true)
  writeFileSync(join(dir, 'vault', `${ACCOUNT}.bin.partial`), 'stale')
  vault.remove(ACCOUNT)
  assert.deepEqual(readdirSync(join(dir, 'vault')), [])
  vault.remove(ACCOUNT)
  // A path that cannot be deleted (a non-empty directory under the name) does not abort the caller.
  mkdirSync(join(dir, 'vault', `${ACCOUNT}.bin`))
  writeFileSync(join(dir, 'vault', `${ACCOUNT}.bin`, 'f'), 'x')
  assert.doesNotThrow(() => vault.remove(ACCOUNT))
})

test('a renderer that ended is reloaded a few times, never after a clean exit', () => {
  const recovery = new RendererRecovery(2, 1000)
  assert.equal(recovery.allow('clean-exit', 0), false)
  assert.equal(recovery.allow('crashed', 0), true)
  assert.equal(recovery.allow('oom', 100), true)
  assert.equal(recovery.allow('crashed', 200), false)
  assert.equal(recovery.allow('crashed', 1100), true)
})

test('an attachment window closes once when its download is under way', () => {
  const handlers = new Map<string, () => void>()
  let closed = 0
  closeWhenStarted({ once: (event, fn) => { handlers.set(event, fn) } }, () => { closed++ })
  handlers.get('updated')?.()
  handlers.get('done')?.()
  assert.equal(closed, 1)
})

test('a blob that cannot be read is an error, only a missing one is absent', () => {
  const dir = tmp()
  const vault = createVault(dir, storage())
  assert.equal(vault.load(ACCOUNT), null)
  // A directory under the name makes the read fail with something other than ENOENT.
  mkdirSync(join(dir, 'vault', `${ACCOUNT}.bin`), { recursive: true })
  assert.throws(() => vault.load(ACCOUNT))
})
