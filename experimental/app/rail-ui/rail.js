'use strict';
// The server rail: one tile per server with its unread dot and mention count,
// servers grouped in folders, and the community's own icon where the server has
// one. The page draws what the host tells it (`window.mnemaRail.set(state)`)
// and asks the host to switch, add, edit the layout or remove; it holds no
// state of its own and fetches nothing. Menus and questions are not drawn here:
// a menu is shown by the system, above every page (`rail_menu`, the choice
// comes back as `window.mnemaRail.chosen`), and a question gets a window of its
// own (`rail_dialog`). Both hosts (Tauri and Electron) ship this file unchanged.
const host = window.__MNEMA_RAIL_HOST__ || { invoke: (cmd, args) => window.__TAURI_INTERNALS__.invoke(cmd, args) };
const de = /^de\b/i.test(navigator.language || '');
const T = de
  ? {
      list: 'Server', add: 'Server hinzufügen', addClose: 'Zurück', addDisabled: 'Es kann kein weiterer Server hinzugefügt werden.',
      current: 'aktiv', refused: 'nicht verfügbar', starting: 'verbindet', unread: 'ungelesen',
      mentions: (n) => (n === 1 ? '1 Erwähnung' : `${n} Erwähnungen`),
      reconnect: 'Erneut verbinden', up: 'Nach oben', down: 'Nach unten', remove: 'Entfernen …',
      confirm: 'Server entfernen', confirmNote: 'Entfernt die Anmeldung und alle lokalen Daten dieses Servers von diesem Gerät.', cancel: 'Abbrechen',
      moved: (name, n) => `${name} an Position ${n}`, removed: (name) => `${name} entfernt`,
      moveFailed: (name) => `${name}: Die Reihenfolge konnte nicht geändert werden`, removeFailed: (name) => `${name}: Der Server konnte nicht entfernt werden`,
      notifications: 'Benachrichtigungen', levels: { all: 'Alle Nachrichten', mentions: 'Nur Erwähnungen', mute: 'Stummschalten' },
      notifySet: (name, level) => `${name}: ${level}`, notifyFailed: (name) => `${name}: Die Einstellung konnte nicht geändert werden`,
      rename: 'Umbenennen …', renameTitle: 'Server umbenennen', renameNote: (original) => `Der Name gilt nur auf diesem Gerät. Der Server heißt weiterhin „${original}“.`,
      nameLabel: 'Name', save: 'Speichern', useOriginal: 'Namen des Servers verwenden', renamed: (name) => `Umbenannt in ${name}`, renameCleared: (name) => `${name}: Der Name des Servers wird verwendet`,
      folder: 'Ordner', addToFolder: 'Zu Ordner hinzufügen', newFolder: 'Neuer Ordner', removeFromFolder: 'Aus dem Ordner nehmen',
      editFolder: 'Ordner bearbeiten …', folderTitle: 'Ordner bearbeiten', folderName: 'Ordnername', folderColor: 'Farbe',
      expand: 'Aufklappen', collapse: 'Zuklappen', deleteFolder: 'Ordner löschen', deleteNote: 'Die Server bleiben erhalten.',
      folderServers: (n) => (n === 1 ? '1 Server' : `${n} Server`), expanded: 'aufgeklappt', collapsed: 'zugeklappt',
      folderMoved: (name) => `${name} verschoben`, folderDeleted: (name) => `${name} gelöscht`, grouped: (name) => `${name} liegt jetzt im Ordner`,
      editFailed: 'Die Änderung konnte nicht gespeichert werden',
      muted: 'stummgeschaltet', mute: 'Server stummschalten', unmute: 'Stummschaltung aufheben', muteFor: ['Für 1 Stunde', 'Für 8 Stunden', 'Bis morgen', 'Bis ich sie aufhebe'],
      muteSet: (name, how) => `${name}: stummgeschaltet, ${how}`, unmuted: (name) => `${name}: Stummschaltung aufgehoben`, markRead: 'Alles als gelesen markieren', markedRead: (name) => `${name}: alles als gelesen markiert`,
      switchTitle: 'Zu Server wechseln', switchLabel: 'Server', switchHint: 'Suchen …', switchGo: 'Wechseln', noMatch: 'Kein Server gefunden',
      exportList: 'Serverliste exportieren …', importList: 'Serverliste importieren …', exported: 'Serverliste exportiert', exportedNote: (path) => `Gespeichert unter ${path}`, ok: 'OK',
      importTitle: 'Serverliste importieren', importNote: 'Wähle eine exportierte Datei. Neue Server werden hinzugefügt, Namen und Ordner übernommen.', importFile: 'Datei (.json)',
      imported: (n) => (n === 1 ? '1 Server hinzugefügt' : `${n} Server hinzugefügt`), importFailed: 'Die Liste konnte nicht importiert werden', exportFailed: 'Die Liste konnte nicht exportiert werden',
      colors: ['Keine', 'Grün', 'Blau', 'Violett', 'Rosa', 'Orange', 'Gelb', 'Türkis', 'Rot', 'Grau'],
    }
  : {
      list: 'Servers', add: 'Add server', addClose: 'Back', addDisabled: 'No further server can be added.',
      current: 'current', refused: 'unavailable', starting: 'connecting', unread: 'unread',
      mentions: (n) => (n === 1 ? '1 mention' : `${n} mentions`),
      reconnect: 'Reconnect', up: 'Move up', down: 'Move down', remove: 'Remove…',
      confirm: 'Remove server', confirmNote: 'Removes the sign-in and all local data of this server from this device.', cancel: 'Cancel',
      moved: (name, n) => `${name} moved to position ${n}`, removed: (name) => `${name} removed`,
      moveFailed: (name) => `${name}: the order could not be changed`, removeFailed: (name) => `${name}: the server could not be removed`,
      notifications: 'Notifications', levels: { all: 'All messages', mentions: 'Only mentions', mute: 'Mute' },
      notifySet: (name, level) => `${name}: ${level}`, notifyFailed: (name) => `${name}: the setting could not be changed`,
      rename: 'Rename…', renameTitle: 'Rename server', renameNote: (original) => `The name only applies on this device. The server is still called “${original}”.`,
      nameLabel: 'Name', save: 'Save', useOriginal: 'Use the name of the server', renamed: (name) => `Renamed to ${name}`, renameCleared: (name) => `${name}: using the name of the server`,
      folder: 'Folder', addToFolder: 'Add to folder', newFolder: 'New folder', removeFromFolder: 'Remove from folder',
      editFolder: 'Edit folder…', folderTitle: 'Edit folder', folderName: 'Folder name', folderColor: 'Colour',
      expand: 'Expand', collapse: 'Collapse', deleteFolder: 'Delete folder', deleteNote: 'The servers stay.',
      folderServers: (n) => (n === 1 ? '1 server' : `${n} servers`), expanded: 'expanded', collapsed: 'collapsed',
      folderMoved: (name) => `${name} moved`, folderDeleted: (name) => `${name} deleted`, grouped: (name) => `${name} is now in the folder`,
      editFailed: 'The change could not be saved',
      muted: 'muted', mute: 'Mute server', unmute: 'Unmute server', muteFor: ['For 1 hour', 'For 8 hours', 'Until tomorrow', 'Until I turn it back on'],
      muteSet: (name, how) => `${name}: muted, ${how}`, unmuted: (name) => `${name}: unmuted`, markRead: 'Mark all as read', markedRead: (name) => `${name}: marked all as read`,
      switchTitle: 'Switch to server', switchLabel: 'Server', switchHint: 'Search…', switchGo: 'Switch', noMatch: 'No server found',
      exportList: 'Export server list…', importList: 'Import server list…', exported: 'Server list exported', exportedNote: (path) => `Saved to ${path}`, ok: 'OK',
      importTitle: 'Import server list', importNote: 'Choose an exported file. New servers are added; names and folders are taken over.', importFile: 'File (.json)',
      imported: (n) => (n === 1 ? '1 server added' : `${n} servers added`), importFailed: 'The list could not be imported', exportFailed: 'The list could not be exported',
      colors: ['None', 'Green', 'Blue', 'Violet', 'Pink', 'Orange', 'Yellow', 'Teal', 'Red', 'Grey'],
    };
