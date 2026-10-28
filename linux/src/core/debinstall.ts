/**
 * Installing a verified `.deb` over the running one. The package manager owns
 * the files, so the client hands the package to it through polkit
 * (`pkexec`); it never writes under /opt itself.
 *
 * Only called with bytes whose minisign signature already verified (see
 * `selfupdate.ts`). The file the user-level process wrote can be swapped by
 * that same user while the polkit prompt is open, so root never installs it:
 * the privileged script copies it into a root-owned directory, checks the
 * SHA-256 of the verified bytes on that copy, and installs the copy only.
 * Commands are run with an argument vector; the script text is a constant and
 * the path and hash reach it as positional parameters, never as code.
 */
import { createHash } from 'node:crypto'
import { execFile } from 'node:child_process'
import { existsSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const DEB_NAME = 'mnema-desktop-dev.deb'
const AR_MAGIC = '!<arch>\n'
const INSTALL_TIMEOUT_MS = 10 * 60 * 1000
/** pkexec: the user dismissed the dialog (126) or authentication failed (127). */
const PKEXEC_DECLINED = [126, 127]
/** Exit statuses of the privileged script: the copy does not match the verified hash, is not the expected size, or the source is not a regular file. */
export const HASH_MISMATCH = 87
export const NOT_REGULAR = 88
export const WRONG_SIZE = 89

/**
 * Runs as root. $1: the package the client wrote, $2: SHA-256 of the verified
 * bytes, $3: their length. Only a regular, non-symlink file is read, through
 * one descriptor and at most $3 + 1 bytes, into a 0600 file in a 0700
 * root-owned directory; permissions are relaxed (for apt's sandbox user) only
 * after the hash matched. The user account cannot touch the copy at any point.
 */
export const PRIVILEGED_SCRIPT = [
  'set -eu',
  'umask 077',
  `{ [ -f "$1" ] && [ ! -L "$1" ]; } || exit ${NOT_REGULAR}`,
  'dir=$(mktemp -d /var/tmp/mnema-deb.XXXXXX)',
  'trap \'rm -rf -- "$dir"\' EXIT',
  'head -c "$(($3 + 1))" < "$1" > "$dir/pkg.deb"',
  `[ "$(wc -c < "$dir/pkg.deb")" -eq "$3" ] || exit ${WRONG_SIZE}`,
  `printf '%s  %s\\n' "$2" "$dir/pkg.deb" | sha256sum -c --status - || exit ${HASH_MISMATCH}`,
  'chmod 0755 "$dir"',
  'chmod 0644 "$dir/pkg.deb"',
  'apt-get install -y "$dir/pkg.deb" || dpkg -i "$dir/pkg.deb"',
].join('\n')

/** Where electron-builder puts the `.deb` payload (`/opt/<productName>/`). */
export function isDebInstall(execPath: string, env: NodeJS.ProcessEnv, exists: (path: string) => boolean = existsSync): boolean {
  if (env.APPIMAGE) return false
  return execPath.startsWith('/opt/') && exists('/usr/bin/dpkg')
}

export interface Command { file: string, args: string[] }

/** The privileged install, as an argument vector: the fixed script, then the path, the expected hash and the length. */
export function installCommand(path: string, sha256: string, length: number): Command {
  return { file: 'pkexec', args: ['/bin/sh', '-c', PRIVILEGED_SCRIPT, 'mnema-install', path, sha256, String(length)] }
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
 * Writes the package to a private directory and installs it through one
 * privileged script (apt-get, which resolves dependencies, then dpkg -i), so
 * the user sees a single prompt. Without pkexec the file is opened for the
 * software center, which finishes the job (`handoff`).
 */
export async function installDeb(bytes: Uint8Array, host: DebHost): Promise<DebResult> {
  if (Buffer.from(bytes.subarray(0, AR_MAGIC.length)).toString('latin1') !== AR_MAGIC) throw new Error('not a deb package')
  const sha256 = createHash('sha256').update(bytes).digest('hex')
  const dir = mkdtempSync(join(tmpdir(), 'mnema-deb-'))
  const path = join(dir, DEB_NAME)
  try {
    writeFileSync(path, bytes, { mode: 0o600 })
    if (!host.hasPkexec()) {
      await host.open(path)
      return 'handoff'
    }
    const code = await host.run(installCommand(path, sha256, bytes.length))
    if (code === 0) return 'installed'
    if (code === null) throw new Error('the package manager did not finish in time or could not be started')
    if (PKEXEC_DECLINED.includes(code)) throw new Error('installation was not authorized')
    if (code === HASH_MISMATCH || code === WRONG_SIZE || code === NOT_REGULAR) throw new Error('the package changed after it was verified; nothing was installed')
    throw new Error('the package manager refused the package')
  } finally {
    // After a handoff the installer still reads the file; leave it for the system to clean.
    if (host.hasPkexec()) rmSync(dir, { recursive: true, force: true })
  }
}
