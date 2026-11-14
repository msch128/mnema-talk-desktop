/** The de/en texts of the update entries of the app menu, as in the Tauri host (`native_updates/menu_text.rs`). */
export interface MenuText { idle: string, offered: string, now: string, check: string }

export const MENU_EN: MenuText = { idle: 'Updates', offered: 'Update available •', now: 'Update now', check: 'Check for updates…' }
export const MENU_DE: MenuText = { idle: 'Updates', offered: 'Update verfügbar •', now: 'Jetzt aktualisieren', check: 'Nach Updates suchen …' }
