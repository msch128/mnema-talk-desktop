// Runs the desktop host (built with the test-only `real-selftest` feature)
// against a real mnema-talk server: a binary built from a release tag, with
// the PostgreSQL and S3 storage the caller started. The server listens on a
// port of its own and is entered the way a person types a local server,
// `http://localhost:<port>`. The host adds it, loads a verified web bundle for
// it, signs in, opens the WebSocket, sends a message and sees it arrive; the
// server is then stopped and started again on the same port and database, and
// the host must carry on. Isolated from the machine's own data: the host is
// started with a throw-away home directory (on Windows only on a disposable runner).
//
//   DATABASE_URL=... WEED_EXE=... node scripts/real-server-selftest.mjs <host executable> <server executable>
//
// PostgreSQL is the caller's (DATABASE_URL). Object storage is a SeaweedFS
// started here from WEED_EXE, or the caller's own S3 endpoint (S3_ENDPOINT).
//
// A client that is driven from outside (the Electron client, by a test) runs as
//
//   ... node scripts/real-server-selftest.mjs --run <server executable> <command> [arguments]
//
// with the same environment and the same restart protocol; its exit status is the verdict.
//
// The host appends `ok`/`fail` lines to a record (src/selftest_real.rs); it asks
// for the restart with a `restart` line, and this script answers in a control file.
import { spawn } from 'node:child_process'
import { appendFileSync, closeSync, existsSync, mkdirSync, mkdtempSync, openSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { createServer } from 'node:net'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { randomBytes } from 'node:crypto'

const args = process.argv.slice(2)
const driven = args[0] === '--run'
const [hostExe, serverExe, ...driverArgs] = driven ? [args[2], args[1], ...args.slice(3)] : args
const usage = 'usage: real-server-selftest.mjs <host executable> <server executable> | --run <server executable> <command> [arguments]'
if (!hostExe || !serverExe || !existsSync(serverExe) || (!driven && !existsSync(hostExe))) throw new Error(usage)
if (process.platform === 'win32' && !process.env.CI) throw new Error('this check uses the real profile on Windows; run it on a disposable machine (CI)')
if (!process.env.DATABASE_URL) throw new Error('DATABASE_URL is required')
if (!process.env.S3_ENDPOINT && !(process.env.WEED_EXE && existsSync(process.env.WEED_EXE))) throw new Error('S3_ENDPOINT or WEED_EXE is required')

const port = Number(process.env.REAL_SERVER_PORT ?? 18443)
const address = `http://localhost:${port}`
const admin = { user: 'selftest_admin', password: randomBytes(18).toString('base64url') }
const jwtSecret = randomBytes(32).toString('hex')

const dir = mkdtempSync(join(tmpdir(), 'mnema-real-'))
const record = join(dir, 'record.txt')
const control = join(dir, 'control.txt')
writeFileSync(control, '')
const lines = (file) => (existsSync(file) ? readFileSync(file, 'utf8').split('\n').map((l) => l.trim()).filter(Boolean) : [])
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))
// SeaweedFS adds 10000 to each port for gRPC, so its ports stay low enough for that and both must be free.
const listens = (port) => new Promise((resolve) => {
  const probe = createServer()
  probe.once('error', () => resolve(false))
  probe.listen(port, '127.0.0.1', () => probe.close(() => resolve(true)))
})
const taken = new Set()
const freePort = async () => {
  for (let i = 0; i < 100; i++) {
    const port = 20000 + Math.floor(Math.random() * 25000)
    if (taken.has(port) || !(await listens(port)) || !(await listens(port + 10000))) continue
    taken.add(port)
    return port
  }
  throw new Error('no free port')
}

// The server's S3 storage. It is lazy: until the bucket answers, /api/health says 503.
let weed = null
let s3Endpoint = process.env.S3_ENDPOINT
const startStorage = async () => {
  if (s3Endpoint) return
  const [s3, master, volume, filer] = [await freePort(), await freePort(), await freePort(), await freePort()]
  const data = join(dir, 's3')
  mkdirSync(data)
  weed = spawn(process.env.WEED_EXE, ['server', '-ip=127.0.0.1', `-dir=${data}`, '-s3', `-s3.port=${s3}`, `-master.port=${master}`, `-volume.port=${volume}`, `-filer.port=${filer}`], { stdio: 'ignore', windowsHide: true })
  weed.on('error', (error) => console.error(`weed: ${error}`))
  s3Endpoint = `http://127.0.0.1:${s3}`
  for (let i = 0; i < 120; i++) {
    if (weed.exitCode !== null) throw new Error(`SeaweedFS exited with ${weed.exitCode}`)
    try { if ((await fetch(`${s3Endpoint}/`)).ok) return } catch { /* not listening yet */ }
    await sleep(500)
  }
  throw new Error('SeaweedFS did not start')
}

