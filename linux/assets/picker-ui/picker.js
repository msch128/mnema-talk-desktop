'use strict';
// Talks to the host through two commands only; the page itself fetches nothing.
const invoke = (cmd, args) => window.__TAURI_INTERNALS__.invoke(cmd, args || {});
const de = /^de\b/i.test(navigator.language || '');
const T = de
  ? { title: 'Bildschirm teilen', detail: 'Wähle einen Bildschirm oder ein Fenster.', share: 'Teilen', cancel: 'Abbrechen', screens: 'Bildschirme', windows: 'Fenster' }
  : { title: 'Share your screen', detail: 'Choose a screen or a window.', share: 'Share', cancel: 'Cancel', screens: 'Screens', windows: 'Windows' };
const $ = (id) => document.getElementById(id);
let chosen = null;

document.documentElement.lang = de ? 'de' : 'en';
document.title = T.title;
$('title').textContent = T.title;
$('detail').textContent = T.detail;
$('share').textContent = T.share;
$('cancel').textContent = T.cancel;

function select(button, id) {
  chosen = id;
  for (const other of document.querySelectorAll('.source')) other.setAttribute('aria-selected', String(other === button));
  $('share').disabled = false;
}

function tile(source) {
  const button = document.createElement('button');
  button.type = 'button';
  button.className = 'source';
  button.setAttribute('role', 'option');
  button.setAttribute('aria-selected', 'false');
  const image = document.createElement('img');
  image.alt = '';
  image.src = source.thumbnail;
  const name = document.createElement('span');
  name.textContent = source.name;
  name.title = source.name;
  button.append(image, name);
  button.addEventListener('click', () => select(button, source.id));
  button.addEventListener('dblclick', () => { select(button, source.id); finish(source.id); });
  return button;
}

async function finish(id) {
  try { await invoke('picker_choose', { id }); } catch (_) { /* the window closes */ }
}

$('share').addEventListener('click', () => finish(chosen));
$('cancel').addEventListener('click', () => finish(null));
document.addEventListener('keydown', (event) => { if (event.key === 'Escape') finish(null); });

invoke('picker_sources').then((sources) => {
  const list = $('sources');
  const screens = sources.filter((s) => s.screen);
  const others = sources.filter((s) => !s.screen);
  list.replaceChildren(...screens.map(tile), ...others.map(tile));
}).catch(() => finish(null));