const PALETTE = ['', '#2da771', '#3b82f6', '#a855f7', '#ec4899', '#f97316', '#eab308', '#14b8a6', '#ef4444', '#94a3b8'];
// How long a menu waits for the server's own notification level before it opens without it.
const NOTIFY_WAIT = 300;

const $ = (id) => document.getElementById(id);
let state = { active: null, adding: true, can_add: false, instances: [], items: [] };
// What is being dragged ({ kind: 'server' | 'folder', id }). A push of the host
// that arrives meanwhile waits: replacing the dragged tile would end the drag.
let drag = null;
let stale = false;
let menu = null;
let menuSeq = 0;
// The community icons by server: { token, url }. The token names the version the host holds.
const icons = new Map();
const loading = new Set();

// Resolves to { ok, value }; a command the host refuses is { ok: false }.
function attempt(cmd, args) {
  try {
    return Promise.resolve(host.invoke(cmd, args)).then((value) => ({ ok: true, value }), () => ({ ok: false, value: undefined }));
  } catch {
    return Promise.resolve({ ok: false, value: undefined });
  }
}
const call = (cmd, args) => attempt(cmd, args).then((result) => result.value);
const count = (n) => (n > 99 ? '99+' : String(n));
const say = (text) => { $('live').textContent = text; };