// The server keeps its database, secret and port across the restart: the session of the host must stay valid.
const serverEnv = {
  PATH: process.env.PATH,
  SystemRoot: process.env.SystemRoot,
  TEMP: process.env.TEMP,
  TMP: process.env.TMP,
  HOME: dir,
  APP_ENV: 'development',
  PORT: String(port),
  BIND_ADDR: '127.0.0.1',
  PUBLIC_URL: address,
  JWT_SECRET: jwtSecret,
  ADMIN_USERNAME: admin.user,
  ADMIN_INITIAL_PASSWORD: admin.password,
  DATABASE_URL: process.env.DATABASE_URL,
  S3_ENDPOINT: '',
  S3_BUCKET: 'mnema-selftest',
  S3_ACCESS_KEY: 'selftest',
  S3_SECRET_KEY: 'selftest-secret',
  S3_FORCE_PATH_STYLE: 'true',
  LINK_PREVIEWS_ENABLED: 'false',
  UPDATE_CHECK_ENABLED: 'false',
  WEBRTC_BUILTIN_TURN: 'off',
}

const serverLog = join(dir, 'server.log')
let server = null
const startServer = () => {
  serverEnv.S3_ENDPOINT = s3Endpoint
  const out = openSync(serverLog, 'a')
  const child = spawn(serverExe, [], { env: serverEnv, cwd: dir, stdio: ['ignore', out, out], windowsHide: true })
  closeSync(out)
  child.on('error', (error) => console.error(`server: ${error}`))
  server = child
  return new Promise((resolve) => child.once('spawn', resolve))
}
const stopServer = async () => {
  if (!server || server.exitCode !== null) return
  const gone = new Promise((resolve) => server.once('exit', resolve))
  server.kill()
  await gone
}
const healthy = async () => {
  try { return (await fetch(`${address}/api/health`)).status === 200 } catch { return false }
}
const waitHealthy = async (what) => {
  for (let i = 0; i < 240; i++) {
    if (await healthy()) return
    if (server.exitCode !== null) throw new Error(`${what}: the server exited with ${server.exitCode}`)
    await sleep(500)
  }
  throw new Error(`${what}: the server did not become healthy`)
}

let host = null
const stop = () => { for (const child of [host, server, weed]) if (child && child.exitCode === null) child.kill() }
process.on('exit', stop)

let serverTail = ''
const fails = []
const must = (good, what) => { if (!good) fails.push(what) }

try {
  await startStorage()
  await startServer()
  await waitHealthy('first start')
  const version = (await (await fetch(`${address}/api/health`)).json()).version
  console.log(`server ${version} on ${address}`)

  const home = join(dir, 'home')
  const isolated = driven || process.platform === 'win32' ? {} : { HOME: home, XDG_DATA_HOME: join(home, 'share') }
  const env = {
    ...process.env,
    ...isolated,
    MNEMA_DESKTOP_NO_UPDATE_CHECK: '1',
    MNEMA_REAL_SELFTEST: record,
    MNEMA_REAL_SELFTEST_SERVER: address,
    MNEMA_REAL_SELFTEST_USER: admin.user,
    MNEMA_REAL_SELFTEST_PASSWORD: admin.password,
    MNEMA_REAL_SELFTEST_CONTROL: control,
  }
  host = spawn(hostExe, driverArgs, { env, stdio: 'inherit' })
  const exited = new Promise((resolve) => host.on('exit', resolve))
  const deadline = Date.now() + 420_000
  let restarted = false
  let answered = false
  while (!(!driven && lines(record).includes('done')) && Date.now() < deadline) {
    const seen = lines(record)
    if (seen.includes('restart') && !restarted) {
      restarted = true
      await stopServer()
      appendFileSync(control, 'stopped\n')
    }
    if (seen.includes('down-checked') && !answered) {
      answered = true
      await startServer()
      await waitHealthy('restart')
      appendFileSync(control, 'started\n')
    }
    if (await Promise.race([exited.then(() => true), sleep(300).then(() => false)])) break
  }

  const result = lines(record)
  console.log(result.join('\n'))
  must(restarted && answered, 'the host never asked for the restart')
  if (driven) {
    await Promise.race([exited, sleep(60_000)])
    must(host.exitCode === 0, `the command exited with ${host.exitCode}`)
  } else {
    must(result.includes('done'), 'the host did not finish')
    for (const line of result.filter((l) => l.startsWith('fail'))) fails.push(line)
    const passed = result.filter((l) => l.startsWith('ok ')).length
    const total = Number(result.find((l) => l.startsWith('total '))?.slice(6))
    must(total > 0 && passed === total, `${passed} of the host's ${total} checks passed`)
  }
} catch (error) {
  fails.push(String(error?.stack ?? error))
} finally {
  serverTail = lines(serverLog).slice(-40).join('\n')
  stop()
  await sleep(500)
  rmSync(dir, { recursive: true, force: true })
}
if (fails.length) {
  console.error(`FAILED:\n${fails.join('\n')}`)
  console.error(`server log (end):\n${serverTail}`)
  process.exit(1)
}
console.log('real-server self-test passed')
