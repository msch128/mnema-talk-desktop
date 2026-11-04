import { bundle, entries, entry, sha256, signWith, spec, testKey, zipEntries, zipOf, type Entry, type ZipItem } from '../../test/helpers/fixtures.ts'
import { keysOf, outcome, type Json } from './common.ts'

/** Offsets of the central directory records of a zip built by `zipOf`, by entry name. */
export function central(zip: Buffer): Map<string, number> {
  const eocd = zip.length - 22
  const out = new Map<string, number>()
  let at = zip.readUInt32LE(eocd + 16)
  for (let i = 0; i < zip.readUInt16LE(eocd + 10); i++) {
    const length = zip.readUInt16LE(at + 28)
    out.set(zip.subarray(at + 46, at + 46 + length).toString(), at)
    at += 46 + length + zip.readUInt16LE(at + 30) + zip.readUInt16LE(at + 32)
  }
  return out
}

/** A copy of a zip built by `zipOf` with its records changed in place; `find` gives the offsets of an entry. */
export function patched(zip: Buffer, change: (copy: Buffer, find: (name: string) => { central: number, local: number }) => void): Buffer {
  const copy = Buffer.from(zip)
  const at = central(zip)
  change(copy, (name) => {
    const record = at.get(name) as number
    return { central: record, local: zip.readUInt32LE(record + 42) }
  })
  return copy
}

/** A bundle whose entries are changed and whose manifest is signed again to list them. */
export function resignedEntries(change: (list: Entry[], manifest: { files: { path: string, sha256: string, size: number }[] }) => void): Buffer {
  const list = entries(spec('0.8.0'))
  const manifest = JSON.parse(entry(list, 'manifest.json').body.toString()) as { files: { path: string, sha256: string, size: number }[] }
  change(list, manifest)
  const raw = Buffer.from(JSON.stringify(manifest))
  entry(list, 'manifest.json').body = raw
  entry(list, 'manifest.sig').body = Buffer.from(signWith(testKey(7), raw).toString('base64'))
  return zipEntries(list)
}

/** The zip with a zip64 end record and locator written in front of its end record. */
function withZip64Records(good: Buffer): Buffer {
  const eocd = good.subarray(good.length - 22)
  const record = Buffer.alloc(56)
  record.writeUInt32LE(0x06064b50, 0)
  record.writeBigUInt64LE(44n, 4)
  record.writeUInt16LE(45, 12)
  record.writeUInt16LE(45, 14)
  record.writeBigUInt64LE(BigInt(eocd.readUInt16LE(10)), 24)
  record.writeBigUInt64LE(BigInt(eocd.readUInt16LE(10)), 32)
  record.writeBigUInt64LE(BigInt(eocd.readUInt32LE(12)), 40)
  record.writeBigUInt64LE(BigInt(eocd.readUInt32LE(16)), 48)
  const locator = Buffer.alloc(20)
  locator.writeUInt32LE(0x07064b50, 0)
  locator.writeBigUInt64LE(BigInt(good.length - 22), 8)
  locator.writeUInt32LE(1, 16)
  return Buffer.concat([good.subarray(0, good.length - 22), record, locator, eocd])
}

/** The zip with bytes in front of the archive and every offset shifted to match. */
function shifted(good: Buffer): Buffer {
  const junk = Buffer.from('JUNKJUNK')
  const z = Buffer.concat([junk, good])
  const end = z.length - 22
  let at = z.readUInt32LE(end + 16) + junk.length
  for (let i = 0; i < z.readUInt16LE(end + 10); i++) {
    z.writeUInt32LE(z.readUInt32LE(at + 42) + junk.length, at + 42)
    at += 46 + z.readUInt16LE(at + 28) + z.readUInt16LE(at + 30) + z.readUInt16LE(at + 32)
  }
  z.writeUInt32LE(z.readUInt32LE(end + 16) + junk.length, end + 16)
  return z
}

/**
 * Hostile zip shapes around a bundle that is otherwise correctly signed. Both
 * clients must refuse every one of them with the same message; the first
 * bundle is the control and must open.
 */
