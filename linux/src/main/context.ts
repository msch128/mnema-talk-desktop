import type { BrowserWindow } from 'electron'
import type { SelfUpdater } from '../core/selfupdate.ts'
import type { Trusted } from '../core/trusted.ts'

/** The kinds of window, each with its own session, handler and commands. */
export type Role = 'selector' | 'instance' | 'attachment' | 'updater' | 'picker'

export const PARTITIONS: Record<Role, string> = {
  selector: 'mnema-selector',
  // The only persistent one: the app keeps its end-to-end encrypted state in the browser storage of this partition.
  instance: 'persist:mnema-instance',
  attachment: 'mnema-attachment',
  updater: 'mnema-updater',
  picker: 'mnema-picker',
}

export interface Context {
  readonly trusted: Trusted
  updater: SelfUpdater | null
  readonly debug: boolean
  /** Window kind by web contents id. */
  readonly roles: Map<number, Role>
  readonly windows: Partial<Record<Role, BrowserWindow>>
}
