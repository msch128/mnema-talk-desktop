/** `MAJOR.MINOR.PATCH`, nothing else (the Rust host's `Version`). */
export type Version = readonly [number, number, number]

export const ZERO: Version = [0, 0, 0]

export function parseVersion(text: string): Version | null {
  const parts = text.split('.')
  if (parts.length !== 3) return null
  const out: number[] = []
  for (const part of parts) {
    if (part.length === 0 || part.length > 9 || (part.length > 1 && part.startsWith('0')) || !/^[0-9]+$/.test(part)) return null
    out.push(Number(part))
  }
  return [out[0] as number, out[1] as number, out[2] as number]
}

export function compareVersions(a: Version, b: Version): number {
  for (let i = 0; i < 3; i++) {
    const d = (a[i] as number) - (b[i] as number)
    if (d !== 0) return d < 0 ? -1 : 1
  }
  return 0
}

export function maxVersion(a: Version, b: Version): Version {
  return compareVersions(a, b) >= 0 ? a : b
}

export function minVersion(a: Version, b: Version): Version {
  return compareVersions(a, b) <= 0 ? a : b
}

export function formatVersion(v: Version): string {
  return `${v[0]}.${v[1]}.${v[2]}`
}
