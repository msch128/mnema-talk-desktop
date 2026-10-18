'use strict';
// Talks to the host through two commands only; the page itself fetches nothing.
const invoke = (cmd, args) => window.__TAURI_INTERNALS__.invoke(cmd, args);
const de = /^de\b/i.test(navigator.language || '');
const T = de
  ? {
      title: { idle: 'Mnema Desktop', up_to_date: 'Du bist auf dem neuesten Stand', checking: 'Suche nach Updates …', available: 'Update verfügbar', downloading: 'Update wird geladen …', ready: 'Update bereit', installing: 'Update wird installiert …', failed: 'Update fehlgeschlagen' },
      available: (v) => `Version ${v} ist verfügbar. Das Update ist signiert und wird vor der Installation geprüft.`,
      up_to_date: 'Es ist keine neuere Version verfügbar.',
      idle: 'Prüfe, ob eine neuere Version vorliegt.',
      checking: 'Das dauert nur einen Moment.',
      downloading_now: (v) => `Version ${v} wird geladen. Danach startet die App neu.`,
      downloading_later: (v) => `Version ${v} wird im Hintergrund geladen. Du kannst weiterarbeiten.`,
      ready: (v) => `Version ${v} ist geladen und wird beim nächsten Start von Mnema Desktop installiert.`,
      installing: 'Die App schließt sich gleich und startet danach neu.',
      errors: { offline: 'Der Update-Server ist nicht erreichbar. Prüfe deine Verbindung.', verify: 'Die Signatur des Updates stimmt nicht. Es wurde nicht installiert.', install: 'Die Installation ist fehlgeschlagen. Die installierte Version bleibt unverändert.', unknown: 'Beim Update ist ein Fehler aufgetreten.' },
      now: 'Jetzt aktualisieren', later: 'Beim nächsten Start', skip: 'Diese Version überspringen', retry: 'Erneut versuchen', check: 'Erneut prüfen', close: 'Schließen', installNow: 'Jetzt installieren',
      version: (v) => `Installiert: v${v}`,
      external: (v) => `Version ${v} ist verfügbar. Diese Installation wird von der Paketverwaltung verwaltet (.deb): lade das neue Paket von der Release-Seite.`,
      downloadPage: 'Release-Seite öffnen',
    }
  : {
      title: { idle: 'Mnema Desktop', up_to_date: 'You are up to date', checking: 'Checking for updates…', available: 'Update available', downloading: 'Downloading update…', ready: 'Update ready', installing: 'Installing update…', failed: 'Update failed' },
      available: (v) => `Version ${v} is available. The update is signed and verified before it is installed.`,
      up_to_date: 'No newer version is available.',
      idle: 'Checking whether a newer version exists.',
      checking: 'This only takes a moment.',
      downloading_now: (v) => `Downloading version ${v}. The app restarts afterwards.`,
      downloading_later: (v) => `Downloading version ${v} in the background. You can keep working.`,
      ready: (v) => `Version ${v} is downloaded and installs the next time Mnema Desktop starts.`,
      installing: 'The app closes shortly and restarts.',
      errors: { offline: 'The update server cannot be reached. Check your connection.', verify: 'The update signature does not match. It was not installed.', install: 'Installation failed. The installed version is unchanged.', unknown: 'Something went wrong while updating.' },
      now: 'Update now', later: 'On next start', skip: 'Skip this version', retry: 'Retry', check: 'Check again', close: 'Close', installNow: 'Install now',
      version: (v) => `Installed: v${v}`,
      external: (v) => `Version ${v} is available. This installation is managed by your package manager (.deb): download the new package from the release page.`,
      downloadPage: 'Open release page',
    };

const $ = (id) => document.getElementById(id);
let last = '';

function button(label, action, primary) {
  const b = document.createElement('button');
  b.type = 'button';
  b.textContent = label;
  if (primary) b.className = 'primary';
  b.addEventListener('click', () => act(action));
  return b;
}

function render(s) {
  const key = JSON.stringify(s);
  if (key === last) return;
  last = key;
  document.documentElement.lang = de ? 'de' : 'en';
  document.title = T.title[s.phase] || T.title.idle;
  $('title').textContent = T.title[s.phase] || T.title.idle;
  document.querySelector('.mark').className = 'mark' + (s.phase === 'failed' ? ' err' : '');
  const v = s.version || '';
  let detail;
  if (s.phase === 'available' && s.external) detail = T.external(v);
  else if (s.phase === 'available') detail = T.available(v);
  else if (s.phase === 'downloading') detail = (s.mode === 'later' ? T.downloading_later : T.downloading_now)(v);
  else if (s.phase === 'ready') detail = T.ready(v);
  else if (s.phase === 'failed') detail = T.errors[s.error] || T.errors.unknown;
  else detail = T[s.phase] || '';
  $('detail').textContent = typeof detail === 'function' ? detail(v) : detail;
  const notes = $('notes');
  notes.hidden = !(s.notes && (s.phase === 'available' || s.phase === 'ready'));
  notes.textContent = s.notes || '';
  const bar = $('bar');
  const busy = s.phase === 'downloading' || s.phase === 'installing' || s.phase === 'checking';
  bar.hidden = !busy;
  bar.classList.toggle('indeterminate', !(s.phase === 'downloading' && s.total));
  const pct = s.phase === 'downloading' && s.total ? Math.min(100, Math.round((s.received / s.total) * 100)) : 0;
  $('fill').style.width = pct ? pct + '%' : '';
  if (pct) bar.setAttribute('aria-valuenow', String(pct)); else bar.removeAttribute('aria-valuenow');
  const actions = $('actions');
  actions.replaceChildren();
  if (s.phase === 'available' && s.external) {
    actions.append(button(T.downloadPage, 'download_page', true), button(T.close, 'close'));
  } else if (s.phase === 'available') {
    actions.append(button(T.now, 'now', true), button(T.later, 'later'));
  } else if (s.phase === 'ready') {
    actions.append(button(T.installNow, 'now', true), button(T.close, 'close'));
  } else if (s.phase === 'failed') {
    actions.append(button(T.retry, s.version && !s.external ? (s.mode === 'later' ? 'later' : 'now') : 'check', true), button(T.close, 'close'));
  } else if (s.phase === 'up_to_date' || s.phase === 'idle') {
    actions.append(button(T.check, 'check'), button(T.close, 'close'));
  }
  const skippable = s.version && (s.phase === 'available' || s.phase === 'failed');
  $('skip').hidden = !skippable;
  $('skip').textContent = T.skip;
  $('version').textContent = T.version(s.current);
}

async function act(action) {
  try { render(await invoke('native_update_action', { action })); } catch (_) { /* the next poll shows the state */ }
}

async function poll() {
  try { render(await invoke('native_update_state')); } catch (_) { /* host gone */ }
}

$('skip').addEventListener('click', () => act('skip'));
poll();
setInterval(poll, 500);