const server = (id) => state.instances.find((s) => s.id === id);
const entries = () => (Array.isArray(state.items) && state.items.length > 0 ? state.items : state.instances.map((s) => ({ kind: 'server', id: s.id })));
const folders = () => entries().filter((e) => e.kind === 'folder');
const folderById = (id) => folders().find((f) => f.id === id);
const folderOf = (serverId) => folders().find((f) => f.servers.includes(serverId));
const folderName = (f) => f.name || T.folder;
const shown = (s) => (s.alias ? `${s.name} (${s.original})` : s.name);
// Text for a menu or a question: no control characters (the host refuses them), not too long, never empty.
// Whether a server's notifications are off right now (the host sends epoch seconds, -1 for good).
const muted = (s) => s.muted_until === -1 || (typeof s.muted_until === 'number' && s.muted_until * 1000 > Date.now());
const short = (text) => {
  const clean = [...String(text)].map((ch) => { const c = ch.codePointAt(0); return c < 32 || (c >= 127 && c < 160) ? ' ' : ch; }).join('').trim() || '–';
  const points = [...clean];
  return points.length > 60 ? `${points.slice(0, 59).join('')}…` : clean;
};

function label(s) {
  const parts = [shown(s)];
  if (s.id === state.active && !state.adding) parts.push(T.current);
  if (s.status === 'refused') parts.push(T.refused);
  if (s.status === 'starting') parts.push(T.starting);
  if (muted(s)) parts.push(T.muted);
  else if (s.mentions > 0) parts.push(T.mentions(s.mentions));
  else if (s.unread > 0) parts.push(T.unread);
  return parts.join(', ');
}

// ---------------------------------------------------------------- icons

function fetchIcon(s) {
  if (loading.has(s.id)) return;
  loading.add(s.id);
  call('rail_icon', { id: s.id }).then((url) => {
    loading.delete(s.id);
    const now = server(s.id);
    if (typeof url === 'string' && url.startsWith('data:image/') && now && now.icon === s.icon) {
      icons.set(s.id, { token: s.icon, url });
      redraw();
    } else if (now && now.icon && now.icon !== s.icon) fetchIcon(now);
  });
}

// The icon the host holds for a server, once it has been fetched; else null.
function iconOf(s) {
  if (!s.icon) { icons.delete(s.id); return null; }
  const have = icons.get(s.id);
  if (have && have.token === s.icon) return have.url;
  fetchIcon(s);
  return null;
}

function picture(parent, s, className) {
  const url = iconOf(s);
  if (url) {
    const img = document.createElement('img');
    img.className = className;
    img.alt = '';
    img.draggable = false;
    img.src = url;
    parent.append(img);
  } else parent.append(document.createTextNode(s.initial));
}

// ----------------------------------------------------------------- render

function badge(item, marker) {
  const el = document.createElement('span');
  el.className = marker.warn ? 'badge warn' : 'badge';
  el.setAttribute('aria-hidden', 'true');
  el.textContent = marker.text;
  item.append(el);
}

// Mentions of the servers that are not muted.
const mentionsOf = (list) => list.reduce((sum, s) => sum + (muted(s) || s.status === 'refused' ? 0 : s.mentions), 0);
const unreadOf = (list) => list.some((s) => !muted(s) && (s.unread > 0 || s.mentions > 0));

function markerOf(list) {
  const mentions = mentionsOf(list);
  return mentions > 0 ? { text: count(mentions), warn: false } : null;
}

// A dot for a server that cannot be reached, next to the icon.
function dot(item, list) {
  if (!list.some((s) => s.status === 'refused')) return;
  const el = document.createElement('span');
  el.className = 'dot';
  el.setAttribute('aria-hidden', 'true');
  item.append(el);
}

// A bell with a stroke through it on a muted server.
function bell(item) {
  const ns = 'http://www.w3.org/2000/svg';
  const svg = document.createElementNS(ns, 'svg');
  svg.setAttribute('class', 'mute');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('width', '14');
  svg.setAttribute('height', '14');
  svg.setAttribute('aria-hidden', 'true');
  const path = document.createElementNS(ns, 'path');
  path.setAttribute('fill', 'none');
  path.setAttribute('stroke', 'currentColor');
  path.setAttribute('stroke-width', '2');
  path.setAttribute('stroke-linecap', 'round');
  path.setAttribute('d', 'M6 17h12l-1.5-2V11a4.5 4.5 0 0 0-9 0v4zM10 20h4M4 4l16 16');
  svg.append(path);
  item.append(svg);
}