export function zipVectors(): Json {
  const base = spec('0.8.0')
  const good = bundle(base)
  const index = 'app/index.html'
  const items = (extra: (e: Entry) => Partial<ZipItem>) => zipOf(entries(base).map((e) => ({ name: e.name, body: e.body, ...extra(e) })))
  const mode = (kind: number, madeBy = 0x0314) => patched(good, (z, f) => { const e = f(index).central; z.writeUInt16LE(madeBy, e + 4); z.writeUInt32LE(kind * 0x10000, e + 38) })
  const flag = (bit: number) => patched(good, (z, f) => { const e = f(index); z.writeUInt16LE(z.readUInt16LE(e.central + 8) | bit, e.central + 8); z.writeUInt16LE(z.readUInt16LE(e.local + 6) | bit, e.local + 6) })
  const cases: { name: string, zip: Buffer }[] = [
    { name: 'a plain stored bundle', zip: good },
    { name: 'an entry that announces a data descriptor', zip: flag(8) },
    { name: 'a zip comment', zip: (() => { const z = Buffer.concat([good, Buffer.from('note')]); z.writeUInt16LE(4, good.length - 2); return z })() },
    { name: 'an entry marked encrypted', zip: flag(1) },
    { name: 'an entry with strong encryption', zip: flag(0x40) },
    { name: 'an entry with a compression method of its own', zip: patched(good, (z, f) => { const e = f(index); z.writeUInt16LE(99, e.central + 10); z.writeUInt16LE(99, e.local + 8) }) },
    { name: 'a zip64 end record with 65535 entries', zip: patched(good, (z) => { z.writeUInt16LE(0xffff, z.length - 14); z.writeUInt16LE(0xffff, z.length - 12) }) },
    { name: 'a zip64 directory offset', zip: patched(good, (z) => { z.writeUInt32LE(0xffffffff, z.length - 6) }) },
    { name: 'a zip64 entry size', zip: patched(good, (z, f) => { z.writeUInt32LE(0xffffffff, f(index).central + 24) }) },
    { name: 'a zip64 compressed size', zip: patched(good, (z, f) => { z.writeUInt32LE(0xffffffff, f(index).central + 20) }) },
    { name: 'a zip64 header offset', zip: patched(good, (z, f) => { z.writeUInt32LE(0xffffffff, f(index).central + 42) }) },
    { name: 'a zip64 end record and locator in front of the end record', zip: withZip64Records(good) },
    { name: 'an entry that declares a size smaller than it holds', zip: items((e) => ({ declaredSize: e.name === index ? e.body.length - 1 : undefined })) },
    { name: 'an entry that declares a size larger than it holds', zip: items((e) => ({ declaredSize: e.name === index ? e.body.length + 1 : undefined })) },
    { name: 'an entry that declares a huge size', zip: items((e) => ({ declaredSize: e.name === index ? 0x7fffffff : undefined })) },
    { name: 'a manifest that declares a size smaller than it holds', zip: items((e) => ({ declaredSize: e.name === 'manifest.json' ? 10 : undefined })) },
    { name: 'a deflated entry that declares a size smaller than it inflates to', zip: items((e) => ({ method: 8 as const, declaredSize: e.name === index ? 3 : undefined })) },
    { name: 'an entry with a wrong checksum', zip: items((e) => ({ crc: e.name === index ? 1 : undefined })) },
    { name: 'a local header that is not one', zip: patched(good, (z, f) => { z.writeUInt32LE(0, f(index).local) }) },
    { name: 'bytes in front of the archive', zip: Buffer.concat([Buffer.from('MZ junk before the zip'), good]) },
    { name: 'bytes in front of the archive, offsets shifted to match', zip: shifted(good) },
    { name: 'bytes after the end record', zip: Buffer.concat([good, Buffer.from('trailing')]) },
    { name: 'the same name twice', zip: zipEntries([...entries(base), { name: index, body: Buffer.from('<html>app</html>') }]) },
    { name: 'the manifest twice', zip: zipEntries([...entries(base), entries(base)[0] as Entry]) },
    { name: 'a name that differs only in case', zip: zipEntries([...entries(base), { name: 'app/INDEX.html', body: Buffer.from('<html>app</html>') }]) },
    { name: 'the manifest under another case', zip: zipEntries(entries(base).map((e) => (e.name === 'manifest.json' ? { ...e, name: 'Manifest.json' } : e))) },
    {
      name: 'a signed manifest that lists two paths that differ only in case',
      zip: resignedEntries((list, m) => {
        list.push({ name: 'app/INDEX.html', body: Buffer.from('<html>app</html>') })
        m.files.push({ path: 'INDEX.html', sha256: sha256('<html>app</html>'), size: 16 })
      }),
    },
    { name: 'an entry that climbs out of the folder', zip: zipEntries([...entries(base), { name: 'app/../evil.js', body: Buffer.from('x') }]) },
    { name: 'an entry that starts with a slash', zip: zipEntries(entries(base).map((e) => (e.name === index ? { ...e, name: '/app/index.html' } : e))) },
    { name: 'an entry with a backslash', zip: zipEntries(entries(base).map((e) => (e.name === index ? { ...e, name: 'app\\index.html' } : e))) },
    { name: 'an entry with a drive letter', zip: zipEntries([...entries(base), { name: 'C:/evil.js', body: Buffer.from('x') }]) },
    {
      name: 'a signed manifest whose path climbs out of the folder',
      zip: resignedEntries((list, m) => {
        list.push({ name: 'app/../evil.js', body: Buffer.from('x') })
        m.files.push({ path: '../evil.js', sha256: sha256('x'), size: 1 })
      }),
    },
    { name: 'an entry that is a symbolic link', zip: mode(0o120777) },
    { name: 'an entry that is a directory', zip: mode(0o040755) },
    { name: 'an entry that is a device', zip: mode(0o060644) },
    { name: 'an entry with a regular file mode', zip: mode(0o100644) },
    { name: 'a link mode written by a system that does not use modes', zip: mode(0o120777, 0x0014) },
    { name: 'an entry named by a name that is not UTF-8', zip: Buffer.from(good.toString('latin1').replaceAll('app/index.html', 'app/ind\xe9x.html'), 'latin1') },
  ]
  return { bundles: cases.map(({ name, zip }) => outcome(name, keysOf(7), zip)) }
}

