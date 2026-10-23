import type { BaseWindow, BrowserWindow, WebContentsView } from 'electron'
import type { SelfUpdater } from '../core/selfupdate.ts'
import type { Trusted } from '../core/trusted.ts'

/** The kinds of page, each with its own session, handler and commands. */
export type Role = 'rail' | 'selector' | 'instance' | 'attachment' | 'updater' | 'picker'

/** Fixed partitions of the host's own pages. A server's page and attachment window get theirs from its id (`serverPartition`). */
export const PARTITIONS = {
  rail: 'mnema-rail',
  selector: 'mnema-selector',
  updater: 'mnema-updater',
  picker: 'mnema-picker',
} as const

/** The partition of the single-server client: its browser storage (end-to-end state) is adopted by the first server added after the update. */
export const LEGACY_PARTITION = 'persist:mnema-instance'

/** The window around the servers: the rail, the page for adding one, and a page per server. */
export interface Shell {
  window: BaseWindow | null
  rail: WebContentsView | null
  selector: WebContentsView | null
  /** The page of each running server, by server id. Hidden, not destroyed, while another is in front. */
  pages: Map<string, WebContentsView>
  /** The attachment window a server's page opened, by server id. */
  attachments: Map<string, BrowserWindow>
  /** The single-server client left storage behind and no list exists yet: the first server added adopts it. */
}

export interface Context {
  readonly trusted: Trusted
  updater: SelfUpdater | null
  readonly debug: boolean
  /** Page kind by web contents id. */
  readonly roles: Map<number, Role>
  /** The server a page or attachment window belongs to, by web contents id. Set by the host, never by the page. */
  readonly servers: Map<number, string>
  /** Partitions whose protocol handler and permissions are set. */
  readonly prepared: Set<string>
  readonly shell: Shell
  readonly windows: Partial<Record<'updater' | 'picker', BrowserWindow>>
}

export function serverPartition(ctx: Context, id: string): string {
  return `persist:inst-${id}`
}

export function attachmentPartition(id: string): string {
  return `mnema-att-${id}`
}
