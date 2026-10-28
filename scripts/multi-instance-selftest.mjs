// Runs the desktop host (built with the test-only `multi-selftest` feature)
// against two stand-in instances and checks what it recorded and what the
// instances saw. Isolated from the machine's own data: the host is started
// with a throw-away home directory.
//
//   node scripts/multi-instance-selftest.mjs <path to the host executable>
import { spawn } from 'node:child_process'
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { createServer } from 'node:net'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

const exe = process.argv[2]
if (!exe || !existsSync(exe)) throw new Error('usage: multi-instance-selftest.mjs <host executable>')
// Windows keeps its profile where the system says, whatever the environment: only a throw-away machine.
if (process.platform === 'win32' && !process.env.CI) throw new Error('this check uses the real profile on Windows; run it on a disposable machine (CI)')
const here = fileURLToPath(new URL('.', import.meta.url))
const dir = mkdtempSync(join(tmpdir(), 'mnema-multi-'))
const record = join(dir, 'record.txt')

const freePort = () => new Promise((resolve) => {
  const probe = createServer().listen(0, '127.0.0.1', () => { const { port } = probe.address(); probe.close(() => resolve(port)) })
})
const lines = (file) => (existsSync(file) ? readFileSync(file, 'utf8').split('\n').filter(Boolean) : [])
const entries = (file) => lines(file).map((l) => JSON.parse(l))
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

const children = []
const stop = () => { for (const child of children) child.kill() }
process.on('exit', stop)

const fails = []
const must = (good, what) => { if (!good) fails.push(what) }

try {
  const ports = [await freePort(), await freePort()]
  const logs = [join(dir, 'a.log'), join(dir, 'b.log')]
  ;['A', 'B'].forEach((name, i) => children.push(spawn(process.execPath, [join(here, 'fake-instance.mjs'), name, String(ports[i]), logs[i]], { stdio: 'inherit' })))
  const listening = () => logs.every((l) => entries(l).some((e) => e.kind === 'listening'))
  for (let i = 0; i < 50 && !listening(); i++) await sleep(100)
  if (!listening()) throw new Error('a stand-in instance did not start listening')

  const home = join(dir, 'home')
  // Elsewhere the data directory follows the home directory; on Windows it is
  // where the system says (this check only runs on a disposable runner there).
  const isolated = process.platform === 'win32' ? {} : { HOME: home, XDG_DATA_HOME: join(home, 'share') }
  const env = {
    ...process.env,
    ...isolated,
    MNEMA_DESKTOP_NO_UPDATE_CHECK: '1',
    MNEMA_MULTI_SELFTEST: record,
    MNEMA_MULTI_SELFTEST_SERVERS: ports.map((p) => `http://127.0.0.1:${p}`).join(','),
  }
  const host = spawn(exe, [], { env, stdio: 'inherit' })
  children.push(host)
  const exited = new Promise((resolve) => host.on('exit', resolve))
  const deadline = Date.now() + 180_000
  while (!lines(record).includes('done') && Date.now() < deadline) {
    if (await Promise.race([exited.then(() => true), sleep(500).then(() => false)])) break
  }

  const result = lines(record)
  console.log(result.join('\n'))
  must(result.includes('done'), 'the host did not finish')
  for (const line of result.filter((l) => l.startsWith('fail'))) fails.push(line)
  const passed = result.filter((l) => l.startsWith('ok ')).length
  const total = Number(result.find((l) => l.startsWith('total '))?.slice(6))
  must(total > 0 && passed === total, `${passed} of the host's ${total} checks passed`)

  const [a, b] = logs.map(entries)
  const origins = ports.map((p) => `http://127.0.0.1:${p}`)
  const upgrade = a.find((e) => e.kind === 'upgrade')
  must(upgrade?.url === '/api/ws?dm=1', `A's socket asked for ${upgrade?.url}`)
  must(upgrade?.cookie === 'session=A-token' && upgrade?.origin === origins[0], `A's socket carried ${upgrade?.cookie} from ${upgrade?.origin}`)
  must(!b.some((e) => e.kind === 'upgrade'), 'B was sent a socket it never asked for')
  must(!b.some((e) => String(e.cookie).includes('A-token')), 'B was sent A\'s cookie')
  must(!a.some((e) => String(e.cookie).includes('B-token')), 'A was sent B\'s cookie')
  ;[a, b].forEach((seen, i) => must(seen.every((e) => e.kind !== 'http' || e.origin === null || e.origin === origins[i]), `${'AB'[i]} saw an origin that is not its own`))
} finally {
  stop()
  await sleep(500)
  rmSync(dir, { recursive: true, force: true })
}
if (fails.length) { console.error(`FAILED:\n${fails.join('\n')}`); process.exit(1) }
console.log('multi-instance self-test passed')
