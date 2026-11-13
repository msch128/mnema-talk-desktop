'use strict';
// The server rail, part 4: what a tile or a folder can be told to do.
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
    // The host's import bound (MAX_IMPORT_BYTES): room for the most servers a window holds.
    file: { label: T.importFile, accept: '.json', max: 1048576 },
    buttons: [{ id: 'import', text: T.importList.replace(/\s?…$/, ''), primary: true }, { id: 'cancel', text: T.cancel }],
    cancel: 'cancel',
  }).then((answer) => {
    if (!answer || answer.button !== 'import' || typeof answer.value !== 'string') return;
    attempt('rail_edit', { op: 'import', text: answer.value }).then(({ ok, value }) => { say(ok && typeof value === 'number' ? T.imported(value) : T.importFailed); });
  });
}

// After how long hidden a server's page is closed to save memory (seconds; -1 Off), as the host offers it.
const UNLOAD_CHOICES = [-1, 600, 3600];

// The submenu "Unload idle servers": Off, after 10 minutes, after 1 hour; the one in force is checked.
function unloadMenu(actions) {
  const now = typeof state.unload_after === 'number' ? state.unload_after : -1;
  return {
    id: 'unload',
    text: T.unload,
    items: UNLOAD_CHOICES.map((secs, i) => entry(actions, `unload-${i}`, T.unloadChoices[i], () => {
      attempt('rail_edit', { op: 'unload_after', until: secs }).then(({ ok }) => { say(ok ? T.unloadSet(T.unloadChoices[i]) : T.editFailed); });
    }, { checked: secs === now })),
  };
}

// The menu of the + button.
function addMenu(anchor, byKey) {
  const mine = ++menuSeq;
  const actions = new Map();
  const items = [entry(actions, 'export', T.exportList, exportList), entry(actions, 'import', T.importList, importList), { separator: true }, unloadMenu(actions)];
  const token = `m${mine}-${Math.random().toString(36).slice(2, 8)}`;
  menu = { token, actions };
  const box = byKey ? anchor.getBoundingClientRect() : null;
  attempt('rail_menu', { spec: JSON.stringify({ token, items }), x: box ? Math.round(box.right + 6) : -1, y: box ? Math.round(box.top) : -1 });
}

