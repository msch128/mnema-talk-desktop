/**
 * How the servers sit on the rail: in folders or on their own, in the order the
 * person chose, with the local names they gave. Pure bookkeeping, shared in
 * its rules with the Tauri host through `test-vectors/rail-layout.json`. The
 * registry owns one of these and keeps its servers in the order of `flat`.
 */

export interface ServerItem { kind: 'server', id: string }
export interface FolderItem {
  kind: 'folder'
  id: string
  /** May be empty: the rail then shows its own word for it. */
  name: string
  /** `#rrggbb` or none. */
  color: string | null
  collapsed: boolean
  servers: string[]
}
export type Item = ServerItem | FolderItem

export const MAX_NAME_CHARS = 40
const COLOR = /^#[0-9a-fA-F]{6}$/
const FOLDER_ID = /^f-[0-9a-z]{1,16}$/

/** Control characters out, the ends trimmed, at most 40 characters. */
export function cleanName(raw: string): string {
  return Array.from(raw.replace(/\p{Cc}/gu, '').trim()).slice(0, MAX_NAME_CHARS).join('').trim()
}

function cleanColor(raw: unknown): string | null | undefined {
  if (raw === null || raw === undefined) return null
  return typeof raw === 'string' && COLOR.test(raw) ? raw.toLowerCase() : undefined
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

export type Edit =
  | { op: 'place', id: string, target: string, before: string }
  | { op: 'group', id: string, target: string }
  | { op: 'new_folder', id: string }
  | { op: 'ungroup', id: string }
  | { op: 'step', id: string, delta: number }
  | { op: 'toggle', id: string, flag: boolean | null }
  | { op: 'folder_edit', id: string, name: string, color: string | null }
  | { op: 'folder_delete', id: string }
  | { op: 'rename', id: string, name: string }

export class Layout {
  items: Item[] = []
  readonly aliases = new Map<string, string>()
  private readonly known = new Set<string>()

  /**
   * The layout as stored. Anything that does not hold up is dropped: unknown or
   * repeated servers, repeated folder ids, empty folders, bad names and colours.
   * Servers in `order` that the stored items do not place follow at the end.
   */
  static load(rawItems: unknown, rawAliases: unknown, known: readonly string[], order: readonly string[]): Layout {
    const layout = new Layout()
    for (const id of known) layout.known.add(id)
    const seen = new Set<string>()
    const folders = new Set<string>()
    const claim = (id: unknown): id is string => {
      if (typeof id !== 'string' || !layout.known.has(id) || seen.has(id)) return false
      seen.add(id)
      return true
    }
    for (const raw of Array.isArray(rawItems) ? rawItems as unknown[] : []) {
      if (!isRecord(raw)) continue
      if (raw['kind'] === 'server') {
        if (claim(raw['id'])) layout.items.push({ kind: 'server', id: raw['id'] })
      } else if (raw['kind'] === 'folder') {
        const id = raw['id']
        if (typeof id !== 'string' || !FOLDER_ID.test(id) || folders.has(id)) continue
        folders.add(id)
        const servers = (Array.isArray(raw['servers']) ? raw['servers'] as unknown[] : []).filter(claim)
        layout.items.push({
          kind: 'folder',
          id,
          name: typeof raw['name'] === 'string' ? cleanName(raw['name']) : '',
          color: cleanColor(raw['color']) ?? null,
          collapsed: raw['collapsed'] === true,
          servers,
        })
      }
    }
    for (const id of order) if (layout.known.has(id) && !seen.has(id)) { seen.add(id); layout.items.push({ kind: 'server', id }) }
    layout.prune()
    if (isRecord(rawAliases)) {
      for (const [id, name] of Object.entries(rawAliases)) {
        const alias = typeof name === 'string' ? cleanName(name) : ''
        if (layout.known.has(id) && alias !== '') layout.aliases.set(id, alias)
      }
    }
    return layout
  }

  /** The servers in the order they stand on the rail. */
  flat(): string[] {
    return this.items.flatMap((item) => item.kind === 'server' ? [item.id] : item.servers)
  }

  folder(id: string): FolderItem | undefined {
    return this.items.find((item): item is FolderItem => item.kind === 'folder' && item.id === id)
  }

  hasFolder(id: string): boolean {
    return this.folder(id) !== undefined
  }

  private isServer(id: string): boolean {
    return this.known.has(id)
  }

  /** The folder a server is in, if it is in one. */
  private home(id: string): FolderItem | undefined {
    return this.items.find((item): item is FolderItem => item.kind === 'folder' && item.servers.includes(id))
  }

  private topIndex(id: string): number {
    return this.items.findIndex((item) => item.id === id)
  }

  /** Takes a server out of wherever it is. Empty folders stay until `prune`. */
  private detach(id: string): void {
    const folder = this.home(id)
    if (folder) folder.servers.splice(folder.servers.indexOf(id), 1)
    else this.items.splice(this.topIndex(id), 1)
  }

  private prune(): void {
    this.items = this.items.filter((item) => item.kind === 'server' || item.servers.length > 0)
  }

  add(id: string): boolean {
    if (this.known.has(id)) return false
    this.known.add(id)
    this.items.push({ kind: 'server', id })
    return true
  }

  remove(id: string): boolean {
    if (!this.known.has(id)) return false
    this.detach(id)
    this.known.delete(id)
    this.aliases.delete(id)
    this.prune()
    return true
  }

  /** Puts a server or a folder before `before` in a folder (`target`) or at the top (`target` ""); the end if `before` is not there. */
  place(id: string, target: string, before: string): boolean {
    const isFolder = this.hasFolder(id)
    if (!isFolder && !this.isServer(id)) return false
    if (target !== '' && (isFolder || !this.hasFolder(target))) return false
    if (before === id) return true
    const item: Item = isFolder ? this.folder(id) as FolderItem : { kind: 'server', id }
    if (isFolder) this.items.splice(this.topIndex(id), 1)
    else this.detach(id)
    if (target === '') {
      const at = this.topIndex(before)
      this.items.splice(at < 0 ? this.items.length : at, 0, item)
    } else {
      const folder = this.folder(target) as FolderItem
      const at = folder.servers.indexOf(before)
      folder.servers.splice(at < 0 ? folder.servers.length : at, 0, id)
    }
    this.prune()
    return true
  }

  /** `id` was dropped on the server `target`: they share a folder (a new one called `folder` if `target` stood alone). */
  group(id: string, target: string, folder: string): boolean {
    if (!this.isServer(id) || !this.isServer(target) || id === target) return false
    const into = this.home(target)
    if (!into && !this.freeFolderId(folder)) return false
    this.detach(id)
    const home = this.home(target)
    if (home) home.servers.splice(home.servers.indexOf(target) + 1, 0, id)
    else this.items.splice(this.topIndex(target), 1, { kind: 'folder', id: folder, name: '', color: null, collapsed: false, servers: [target, id] })
    this.prune()
    return true
  }

  /** A new folder with just this server, where the server was (right after its old folder). */
  newFolder(id: string, folder: string): boolean {
    if (!this.isServer(id) || !this.freeFolderId(folder)) return false
    const old = this.home(id)
    const at = old ? this.topIndex(old.id) + 1 : this.topIndex(id)
    this.detach(id)
    this.items.splice(at, 0, { kind: 'folder', id: folder, name: '', color: null, collapsed: false, servers: [id] })
    this.prune()
    return true
  }

  private freeFolderId(id: string): boolean {
    return FOLDER_ID.test(id) && !this.hasFolder(id)
  }

  /** Out of its folder, right after it. */
  ungroup(id: string): boolean {
    const folder = this.home(id)
    if (!folder) return false
    const at = this.topIndex(folder.id) + 1
    this.detach(id)
    this.items.splice(at, 0, { kind: 'server', id })
    this.prune()
    return true
  }

  /** One place up (-1) or down (1). A folder and a lone server hop over their neighbours whole; a member leaves its folder at the ends. */
  step(id: string, delta: number): boolean {
    if (delta !== 1 && delta !== -1) return false
    const folder = this.isServer(id) ? this.home(id) : undefined
    if (folder) {
      const at = folder.servers.indexOf(id)
      const to = at + delta
      if (to >= 0 && to < folder.servers.length) {
        const other = folder.servers[to] as string
        folder.servers[to] = id
        folder.servers[at] = other
        return true
      }
      const outside = this.topIndex(folder.id) + (delta === 1 ? 1 : 0)
      this.detach(id)
      this.items.splice(outside, 0, { kind: 'server', id })
      this.prune()
      return true
    }
    const at = this.topIndex(id)
    const to = at + delta
    if (at < 0 || to < 0 || to >= this.items.length) return false
    const other = this.items[to] as Item
    this.items[to] = this.items[at] as Item
    this.items[at] = other
    return true
  }

  toggle(id: string, flag: boolean | null): boolean {
    const folder = this.folder(id)
    if (!folder) return false
    folder.collapsed = flag ?? !folder.collapsed
    return true
  }

  folderEdit(id: string, name: string, color: string | null): boolean {
    const folder = this.folder(id)
    const clean = cleanColor(color === '' ? null : color)
    if (!folder || clean === undefined) return false
    folder.name = cleanName(name)
    folder.color = clean
    return true
  }

  folderDelete(id: string): boolean {
    const at = this.items.findIndex((item) => item.kind === 'folder' && item.id === id)
    if (at < 0) return false
    const folder = this.items[at] as FolderItem
    this.items.splice(at, 1, ...folder.servers.map((server): Item => ({ kind: 'server', id: server })))
    return true
  }

  /** A local name for a server; an empty one takes it away. */
  rename(id: string, name: string): boolean {
    if (!this.isServer(id)) return false
    const alias = cleanName(name)
    if (alias === '') this.aliases.delete(id)
    else this.aliases.set(id, alias)
    return true
  }

  /** `newFolder` is the id a folder made by this edit gets (the host picks it). */
  apply(edit: Edit, newFolder: string): boolean {
    switch (edit.op) {
      case 'place': return this.place(edit.id, edit.target, edit.before)
      case 'group': return this.group(edit.id, edit.target, newFolder)
      case 'new_folder': return this.newFolder(edit.id, newFolder)
      case 'ungroup': return this.ungroup(edit.id)
      case 'step': return this.step(edit.id, edit.delta)
      case 'toggle': return this.toggle(edit.id, edit.flag)
      case 'folder_edit': return this.folderEdit(edit.id, edit.name, edit.color)
      case 'folder_delete': return this.folderDelete(edit.id)
      case 'rename': return this.rename(edit.id, edit.name)
    }
  }
}
