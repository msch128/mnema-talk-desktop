/**
 * A JSON reader that accepts what `serde_json` accepts for the documents the
 * host verifies (bundle manifest, trust statement, health, release feed) and
 * nothing laxer. `JSON.parse` differs in ways that matter for a signed
 * document: it takes the last of two equal keys (serde refuses a duplicate
 * field), and it cannot tell `1` from `1.0` or `1e0` (serde refuses those for
 * an integer field) or hold an integer above 2^53 exactly. A leading byte
 * order mark is refused too.
 *
 * Objects come back without a prototype. Integer tokens become a `number`, or a
 * `bigint` above 2^53; every other number token becomes a `JsonFloat`, so a
 * check such as `typeof value === 'number'` only passes for an integer.
 */

/** A number written with a fraction or an exponent: not an integer, even when its value is whole. */
export class JsonFloat {
  readonly text: string

  constructor(text: string) {
    this.text = text
  }
}

const MAX_DEPTH = 128
const INTEGER = /^-?(?:0|[1-9][0-9]*)$/
const NUMBER = /^-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?/
const WHITESPACE = new Set([' ', '\t', '\n', '\r'])
const ESCAPES: Record<string, string> = { '"': '"', '\\': '\\', '/': '/', b: '\b', f: '\f', n: '\n', r: '\r', t: '\t' }

class Reader {
  private at = 0
  private readonly text: string

  constructor(text: string) {
    this.text = text
  }

  document(): unknown {
    const value = this.value(0)
    this.space()
    if (this.at !== this.text.length) this.fail()
    return value
  }

  private fail(): never {
    throw new SyntaxError(`invalid JSON at ${this.at}`)
  }

  private space(): void {
    while (this.at < this.text.length && WHITESPACE.has(this.text[this.at] as string)) this.at++
  }

  private value(depth: number): unknown {
    if (depth > MAX_DEPTH) this.fail()
    this.space()
    const c = this.text[this.at]
    if (c === '{') return this.object(depth)
    if (c === '[') return this.array(depth)
    if (c === '"') return this.string()
    for (const [word, value] of [['true', true], ['false', false], ['null', null]] as const) {
      if (this.text.startsWith(word, this.at)) { this.at += word.length; return value }
    }
    return this.number()
  }

  private number(): number | bigint | JsonFloat {
    const token = NUMBER.exec(this.text.slice(this.at, this.at + 400))?.[0]
    if (token === undefined || token === '') this.fail()
    this.at += token.length
    if (!INTEGER.test(token)) return new JsonFloat(token)
    const n = Number(token)
    return Number.isSafeInteger(n) ? n : BigInt(token)
  }

  private string(): string {
    this.at++
    let out = ''
    for (;;) {
      const c = this.text[this.at++]
      if (c === undefined || c < ' ') this.fail()
      if (c === '"') return out
      if (c !== '\\') { out += c; continue }
      const e = this.text[this.at++]
      if (e === undefined) this.fail()
      if (e !== 'u') {
        const mapped = ESCAPES[e]
        if (mapped === undefined) this.fail()
        out += mapped
        continue
      }
      const unit = this.hex4()
      if (unit >= 0xd800 && unit <= 0xdbff) {
        if (this.text[this.at] !== '\\' || this.text[this.at + 1] !== 'u') this.fail()
        this.at += 2
        const low = this.hex4()
        if (low < 0xdc00 || low > 0xdfff) this.fail()
        out += String.fromCharCode(unit, low)
      } else if (unit >= 0xdc00 && unit <= 0xdfff) {
        this.fail()
      } else {
        out += String.fromCharCode(unit)
      }
    }
  }

  private hex4(): number {
    const digits = this.text.slice(this.at, this.at + 4)
    if (!/^[0-9a-fA-F]{4}$/.test(digits)) this.fail()
    this.at += 4
    return Number.parseInt(digits, 16)
  }

  private array(depth: number): unknown[] {
    this.at++
    const out: unknown[] = []
    this.space()
    if (this.text[this.at] === ']') { this.at++; return out }
    for (;;) {
      out.push(this.value(depth + 1))
      this.space()
      const c = this.text[this.at++]
      if (c === ']') return out
      if (c !== ',') this.fail()
    }
  }

  private object(depth: number): Record<string, unknown> {
    this.at++
    const out: Record<string, unknown> = Object.create(null) as Record<string, unknown>
    this.space()
    if (this.text[this.at] === '}') { this.at++; return out }
    for (;;) {
      this.space()
      if (this.text[this.at] !== '"') this.fail()
      const key = this.string()
      if (Object.hasOwn(out, key)) this.fail()
      this.space()
      if (this.text[this.at++] !== ':') this.fail()
      out[key] = this.value(depth + 1)
      this.space()
      const c = this.text[this.at++]
      if (c === '}') return out
      if (c !== ',') this.fail()
    }
  }
}

/** Parses UTF-8 JSON strictly (see the file comment); throws on anything else. */
export function parseStrictJson(bytes: Uint8Array): unknown {
  // `ignoreBOM` keeps a leading byte order mark in the text, where the reader refuses it.
  return new Reader(new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(bytes)).document()
}

/** An unsigned integer below 2^32 (`u32`), or null. */
export function asU32(value: unknown): number | null {
  return typeof value === 'number' && value >= 0 && value <= 0xffffffff ? value : null
}

/** An unsigned integer below 2^64 (`u64`), or null. Above 2^53 the result is approximate, which is enough to compare against a limit. */
export function asU64(value: unknown): number | null {
  if (typeof value === 'number') return value >= 0 ? value : null
  return typeof value === 'bigint' && value >= 0n && value < 1n << 64n ? Number(value) : null
}