function tile(s, nested) {
  const item = document.createElement('li');
  const front = s.id === state.active && !state.adding;
  if (front) item.className = 'current';
  else if (unreadOf([s])) item.className = 'unread';
  const button = document.createElement('button');
  button.type = 'button';
  button.className = `tile ${s.status}`;
  button.style.setProperty('--hue', String(s.hue));
  button.dataset.id = s.id;
  button.dataset.kind = 'server';
  button.draggable = true;
  picture(button, s, 'icon');
  button.title = s.message ? `${shown(s)}: ${s.message}` : shown(s);
  button.setAttribute('aria-label', label(s));
  button.setAttribute('aria-current', front ? 'true' : 'false');
  button.addEventListener('click', () => { call('rail_activate', { id: s.id }); });
  button.addEventListener('contextmenu', (event) => { event.preventDefault(); openMenu('server', s.id, button, false); });
  button.addEventListener('keydown', (event) => keys(event, 'server', s.id));
  wireDrag(button, 'server', s.id);
  item.append(button);
  const marker = markerOf([s]);
  if (marker) badge(item, marker);
  dot(item, [s]);
  if (muted(s)) { item.classList.add('muted'); bell(item); }
  if (nested) item.classList.add('nested');
  return item;
}

function glyph() {
  const ns = 'http://www.w3.org/2000/svg';
  const svg = document.createElementNS(ns, 'svg');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('width', '22');
  svg.setAttribute('height', '22');
  svg.setAttribute('aria-hidden', 'true');
  const path = document.createElementNS(ns, 'path');
  path.setAttribute('fill', 'currentColor');
  path.setAttribute('d', 'M3 6.5A2.5 2.5 0 0 1 5.5 4h4.1c.5 0 1 .2 1.4.6l1.4 1.4H18.5A2.5 2.5 0 0 1 21 8.5v9a2.5 2.5 0 0 1-2.5 2.5h-13A2.5 2.5 0 0 1 3 17.5z');
  svg.append(path);
  return svg;
}

function folderEntry(f) {
  const members = f.servers.map(server).filter(Boolean);
  const item = document.createElement('li');
  item.className = f.collapsed ? 'folder collapsed' : 'folder';
  if (f.color) item.style.setProperty('--folder', f.color);
  const box = document.createElement('div');
  box.className = 'box';
  const button = document.createElement('button');
  button.type = 'button';
  button.className = 'folder-btn';
  button.dataset.id = f.id;
  button.dataset.kind = 'folder';
  button.draggable = true;
  button.setAttribute('aria-expanded', f.collapsed ? 'false' : 'true');
  const names = members.map((s) => s.name).join(', ');
  button.title = names ? `${folderName(f)}: ${names}` : folderName(f);
  const parts = [folderName(f), T.folderServers(members.length), f.collapsed ? T.collapsed : T.expanded];
  const marker = markerOf(members);
  const mentions = mentionsOf(members);
  if (mentions > 0) parts.push(T.mentions(mentions));
  else if (unreadOf(members)) parts.push(T.unread);
  button.setAttribute('aria-label', parts.join(', '));
  if (f.collapsed) {
    const grid = document.createElement('span');
    grid.className = 'grid';
    grid.setAttribute('aria-hidden', 'true');
    for (const s of members.slice(0, 4)) {
      const mini = document.createElement('span');
      mini.className = 'mini';
      mini.style.setProperty('--hue', String(s.hue));
      picture(mini, s, 'icon');
      grid.append(mini);
    }
    button.append(grid);
    if (members.some((s) => s.id === state.active) && !state.adding) item.classList.add('current');
    else if (unreadOf(members)) item.classList.add('unread');
  } else button.append(glyph());
  button.addEventListener('click', () => toggle(f));
  button.addEventListener('contextmenu', (event) => { event.preventDefault(); openMenu('folder', f.id, button, false); });
  button.addEventListener('keydown', (event) => keys(event, 'folder', f.id));
  wireDrag(button, 'folder', f.id);
  box.append(button);
  if (f.collapsed) {
    if (marker) badge(box, marker);
    dot(box, members);
  } else {
    const list = document.createElement('ul');
    list.className = 'folder-list';
    list.setAttribute('aria-label', folderName(f));
    list.append(...members.map((s) => tile(s, true)));
    box.append(list);
  }
  // The gaps in an open folder take what is dropped on them.
  box.addEventListener('dragover', (event) => {
    if (!drag || drag.kind !== 'server' || event.target.closest('button')) return;
    event.preventDefault();
    markDrop(item, 'into');
  });
  box.addEventListener('drop', (event) => {
    if (event.target.closest('button')) return;
    event.preventDefault();
    event.stopPropagation();
    finishDrag((d) => (d.kind === 'server' ? edit({ op: 'place', id: d.id, target: f.id, before: '' }) : null));
  });
  item.append(box);
  return item;
}

function render() {
  const list = $('list');
  list.replaceChildren(...entries().map((e) => {
    if (e.kind === 'folder') return folderEntry(e);
    const s = server(e.id);
    return s ? tile(s, false) : document.createComment('');
  }));
  list.setAttribute('aria-label', T.list);
  const add = $('add');
  const canClose = state.adding && state.instances.length > 0;
  add.textContent = canClose ? '×' : '+';
  add.disabled = !canClose && !state.can_add;
  add.setAttribute('aria-pressed', state.adding ? 'true' : 'false');
  const text = canClose ? T.addClose : state.can_add ? T.add : T.addDisabled;
  add.setAttribute('aria-label', canClose || state.can_add ? text : `${T.add}. ${T.addDisabled}`);
  add.title = text;
}

