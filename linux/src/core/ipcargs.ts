/**
 * Arguments and answers shared by the host's command channels. Free of
 * Electron so the checks can be tested.
 */

export type Args = Record<string, unknown>

/** A non-negative whole number (an id, a count, a position); anything else is refused. */
export function wholeNumber(args: Args, name: string): number {
  const value = args[name]
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0) throw new Error('invalid arguments')
  return value
}

/** What a page learns of a failed command: plain text only, no stack and no path, and bounded. */
export function failure(error: unknown): { ok: false, error: string } {
  return { ok: false, error: (error instanceof Error ? error.message : 'The command failed.').slice(0, 300) }
}
