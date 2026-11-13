/** What the rail page reads: the shape of one server on the rail and of the rail as a whole. */
import type { Item } from './layout.ts'

/** What the rail draws for one server (the shape the rail page reads). */
export interface RailInstance {
  id: string
  /** What the rail calls it: the local name if there is one, else `original`. */
  name: string
  /** What the server calls itself, or its address. */
  original: string
  /** The local name the person gave it. */
  alias: string | null
  initial: string
  hue: number
  status: 'starting' | 'ready' | 'refused'
  message: string | null
  unread: number
  mentions: number
  /** The version of the community icon (`rail_icon` fetches it), or none. */
  icon: string | null
  /** Muted until this time (epoch seconds), for ever (-1), or not muted. */
  muted_until: number | null
}

export interface RailState {
  active: string | null
  adding: boolean
  can_add: boolean
  /** In the order they stand on the rail. */
  instances: RailInstance[]
  /** The structure of the rail: servers on their own and folders. */
  items: Item[]
  /** After how many seconds hidden a page is closed; null: never. */
  unload_after: number | null
}

export function initial(name: string): string {
  const first = /[\p{L}\p{N}]/u.exec(name.replace(/^www\./, ''))
  return first ? first[0].toUpperCase() : '?'
}

export function hue(id: string): number {
  return (Number.parseInt(id.slice(0, 4), 16) || 0) % 360
}