const buttons = () => [...document.querySelectorAll('.tile, .folder-btn')];
const buttonOf = (id) => buttons().find((b) => b.dataset.id === id);

// Every push builds new tiles. What the person is working with has to survive
// that: the focused tile.
let expiry = 0;
function redraw() {
  if (drag) { stale = true; return; }
  stale = false;
  // A mute that runs out changes the look of the rail without the host saying so.
  clearTimeout(expiry);
  const next = state.instances.filter((s) => typeof s.muted_until === 'number' && s.muted_until > 0 && muted(s)).map((s) => s.muted_until * 1000 - Date.now());
  if (next.length > 0) expiry = setTimeout(redraw, Math.min(Math.min(...next) + 500, 2147483647));
  const active = document.activeElement;
  const focused = active && active.dataset && active.dataset.kind ? active.dataset.id : null;
  render();
  const again = focused && buttonOf(focused);
  if (again) again.focus();
}

function set(next) {
  if (!next || !Array.isArray(next.instances)) return;
  state = next;
  redraw();
}

// --------------------------------------------------------------- commands

// Changes the layout; resolves to whether the host did it.
function edit(args) {
  return attempt('rail_edit', args).then(({ ok }) => {
    if (!ok) say(T.editFailed);
    return ok;
  });
}

function toggle(f) {
  edit({ op: 'toggle', id: f.id, flag: !f.collapsed }).then((ok) => {
    if (!ok) return;
    const again = buttonOf(f.id);
    if (again) again.focus();
    say(`${folderName(f)}, ${f.collapsed ? T.expanded : T.collapsed}`);
  });
}

function step(kind, id, delta) {
  edit({ op: 'step', id, delta }).then((ok) => {
    if (!ok) return;
    const again = buttonOf(id);
    if (again) again.focus();
    if (kind === 'folder') {
      const f = folderById(id);
      if (f) say(T.folderMoved(folderName(f)));
    } else {
      const s = server(id);
      if (s) say(T.moved(s.name, state.instances.findIndex((x) => x.id === id) + 1));
    }
  });
}

function ask(spec) {
  return attempt('rail_dialog', { spec: JSON.stringify(spec) }).then(({ ok, value }) => (ok && value && typeof value === 'object' ? value : null));
}

function rename(s) {
  ask({
    title: T.renameTitle,
    message: T.renameNote(s.original),
    input: { label: T.nameLabel, value: s.alias || '', max: 40, placeholder: [...short(s.original)].slice(0, 40).join('') },
    buttons: [{ id: 'save', text: T.save, primary: true }, { id: 'cancel', text: T.cancel }],
    cancel: 'cancel',
  }).then((answer) => {
    if (!answer || answer.button !== 'save') return;
    const name = String(answer.value || '').trim();
    // The name of the server itself is no alias.
    const clear = name === '' || name === s.original;
    edit({ op: 'rename', id: s.id, name: clear ? '' : name }).then((ok) => {
      if (ok) say(clear ? T.renameCleared(s.original) : T.renamed(name));
    });
  });
}

function editFolder(f) {
  ask({
    title: T.folderTitle,
    input: { label: T.folderName, value: f.name, max: 40, placeholder: T.folder },
    colors: { label: T.folderColor, value: f.color || '', choices: PALETTE.map((value, i) => ({ value, label: T.colors[i] })) },
    buttons: [{ id: 'save', text: T.save, primary: true }, { id: 'cancel', text: T.cancel }],
    cancel: 'cancel',
  }).then((answer) => {
    if (!answer || answer.button !== 'save') return;
    edit({ op: 'folder_edit', id: f.id, name: String(answer.value || ''), color: answer.color || '' });
  });
}

function confirmRemove(s) {
  ask({
    title: `${T.confirm}: ${short(shown(s))}`,
    message: T.confirmNote,
    buttons: [{ id: 'remove', text: T.confirm, danger: true }, { id: 'cancel', text: T.cancel, primary: true }],
    cancel: 'cancel',
  }).then((answer) => {
    if (!answer || answer.button !== 'remove') return;
    attempt('rail_remove', { id: s.id }).then(({ ok }) => { say(ok ? T.removed(s.name) : T.removeFailed(s.name)); });
  });
}

function deleteFolder(f) {
  edit({ op: 'folder_delete', id: f.id }).then((ok) => { if (ok) say(T.folderDeleted(folderName(f))); });
}

// A server into a folder that does not exist yet; the person names it right away.
function newFolder(s) {
  edit({ op: 'new_folder', id: s.id }).then((ok) => {
    if (!ok) return;
    call('rail_state').then((next) => {
      set(next);
      const made = folderOf(s.id);
      if (made) editFolder(made);
    });
  });
}

