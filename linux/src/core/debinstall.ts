/**
 * Installing a verified `.deb` over the running one. The package manager owns
 * the files, so the client hands the package to it through polkit
 * (`pkexec apt-get install -y <file>`); it never writes under /opt itself.
 *
 * Only called with bytes whose minisign signature already verified (see
 * `selfupdate.ts`). Commands are run with an argument vector, never through a
 * shell, and the package sits in a private temporary directory under a fixed
 * name, so nothing user-controlled reaches a command line.
 */
import { execFile } from 'node:child_process'
import { existsSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const DEB_NAME = 'mnema-desktop-dev.deb'
const AR_MAGIC = '!<arch>\n'
const INSTALL_TIMEOUT_MS = 10 * 60 * 1000
/** pkexec: the user dismissed the dialog (126) or is not authorized (127 without a command). */
const PKEXEC_DECLINED = 126

/** Where electron-builder puts the `.deb` payload (`/opt/<productName>/`). */
export function isDebInstall(execPath: string, env: NodeJS.ProcessEnv, exists: (path: string) => boolean = existsSync): boolean {
  if (env.APPIMAGE) return false
  return execPath.startsWith('/opt/') && exists('/usr/bin/dpkg')
}

export interface Command { file: string, args: string[] }

/** The privileged install, as an argument vector. */
export function installCommand(path: string, tool: 'apt-get' | 'dpkg'): Command {
  return tool === 'apt-get'
    ? { file: 'pkexec', args: ['apt-get', 'install', '-y', path] }
    : { file: 'pkexec', args: ['dpkg', '-i', path] }
}

export interface DebHost {
  /** Runs a command without a shell; resolves with the exit code (null when it could not start). */
  run(command: Command): Promise<number | null>
  /** Whether pkexec can be found. */
  hasPkexec(): boolean
  /** Shows the file in the system's software installer. */
  open(path: string): Promise<void>
}

export type DebResult = 'installed' | 'handoff'

export function runCommand({ file, args }: Command): Promise<number | null> {
  return new Promise((resolve) => {
    execFile(file, args, { timeout: INSTALL_TIMEOUT_MS, windowsHide: true }, (error) => {
      if (!error) return resolve(0)
      const code = (error as { code?: unknown }).code
      resolve(typeof code === 'number' ? code : null)
    })
  })
}

export function findPkexec(exists: (path: string) => boolean = existsSync): boolean {
  return ['/usr/bin/pkexec', '/bin/pkexec'].some(exists)
}

/**
 * Writes the package to a private directory and installs it. With pkexec:
 * apt-get first (resolves dependencies); if that fails for another reason than
 * a dismissed dialog, dpkg -i. Without pkexec the file is opened for the
 * software center, which finishes the job (`handoff`).
 */
export async function installDeb(bytes: Uint8Array, host: DebHost): Promise<DebResult> {
  if (Buffer.from(bytes.subarray(0, AR_MAGIC.length)).toString('latin1') !== AR_MAGIC) throw new Error('not a deb package')
  const dir = mkdtempSync(join(tmpdir(), 'mnema-deb-'))
  const path = join(dir, DEB_NAME)
  try {
    writeFileSync(path, bytes, { mode: 0o600 })
    if (!host.hasPkexec()) {
      await host.open(path)
      return 'handoff'
    }
    const first = await host.run(installCommand(path, 'apt-get'))
    if (first === 0) return 'installed'
    if (first === PKEXEC_DECLINED) throw new Error('installation was not authorized')
    if ((await host.run(installCommand(path, 'dpkg'))) === 0) return 'installed'
    throw new Error('the package manager refused the package')
  } finally {
    // After a handoff the installer still reads the file; leave it for the system to clean.
    if (host.hasPkexec()) rmSync(dir, { recursive: true, force: true })
  }
}
