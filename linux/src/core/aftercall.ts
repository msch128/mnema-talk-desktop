/**
 * "Update now" and a running Talk, the same rules as the Tauri host
 * (`experimental/app/src/native_updates/logic.rs`): during a Talk the window
 * asks first, "After the call" downloads and verifies now and installs once no
 * Talk has been seen for a few polls in a row, and "Update now" ends that wait.
 */
import type { Phase } from './updatertypes.ts'

export type NowChoice = 'install' | 'ask_about_talk' | 'stop_waiting' | 'show'

/** "Update now" in `phase`. `confirmed` is the answer "update now anyway" to the question about the running Talk. */
export function nowChoice(phase: Phase, offered: boolean, inTalk: boolean, confirmed: boolean): NowChoice {
  if (phase === 'waiting') return 'stop_waiting'
  if ((phase === 'available' || phase === 'ready' || phase === 'failed') && offered) return inTalk && !confirmed ? 'ask_about_talk' : 'install'
  return 'show'
}

/** How many polls in a row must see no Talk: a page that misses one report is not taken for the end of the Talk. */
export const QUIET_POLLS = 3
export const TALK_POLL_MS = 2000

export function callOver(quietPolls: number, forced: boolean): boolean {
  return forced || quietPolls >= QUIET_POLLS
}

/** Resolves once the Talk has ended or the wait was ended by "Update now". */
export async function waitForCallEnd(inTalk: () => boolean, forced: () => boolean, pollMs: number = TALK_POLL_MS): Promise<void> {
  let quiet = 0
  while (!callOver(quiet, forced())) {
    await new Promise((resolve) => setTimeout(resolve, pollMs))
    quiet = inTalk() ? 0 : quiet + 1
  }
}
