'use strict';
// The server rail, part 5: keyboard and menu.
// ------------------------------------------------------------------- keys

// The keys that act on one tile or folder; true when the key was used.
function actKey(event, kind, id, f, s) {
  const open = () => openMenu(kind, id, event.currentTarget, true);
  let run = null;
  if (event.key === 'ContextMenu' || (event.shiftKey && event.key === 'F10')) run = open;
  else if (event.key === 'F2') run = () => (s ? rename(s) : editFolder(f));
  else if (event.key === 'Delete') run = () => (s ? confirmRemove(s) : deleteFolder(f));
  else if (f && ((event.key === 'ArrowRight' && f.collapsed) || (event.key === 'ArrowLeft' && !f.collapsed))) run = () => toggle(f);
  if (!run) return false;
  event.preventDefault();
  run();
  return true;
}

function keys(event, kind, id) {
  const f = kind === 'folder' ? folderById(id) : null;
  const s = kind === 'server' ? server(id) : null;
  if (!f && !s) return;
  if (actKey(event, kind, id, f, s)) return;
  if (event.key !== 'ArrowUp' && event.key !== 'ArrowDown') return;
  event.preventDefault();
  const delta = event.key === 'ArrowUp' ? -1 : 1;
  if (event.altKey) { step(kind, id, delta); return; }
  const all = buttons();
  const next = all[all.indexOf(event.currentTarget) + delta];
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