// Mutes for a number of seconds from now, or until a given moment (epoch seconds), or for good (-1); 0 turns it off.
function mute(s, until, how) {
  const at = until > 0 && until < 1e9 ? Math.floor(Date.now() / 1000) + until : until;
  edit({ op: 'mute', id: s.id, until: at }).then((ok) => { if (ok) say(at === 0 ? T.unmuted(s.name) : T.muteSet(s.name, how)); });
}

// Tomorrow at eight in the morning, local time, as epoch seconds.
function tomorrow() {
  const d = new Date();
  d.setDate(d.getDate() + 1);
  d.setHours(8, 0, 0, 0);
  return Math.floor(d.getTime() / 1000);
}

function markRead(s) {
  edit({ op: 'mark_read', id: s.id }).then((ok) => { if (ok) say(T.markedRead(s.name)); });
}

// The servers in the order of the rail, in a list to type into; the choice switches to that server.
function quickSwitch() {
  if (state.instances.length === 0) return;
  const choices = state.instances.slice(0, 12).map((s) => {
    const home = folderOf(s.id);
    const hint = s.alias ? s.original : home ? folderName(home) : '';
    return { id: s.id, text: short(shown(s)), ...(hint ? { hint: short(hint) } : {}) };
  });
  ask({
    title: T.switchTitle,
    list: { label: T.switchLabel, placeholder: T.switchHint, choices },
    buttons: [{ id: 'pick', text: T.switchGo, primary: true }, { id: 'cancel', text: T.cancel }],
    cancel: 'cancel',
  }).then((answer) => {
    if (answer && answer.button === 'pick' && typeof answer.value === 'string') call('rail_activate', { id: answer.value });
  });
}

// The list of servers as a file: out through the host, in from a file the person picks.
function exportList() {
  attempt('rail_edit', { op: 'export' }).then(({ ok, value }) => {
    if (!ok || typeof value !== 'string') { say(T.exportFailed); return; }
    ask({ title: T.exported, message: T.exportedNote(value), buttons: [{ id: 'ok', text: T.ok, primary: true }], cancel: 'ok' });
  });
}

function importList() {
  ask({
    title: T.importTitle,
    message: T.importNote,
    file: { label: T.importFile, accept: '.json', max: 65536 },
    buttons: [{ id: 'import', text: T.importList.replace(/\s?…$/, ''), primary: true }, { id: 'cancel', text: T.cancel }],
    cancel: 'cancel',
  }).then((answer) => {
    if (!answer || answer.button !== 'import' || typeof answer.value !== 'string') return;
    attempt('rail_edit', { op: 'import', name: answer.value }).then(({ ok, value }) => { say(ok && typeof value === 'number' ? T.imported(value) : T.importFailed); });
  });
}

// The menu of the + button.
function addMenu(anchor, byKey) {
  const mine = ++menuSeq;
  const actions = new Map();
  const items = [entry(actions, 'export', T.exportList, exportList), entry(actions, 'import', T.importList, importList)];
  const token = `m${mine}-${Math.random().toString(36).slice(2, 8)}`;
  menu = { token, actions };
  const box = byKey ? anchor.getBoundingClientRect() : null;
  attempt('rail_menu', { spec: JSON.stringify({ token, items }), x: box ? Math.round(box.right + 6) : -1, y: box ? Math.round(box.top) : -1 });
}

// ------------------------------------------------------------------- keys

function keys(event, kind, id) {
  const target = event.currentTarget;
  const f = kind === 'folder' ? folderById(id) : null;
  const s = kind === 'server' ? server(id) : null;
  if (!f && !s) return;
  if (event.key === 'ContextMenu' || (event.shiftKey && event.key === 'F10')) { event.preventDefault(); openMenu(kind, id, target, true); return; }
  if (event.key === 'F2') { event.preventDefault(); if (s) rename(s); else editFolder(f); return; }
  if (event.key === 'Delete') { event.preventDefault(); if (s) confirmRemove(s); else deleteFolder(f); return; }
  if (f && event.key === 'ArrowRight' && f.collapsed) { event.preventDefault(); toggle(f); return; }
  if (f && event.key === 'ArrowLeft' && !f.collapsed) { event.preventDefault(); toggle(f); return; }
  if (event.key !== 'ArrowUp' && event.key !== 'ArrowDown') return;
  event.preventDefault();
  const delta = event.key === 'ArrowUp' ? -1 : 1;
  if (event.altKey) { step(kind, id, delta); return; }
  const all = buttons();
  const next = all[all.indexOf(target) + delta];
  if (next) next.focus();
}

// ------------------------------------------------------------------- menu

// What an item of the menu does is kept here, by id; the host only ever sees
// text, ids and states, and hands the id back when the person picks one.
function entry(actions, id, text, run, extra) {
  actions.set(id, run);
  return { id, text: short(text), ...extra };
}

