'use strict';
// The server rail: one tile per server with its unread dot and mention count.
// The page draws what the host tells it (`window.mnemaRail.set(state)`) and
// asks the host to switch, add, move or remove; it holds no state of its own
// and fetches nothing. Both hosts (Tauri and Electron) ship this file unchanged.
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
    };

const $ = (id) => document.getElementById(id);
let state = { active: null, adding: true, can_add: false, instances: [] };
let dragging = null;
let menuFor = null;
let menuLoad = 0;
let menuSync = null;

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

function label(server) {
  const parts = [server.name];
  if (server.id === state.active && !state.adding) parts.push(T.current);
  if (server.status === 'refused') parts.push(T.refused);
  if (server.status === 'starting') parts.push(T.starting);
  if (server.mentions > 0) parts.push(T.mentions(server.mentions));
  else if (server.unread > 0) parts.push(T.unread);
  return parts.join(', ');
}

function tile(server, index) {
  const item = document.createElement('li');
  const front = server.id === state.active && !state.adding;
  if (front) item.className = 'current';
  else if (server.unread > 0 || server.mentions > 0) item.className = 'unread';
  const button = document.createElement('button');
  button.type = 'button';
  button.className = `tile ${server.status}`;
  button.style.setProperty('--hue', String(server.hue));
  button.dataset.id = server.id;
  button.dataset.index = String(index);
  button.draggable = true;
  button.textContent = server.initial;
  button.title = server.message ? `${server.name}: ${server.message}` : server.name;
  button.setAttribute('aria-label', label(server));
  button.setAttribute('aria-current', front ? 'true' : 'false');
  button.addEventListener('click', () => { closeMenu(); call('rail_activate', { id: server.id }); });
  button.addEventListener('contextmenu', (event) => { event.preventDefault(); openMenu(server, button); });
  button.addEventListener('dragstart', (event) => {
    dragging = server.id;
    button.classList.add('dragging');
    if (event.dataTransfer) { event.dataTransfer.effectAllowed = 'move'; event.dataTransfer.setData('text/plain', server.id); }
  });
  button.addEventListener('dragend', () => { dragging = null; button.classList.remove('dragging'); document.querySelectorAll('.over').forEach((el) => el.classList.remove('over')); });
  button.addEventListener('dragover', (event) => { if (dragging && dragging !== server.id) { event.preventDefault(); button.classList.add('over'); } });
  button.addEventListener('dragleave', () => button.classList.remove('over'));
  button.addEventListener('drop', (event) => {
    event.preventDefault();
    button.classList.remove('over');
    if (dragging && dragging !== server.id) call('rail_move', { id: dragging, to: index });
    dragging = null;
  });
  button.addEventListener('keydown', (event) => keys(event, server, index));
  item.append(button);
  const marker = server.status === 'refused' ? { text: '!', warn: true } : server.mentions > 0 ? { text: count(server.mentions), warn: false } : null;
  if (marker) {
    const badge = document.createElement('span');
    badge.className = marker.warn ? 'badge warn' : 'badge';
    badge.setAttribute('aria-hidden', 'true');
    badge.textContent = marker.text;
    item.append(badge);
  }
  return item;
}

function keys(event, server, index) {
  const buttons = [...document.querySelectorAll('.tile')];
  if (event.key === 'ContextMenu' || (event.shiftKey && event.key === 'F10')) { event.preventDefault(); openMenu(server, event.currentTarget); return; }
  if (event.key === 'Delete') { event.preventDefault(); openMenu(server, event.currentTarget, true); return; }
  if (event.key !== 'ArrowUp' && event.key !== 'ArrowDown') return;
  event.preventDefault();
  const step = event.key === 'ArrowUp' ? -1 : 1;
  if (event.altKey) { move(server, index + step); return; }
  const next = buttons[index + step];
  if (next) next.focus();
}

function move(server, to) {
  if (to < 0 || to >= state.instances.length) return;
  attempt('rail_move', { id: server.id, to }).then(({ ok }) => {
    $('live').textContent = ok ? T.moved(server.name, to + 1) : T.moveFailed(server.name);
    if (!ok) return;
    const moved = document.querySelector(`.tile[data-id="${server.id}"]`);
    if (moved) moved.focus();
  });
}

function closeMenu() {
  const menu = $('menu');
  menu.hidden = true;
  menu.dataset.mode = '';
  menuLoad += 1;
  menuSync = null;
  menu.replaceChildren();
  const owner = menuFor;
  menuFor = null;
  if (owner && owner.isConnected) owner.focus();
}

function entry(text, action, className) {
  const b = document.createElement('button');
  b.type = 'button';
  b.setAttribute('role', 'menuitem');
  b.textContent = text;
  if (className) b.className = className;
  b.addEventListener('click', action);
  return b;
}

