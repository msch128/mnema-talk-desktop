'use strict';
// The server rail, part 2: the host bridge, the state of the page and the icons of the servers.
const host = window.__MNEMA_RAIL_HOST__ || { invoke: (cmd, args) => window.__TAURI_INTERNALS__.invoke(cmd, args) };
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