function serverMenu(s, actions, info) {
  const at = state.instances.findIndex((x) => x.id === s.id);
  const home = folderOf(s.id);
  const items = [{ id: 'title', text: short(shown(s)), disabled: true }, { separator: true }];
  if (s.status === 'refused') items.push(entry(actions, 'reconnect', T.reconnect, () => call('rail_reconnect', { id: s.id })));
  if (info) {
    items.push({
      id: 'notifications',
      text: T.notifications,
      items: Object.keys(T.levels).map((level) => entry(actions, `notify-${level}`, T.levels[level], () => {
        call('rail_notify_set', { id: s.id, level }).then((done) => { say(done === null ? T.notifySet(s.name, T.levels[level]) : T.notifyFailed(s.name)); });
      }, { checked: level === info.level })),
    });
    items.push({ separator: true });
  }
  if (s.status === 'ready') items.push(entry(actions, 'mark-read', T.markRead, () => markRead(s)));
  if (muted(s)) items.push(entry(actions, 'unmute', T.unmute, () => mute(s, 0)));
  else {
    const ways = [3600, 8 * 3600, tomorrow(), -1];
    items.push({ id: 'mute', text: T.mute, items: ways.map((until, i) => entry(actions, `mute-${i}`, T.muteFor[i], () => mute(s, until, T.muteFor[i]))) });
  }
  items.push({ separator: true });
  items.push(entry(actions, 'rename', T.rename, () => rename(s)));
  if (s.alias) items.push(entry(actions, 'unalias', T.useOriginal, () => edit({ op: 'rename', id: s.id, name: '' })));
  const into = [entry(actions, 'folder-new', T.newFolder, () => newFolder(s))];
  for (const f of folders()) {
    into.push(entry(actions, `to-${f.id}`, folderName(f), () => edit({ op: 'place', id: s.id, target: f.id, before: '' }).then((ok) => { if (ok) say(T.grouped(s.name)); }), { disabled: home && home.id === f.id }));
  }
  items.push({ id: 'folders', text: T.addToFolder, items: into.slice(0, 24) });
  if (home) items.push(entry(actions, 'ungroup', T.removeFromFolder, () => edit({ op: 'ungroup', id: s.id })));
  items.push({ separator: true });
  const first = at <= 0 && !home;
  const last = at < 0 || (at >= state.instances.length - 1 && !home);
  items.push(entry(actions, 'up', T.up, () => step('server', s.id, -1), { disabled: first }));
  items.push(entry(actions, 'down', T.down, () => step('server', s.id, 1), { disabled: last }));
  items.push({ separator: true });
  items.push(entry(actions, 'remove', T.remove, () => confirmRemove(s)));
  return items;
}

function folderMenu(f, actions) {
  const top = entries();
  const at = top.findIndex((e) => e.id === f.id);
  return [
    { id: 'title', text: short(folderName(f)), disabled: true },
    { separator: true },
    entry(actions, 'toggle', f.collapsed ? T.expand : T.collapse, () => toggle(f)),
    entry(actions, 'edit', T.editFolder, () => editFolder(f)),
    { separator: true },
    entry(actions, 'up', T.up, () => step('folder', f.id, -1), { disabled: at <= 0 }),
    entry(actions, 'down', T.down, () => step('folder', f.id, 1), { disabled: at < 0 || at >= top.length - 1 }),
    { separator: true },
    entry(actions, 'delete', T.deleteFolder, () => deleteFolder(f)),
  ];
}

// The member's server-wide notification level, from the server itself (through
// the host). A server that cannot answer (older, still connecting, or this
// host has no such command), or does not answer in time, simply gets no entries.
function notifyInfo(s) {
  if (s.status !== 'ready') return Promise.resolve(null);
  const wait = new Promise((resolve) => { setTimeout(() => resolve(null), NOTIFY_WAIT); });
  return Promise.race([call('rail_notify_get', { id: s.id }), wait]).then((info) => (info && info.supported === true && info.level in T.levels ? info : null));
}

async function openMenu(kind, id, anchor, byKey) {
  const mine = ++menuSeq;
  menu = null;
  const s = kind === 'server' ? server(id) : null;
  const f = kind === 'folder' ? folderById(id) : null;
  if (!s && !f) return;
  const info = s ? await notifyInfo(s) : null;
  // A newer menu, or a push that took the server away, came meanwhile.
  if (mine !== menuSeq || (s ? !server(id) : !folderById(id))) return;
  const actions = new Map();
  const items = s ? serverMenu(server(id), actions, info) : folderMenu(folderById(id), actions);
  const token = `m${mine}-${Math.random().toString(36).slice(2, 8)}`;
  menu = { token, actions };
  const box = anchor.isConnected ? anchor.getBoundingClientRect() : null;
  // The pointer places a menu opened with the mouse; one opened with the keyboard sits beside its tile.
  const x = byKey && box ? Math.round(box.right + 6) : -1;
  const y = byKey && box ? Math.round(box.top) : -1;
  attempt('rail_menu', { spec: JSON.stringify({ token, items }), x, y });
}

