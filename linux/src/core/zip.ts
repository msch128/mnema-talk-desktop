import { crc32, inflateRawSync } from 'node:zlib'

/**
 * A small, strict ZIP reader for signed bundles. It reads the central
 * directory only, supports stored and deflated entries, refuses ZIP64 and
 * encryption, and counts bytes while inflating so that a lying header or a zip
 * bomb stops at the caller's limit.
 */
export class ZipError extends Error {}

export interface ZipEntry {
  readonly name: string
  /** False when the name is not UTF-8. */
  readonly nameValid: boolean
  readonly method: number
  readonly flags: number
  readonly crc: number
  readonly compressedSize: number
  /** Uncompressed size as the central directory declares it. */
  readonly size: number
  readonly offset: number
}

const EOCD = 0x06054b50
const CENTRAL = 0x02014b50
const LOCAL = 0x04034b50

export class ZipArchive {
  readonly entries: ZipEntry[]
  private readonly data: Uint8Array
  private readonly view: DataView

  constructor(data: Uint8Array) {
    this.data = data
    this.view = new DataView(data.buffer, data.byteOffset, data.byteLength)
    this.entries = this.readDirectory()
  }

  get length(): number {
    return this.entries.length
  }

  private readDirectory(): ZipEntry[] {
    const bad = () => new ZipError('not a readable zip')
    const { view, data } = this
    if (data.length < 22) throw bad()
    let eocd = -1
    const lowest = Math.max(0, data.length - 22 - 0xffff)
    for (let at = data.length - 22; at >= lowest; at--) {
      if (view.getUint32(at, true) === EOCD) { eocd = at; break }
    }
    if (eocd < 0) throw bad()
    const disk = view.getUint16(eocd + 4, true)
    const cdDisk = view.getUint16(eocd + 6, true)
    const onDisk = view.getUint16(eocd + 8, true)
    const total = view.getUint16(eocd + 10, true)
    const cdSize = view.getUint32(eocd + 12, true)
    const cdOffset = view.getUint32(eocd + 16, true)
    // ZIP64 and multi-disk archives are not bundles.
    if (disk !== 0 || cdDisk !== 0 || onDisk !== total || total === 0xffff || cdSize === 0xffffffff || cdOffset === 0xffffffff) throw bad()
    if (cdOffset + cdSize > eocd) throw bad()
    const entries: ZipEntry[] = []
    let at = cdOffset
    for (let i = 0; i < total; i++) {
      if (at + 46 > cdOffset + cdSize || view.getUint32(at, true) !== CENTRAL) throw bad()
      const flags = view.getUint16(at + 8, true)
      const method = view.getUint16(at + 10, true)
      const crc = view.getUint32(at + 16, true)
      const compressedSize = view.getUint32(at + 20, true)
      const size = view.getUint32(at + 24, true)
      const nameLength = view.getUint16(at + 28, true)
      const extraLength = view.getUint16(at + 30, true)
      const commentLength = view.getUint16(at + 32, true)
      const offset = view.getUint32(at + 42, true)
      const end = at + 46 + nameLength + extraLength + commentLength
      if (end > cdOffset + cdSize || compressedSize === 0xffffffff || size === 0xffffffff || offset === 0xffffffff) throw bad()
      const decoded = this.decodeName(at + 46, nameLength)
      entries.push({ name: decoded ?? '', nameValid: decoded !== null, method, flags, crc, compressedSize, size, offset })
      at = end
    }
    return entries
  }

  /** Names are UTF-8. A name that is not stays unreadable instead of being guessed. */
  private decodeName(from: number, length: number): string | null {
    try {
      return new TextDecoder('utf-8', { fatal: true }).decode(this.data.subarray(from, from + length))
    } catch {
      return null
    }
  }


  /** The compressed bytes of an entry, or null if the local header is wrong. */
  private raw(entry: ZipEntry): Uint8Array | null {
    const { view, data } = this
    const at = entry.offset
    if (at + 30 > data.length || view.getUint32(at, true) !== LOCAL) return null
    const start = at + 30 + view.getUint16(at + 26, true) + view.getUint16(at + 28, true)
    const end = start + entry.compressedSize
    if (end > data.length) return null
    return data.subarray(start, end)
  }

  /**
   * Reads one entry. `limit` bounds the uncompressed size; the same messages as
   * the Rust host so that the shared vectors compare them.
   */
  read(entry: ZipEntry, limit: number): Uint8Array {
    const { name } = entry
    if ((entry.flags & 1) !== 0 || (entry.method !== 0 && entry.method !== 8)) throw new ZipError('unreadable entry')
    if (!entry.nameValid) throw new ZipError('invalid entry name')
    if (entry.size > limit) throw new ZipError(`${name}: too large`)
    const packed = this.raw(entry)
    if (!packed) throw new ZipError('unreadable entry')
    let out: Uint8Array
    if (entry.method === 0) {
      if (packed.length > limit) throw new ZipError(`${name}: too large`)
      out = packed
    } else {
      try {
        // One byte more than allowed tells a lying header from an exact size.
        out = inflateRawSync(packed, { maxOutputLength: Math.max(1, limit + 1) })
      } catch (error) {
        if (error instanceof RangeError || (error as { code?: string }).code === 'ERR_BUFFER_TOO_LARGE') throw new ZipError(`${name}: too large`)
        throw new ZipError(`${name}: damaged`)
      }
      if (out.length > limit) throw new ZipError(`${name}: too large`)
    }
    if (out.length !== entry.size || crc32(out) !== entry.crc) throw new ZipError(`${name}: damaged`)
    return out
  }
}