function openMenu(server, anchor, confirm) {
  closeMenu();
  menuFor = anchor;
  const menu = $('menu');
  const title = document.createElement('div');
  title.className = 'title';
  title.textContent = server.name;
  menu.append(title);
  menu.dataset.mode = confirm ? 'confirm' : 'main';
  if (confirm) {
    const note = document.createElement('div');
    note.className = 'note';
    note.textContent = T.confirmNote;
    menu.append(note, entry(T.confirm, () => remove(server), 'danger'), entry(T.cancel, closeMenu));
  } else {
    if (server.status === 'refused') menu.append(entry(T.reconnect, () => { closeMenu(); call('rail_reconnect', { id: server.id }); }));
    const at = () => state.instances.findIndex((s) => s.id === server.id);
    const up = entry(T.up, () => { const from = at(); closeMenu(); move(server, from - 1); });
    const down = entry(T.down, () => { const from = at(); closeMenu(); move(server, from + 1); });
    // The order can change under an open menu; its two entries follow it.
    menuSync = () => { const from = at(); up.disabled = from <= 0; down.disabled = from < 0 || from >= state.instances.length - 1; };
    menuSync();
    menu.append(up, down, entry(T.remove, () => openMenu(server, menuFor, true), 'danger'));
    if (server.status === 'ready') loadNotify(server);
  }
  menu.hidden = false;
  const box = anchor.getBoundingClientRect();
  menu.style.left = `${Math.max(4, Math.min(box.right + 6, window.innerWidth - menu.offsetWidth - 4))}px`;
  menu.style.top = `${Math.max(4, Math.min(box.top, window.innerHeight - menu.offsetHeight - 4))}px`;
  const first = menu.querySelector('button:not(:disabled)');
  if (first) first.focus();
}

// The member's server-wide notification level, from the server itself (through
// the host). A server that cannot answer (older, still connecting, or this
// host has no such command) simply gets no entries.
function loadNotify(server) {
  const load = ++menuLoad;
  call('rail_notify_get', { id: server.id }).then((info) => {
    const menu = $('menu');
    if (load !== menuLoad || menu.hidden || menu.dataset.mode !== 'main') return;
    if (!info || info.supported !== true || !(info.level in T.levels)) return;
    const head = document.createElement('div');
    head.className = 'note';
    head.textContent = T.notifications;
    head.setAttribute('role', 'presentation');
    const choices = Object.keys(T.levels).map((level) => {
      const choice = entry(T.levels[level], () => {
        closeMenu();
        call('rail_notify_set', { id: server.id, level }).then((done) => {
          $('live').textContent = done === null ? T.notifySet(server.name, T.levels[level]) : T.notifyFailed(server.name);
        });
      });
      choice.setAttribute('role', 'menuitemradio');
      choice.setAttribute('aria-checked', level === info.level ? 'true' : 'false');
      return choice;
    });
    menu.querySelector('.title').after(head, ...choices);
  });
}

function remove(server) {
  menuFor = null;
  closeMenu();
  attempt('rail_remove', { id: server.id }).then(({ ok }) => { $('live').textContent = ok ? T.removed(server.name) : T.removeFailed(server.name); });
}

function render() {
  const list = $('list');
  list.replaceChildren(...state.instances.map(tile));
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

function set(next) {
  if (!next || !Array.isArray(next.instances)) return;
  state = next;
  // Every push builds new tiles. What the person is working with has to survive
  // that: the focused tile, and the tile an open menu belongs to.
  const focused = document.activeElement && document.activeElement.classList.contains('tile') ? document.activeElement.dataset.id : null;
  const owner = menuFor ? menuFor.dataset.id : null;
  render();
  const tileOf = (id) => [...document.querySelectorAll('.tile')].find((t) => t.dataset.id === id);
  const refocus = focused && tileOf(focused);
  if (refocus) refocus.focus();
  if (owner) {
    const again = tileOf(owner);
    if (again) {
      menuFor = again;
      if (menuSync) menuSync();
    } else closeMenu();
  }
}

$('add').addEventListener('click', () => {
  closeMenu();
  call(state.adding && state.instances.length > 0 ? 'rail_cancel_add' : 'rail_add');
});
document.addEventListener('keydown', (event) => { if (event.key === 'Escape' && !$('menu').hidden) closeMenu(); });
document.addEventListener('pointerdown', (event) => { if (!$('menu').hidden && !$('menu').contains(event.target)) closeMenu(); });
window.addEventListener('blur', closeMenu);
window.mnemaRail = { set };
render();
call('rail_state').then(set);