// The host reports the item the person picked in the menu it showed last.
function chosen(token, id) {
  if (!menu || menu.token !== token) return;
  const run = menu.actions.get(id);
  if (!run) return;
  menu = null;
  run();
}

// ---------------------------------------------------------------- drag and drop

const SLOT = { before: 'drop-before', after: 'drop-after', into: 'drop-into' };

function clearDrop() {
  document.querySelectorAll('.drop-before, .drop-after, .drop-into').forEach((el) => el.classList.remove(...Object.values(SLOT)));
}

function markDrop(el, zone) {
  clearDrop();
  el.classList.add(SLOT[zone]);
}

// Where on a tile the pointer is: its upper edge, its lower edge, or the middle, which drops onto it.
function zoneOf(event, el, canInto) {
  const box = el.getBoundingClientRect();
  const at = box.height > 0 ? (event.clientY - box.top) / box.height : 0.5;
  if (canInto && at >= 0.25 && at <= 0.75) return 'into';
  return at < 0.5 ? 'before' : 'after';
}

function endDrag() {
  drag = null;
  clearDrop();
  document.querySelectorAll('.dragging').forEach((el) => el.classList.remove('dragging'));
  if (stale) redraw();
}

function finishDrag(run) {
  const d = drag;
  endDrag();
  if (d) run(d);
}

// The top level item a tile belongs to: itself, or its folder.
const topOf = (kind, id) => (kind === 'server' ? (folderOf(id) || { id }).id : id);
// The item after another in its container, '' for the last.
function after(list, id) {
  const i = list.indexOf(id);
  return i >= 0 && i + 1 < list.length ? list[i + 1] : '';
}

function drop(d, kind, id, zone) {
  if (d.id === id) return;
  const top = entries().map((e) => e.id);
  if (d.kind === 'folder') {
    const anchor = topOf(kind, id);
    if (anchor === d.id) return;
    edit({ op: 'place', id: d.id, target: '', before: zone === 'after' ? after(top, anchor) : anchor });
    return;
  }
  if (kind === 'folder') {
    if (zone === 'into') edit({ op: 'place', id: d.id, target: id, before: '' });
    else edit({ op: 'place', id: d.id, target: '', before: zone === 'after' ? after(top, id) : id });
    return;
  }
  if (zone === 'into') {
    edit({ op: 'group', id: d.id, target: id }).then((ok) => { if (ok) { const s = server(d.id); if (s) say(T.grouped(s.name)); } });
    return;
  }
  const home = folderOf(id);
  const list = home ? home.servers : top;
  edit({ op: 'place', id: d.id, target: home ? home.id : '', before: zone === 'after' ? after(list, id) : id });
}

function wireDrag(button, kind, id) {
  const li = () => button.closest('li');
  button.addEventListener('dragstart', (event) => {
    drag = { kind, id };
    button.classList.add('dragging');
    if (event.dataTransfer) { event.dataTransfer.effectAllowed = 'move'; event.dataTransfer.setData('text/plain', id); }
  });
  button.addEventListener('dragend', endDrag);
  button.addEventListener('dragover', (event) => {
    if (!drag || drag.id === id) return;
    event.preventDefault();
    event.stopPropagation();
    // A server can go onto a server or a folder; a folder only between items.
    const zone = zoneOf(event, button, drag.kind === 'server');
    markDrop(zone === 'into' && kind === 'server' ? button : li(), zone);
  });
  button.addEventListener('dragleave', () => clearDrop());
  button.addEventListener('drop', (event) => {
    if (!drag) return;
    event.preventDefault();
    event.stopPropagation();
    const zone = zoneOf(event, button, drag.kind === 'server');
    finishDrag((d) => drop(d, kind, id, zone));
  });
}

// ---------------------------------------------------------------- page

$('add').addEventListener('click', () => {
  call(state.adding && state.instances.length > 0 ? 'rail_cancel_add' : 'rail_add');
});
// What is dropped below the last item goes to the end of the rail.
$('rail').addEventListener('dragover', (event) => {
  if (!drag || event.target.closest('button, .box')) return;
  event.preventDefault();
  clearDrop();
});
$('rail').addEventListener('drop', (event) => {
  if (!drag || event.target.closest('button, .box')) return;
  event.preventDefault();
  finishDrag((d) => edit({ op: 'place', id: d.id, target: '', before: '' }));
});
// A drag that ended where this page saw no event (the tile was replaced) is over once the pointer is used again.
document.addEventListener('pointerdown', () => { if (drag) endDrag(); });
$('add').addEventListener('contextmenu', (event) => { event.preventDefault(); addMenu($('add'), false); });
$('add').addEventListener('keydown', (event) => {
  if (event.key === 'ContextMenu' || (event.shiftKey && event.key === 'F10')) { event.preventDefault(); addMenu($('add'), true); }
});
window.mnemaRail = { set, chosen, quickSwitch };
render();
call('rail_state').then(set);
