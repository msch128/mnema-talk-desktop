'use strict';
// The server rail, part 3: drawing the tiles, folders and badges.
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

// The face of a collapsed folder: the first four servers as small pictures.
function folderGrid(members) {
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
  return grid;
}

function folderLabel(f, members) {
  const parts = [folderName(f), T.folderServers(members.length), f.collapsed ? T.collapsed : T.expanded];
  const mentions = mentionsOf(members);
  if (mentions > 0) parts.push(T.mentions(mentions));
  else if (unreadOf(members)) parts.push(T.unread);
  return parts.join(', ');
}

function folderButton(f, members, item) {
  const button = document.createElement('button');
  button.type = 'button';
  button.className = 'folder-btn';
  button.dataset.id = f.id;
  button.dataset.kind = 'folder';
  button.draggable = true;
  button.setAttribute('aria-expanded', f.collapsed ? 'false' : 'true');
  const names = members.map((s) => s.name).join(', ');
  button.title = names ? `${folderName(f)}: ${names}` : folderName(f);
  button.setAttribute('aria-label', folderLabel(f, members));
  if (f.collapsed) {
    button.append(folderGrid(members));
    if (members.some((s) => s.id === state.active) && !state.adding) item.classList.add('current');
    else if (unreadOf(members)) item.classList.add('unread');
  } else button.append(glyph());
  button.addEventListener('click', () => toggle(f));
  button.addEventListener('contextmenu', (event) => { event.preventDefault(); openMenu('folder', f.id, button, false); });
  button.addEventListener('keydown', (event) => keys(event, 'folder', f.id));
  wireDrag(button, 'folder', f.id);
  return button;
}

function folderEntry(f) {
  const members = f.servers.map(server).filter(Boolean);
  const item = document.createElement('li');
  item.className = f.collapsed ? 'folder collapsed' : 'folder';
  if (f.color) item.style.setProperty('--folder', f.color);
  const box = document.createElement('div');
  box.className = 'box';
  box.append(folderButton(f, members, item));
  if (f.collapsed) {
    const marker = markerOf(members);
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

// The server in front stays in sight: when it changes (a click, a shortcut, a
// switch from the menu) the list scrolls just far enough to show it. A redraw
// for anything else leaves the person's scrolling alone.
let inView = null;
function keepActiveInView() {
  const front = state.adding ? null : state.active;
  if (front === inView) return;
  inView = front;
  // A server inside a closed folder has no tile of its own: the folder stands for it.
  const tile = front && [...document.querySelectorAll('.tile')].find((t) => t.dataset.id === front);
  const target = tile || document.querySelector('.folder.current .folder-btn');
  if (target && target.scrollIntoView) target.scrollIntoView({ block: 'nearest' });
}

// Which edges of the list have more beyond them, for the fades.
function markEdges() {
  const scroll = $('scroll');
  if (!scroll) return;
  scroll.classList.toggle('more-above', scroll.scrollTop > 1);
  scroll.classList.toggle('more-below', scroll.scrollTop + scroll.clientHeight < scroll.scrollHeight - 1);
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
  keepActiveInView();
  markEdges();
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

