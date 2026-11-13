'use strict';
// Talks to the host through two commands only; the page itself fetches nothing.
const invoke = (cmd, args) => window.__TAURI_INTERNALS__.invoke(cmd, args);
const de = /^de\b/i.test(navigator.language || '');
const T = de
  ? {
      title: { idle: 'Mnema Desktop', up_to_date: 'Du bist auf dem neuesten Stand', checking: 'Suche nach Updates …', available: 'Update verfügbar', downloading: 'Update wird geladen …', ready: 'Update bereit', waiting: 'Update nach dem Talk', installing: 'Update wird installiert …', failed: 'Update fehlgeschlagen', confirm: 'Du bist in einem Talk' },
      available: (v) => `Version ${v} ist verfügbar. Das Update ist signiert und wird vor der Installation geprüft.`,
      up_to_date: 'Es ist keine neuere Version verfügbar.',
      idle: 'Prüfe, ob eine neuere Version vorliegt.',
      checking: 'Das dauert nur einen Moment.',
      downloading_now: (v) => `Version ${v} wird geladen. Danach startet die App neu.`,
      downloading_later: (v) => `Version ${v} wird im Hintergrund geladen. Du kannst weiterarbeiten.`,
      downloading_after_call: (v) => `Version ${v} wird geladen. Sie wird installiert, sobald dein Talk endet.`,
      ready: (v) => `Version ${v} ist geladen und wird beim nächsten Start von Mnema Desktop installiert.`,
      waiting: (v) => `Version ${v} ist geladen und geprüft. Sie wird installiert, sobald dein Talk endet.`,
      confirm: (v) => `Das Update startet Mnema Desktop neu und unterbricht deinen Talk kurz. Version ${v} nach dem Talk installieren oder jetzt?`,
      installing: 'Die App schließt sich gleich und startet danach neu.',
      errors: {
        offline: 'Der Update-Server ist nicht erreichbar. Prüfe deine Internetverbindung und wähle dann „Erneut versuchen“.',
        verify: 'Die Signatur des Updates stimmt nicht, deshalb wurde nichts installiert. Deine Version bleibt unverändert. Wähle „Erneut versuchen“, um das Update neu zu laden.',
        untrusted: 'Die aktuelle Vertrauensliste konnte nicht geladen werden, deshalb wird nichts Neues installiert. Prüfe deine Verbindung und wähle dann „Erneut versuchen“.',
        install: 'Die Installation ist fehlgeschlagen. Die installierte Version bleibt unverändert. Wähle „Erneut versuchen“ oder lade die neue Version von der Release-Seite.',
        folder: 'Mnema Desktop kann nicht in seinen eigenen Ordner schreiben und sich daher nicht ersetzen. Verschiebe den Ordner von Mnema Desktop an einen Ort, an dem du schreiben darfst (etwa „Dokumente“), und wähle dann „Erneut versuchen“.',
        unknown: 'Beim Update ist ein Fehler aufgetreten. Die installierte Version bleibt unverändert. Wähle „Erneut versuchen“.',
      },
      now: 'Jetzt aktualisieren', later: 'Beim nächsten Start', afterCall: 'Nach dem Talk', skip: 'Diese Version überspringen', retry: 'Erneut versuchen', check: 'Erneut prüfen', close: 'Schließen', installNow: 'Jetzt installieren',
      version: (v) => `Installiert: v${v}`,
      external: (v) => `Version ${v} ist verfügbar. Diese Installation wird von der Paketverwaltung verwaltet: lade das neue Paket von der Release-Seite.`,
      downloadPage: 'Release-Seite öffnen',
      releaseLink: 'Release-Hinweise und Download-Seite',
      copy: 'Fehler kopieren', copied: 'Kopiert',
    }
  : {
      title: { idle: 'Mnema Desktop', up_to_date: 'You are up to date', checking: 'Checking for updates…', available: 'Update available', downloading: 'Downloading update…', ready: 'Update ready', waiting: 'Update after the call', installing: 'Installing update…', failed: 'Update failed', confirm: 'You are in a Talk' },
      available: (v) => `Version ${v} is available. The update is signed and verified before it is installed.`,
      up_to_date: 'No newer version is available.',
      idle: 'Checking whether a newer version exists.',
      checking: 'This only takes a moment.',
      downloading_now: (v) => `Downloading version ${v}. The app restarts afterwards.`,
      downloading_later: (v) => `Downloading version ${v} in the background. You can keep working.`,
      downloading_after_call: (v) => `Downloading version ${v}. It installs as soon as your Talk ends.`,
      ready: (v) => `Version ${v} is downloaded and installs the next time Mnema Desktop starts.`,
      waiting: (v) => `Version ${v} is downloaded and verified. It installs as soon as your Talk ends.`,
      confirm: (v) => `Updating restarts Mnema Desktop, which interrupts your Talk for a moment. Install version ${v} after the call, or now?`,
      installing: 'The app closes shortly and restarts.',
      errors: {
        offline: 'The update server cannot be reached. Check your internet connection, then choose Retry.',
        verify: 'The update signature does not match, so nothing was installed. Your version is unchanged. Choose Retry to download the update again.',
        untrusted: 'The current trust statement could not be fetched, so nothing new is installed. Check your connection, then choose Retry.',
        install: 'Installation failed. The installed version is unchanged. Choose Retry, or download the new version from the release page.',
        folder: 'Mnema Desktop cannot write to its own folder, so it cannot replace itself. Move the Mnema Desktop folder somewhere you can write to (such as Documents), then choose Retry.',
        unknown: 'Something went wrong while updating. The installed version is unchanged. Choose Retry.',
      },
      now: 'Update now', later: 'On next start', afterCall: 'After the call', skip: 'Skip this version', retry: 'Retry', check: 'Check again', close: 'Close', installNow: 'Install now',
      version: (v) => `Installed: v${v}`,
      external: (v) => `Version ${v} is available. This installation is managed by your package manager: download the new package from the release page.`,
      downloadPage: 'Open release page',
      releaseLink: 'Release notes / download page',
      copy: 'Copy error', copied: 'Copied',
    };

// "Update now" was asked for during a Talk: the page asks whether to wait for its end.
const confirming = (s) => Boolean(s.confirm_call) && ['available', 'ready', 'failed'].includes(s.phase);
const titleFor = (s) => (confirming(s) ? T.title.confirm : T.title[s.phase] || T.title.idle);

// The full text of a failure for a bug report: the sentence shown and the technical detail behind it.
function errorText(s) {
  const detail = [`error=${s.error || 'unknown'}`, `phase=${s.phase}`, `installed=v${s.current}`];
  if (s.version) detail.push(`version=${s.version}`);
  if (s.mode) detail.push(`mode=${s.mode}`);
  if (s.portable) detail.push('portable');
  if (s.external) detail.push('external');
  return `${T.title.failed}: ${detailFor(s)}\n(${detail.join(', ')}; ${navigator.userAgent})`;
}

// The clipboard API needs a secure context; the selection command works without one.
async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch (_) {
    const area = document.createElement('textarea');
    area.value = text;
    area.style.position = 'fixed';
    area.style.opacity = '0';
    document.body.append(area);
    area.select();
    let ok = false;
    try { ok = document.execCommand('copy'); } catch (_) { /* not copyable here */ }
    area.remove();
    return ok;
  }
}

const $ = (id) => document.getElementById(id);
let last = '';
let shown = null;
let copiedTimer = 0;

function button(label, action, primary) {
  const b = document.createElement('button');
  b.type = 'button';
  b.textContent = label;
  if (primary) b.className = 'primary';
  b.addEventListener('click', () => act(action));
  return b;
}

// What the detail line says for a phase; a function takes the version.
const DETAIL = {
  downloading: (s) => (s.mode === 'later' ? T.downloading_later : s.mode === 'after_call' ? T.downloading_after_call : T.downloading_now),
  ready: () => T.ready,
  waiting: () => T.waiting,
  failed: (s) => T.errors[s.error] || T.errors.unknown,
};

function detailFor(s) {
  let detail;
  if (confirming(s)) detail = T.confirm;
  else if (s.phase === 'available') detail = s.external ? T.external : T.available;
  else detail = DETAIL[s.phase] ? DETAIL[s.phase](s) : T[s.phase] || '';
  return typeof detail === 'function' ? detail(s.version || '') : detail;
}

// A failed attempt is retried the way it was made.
const RETRY = { later: 'later', after_call: 'after_call' };

// The buttons for a phase, as [label, action, primary] triples.
function actionsFor(s) {
  const close = [T.close, 'close'];
  if (confirming(s)) return [[T.afterCall, 'after_call', true], [T.now, 'now_anyway'], close];
  switch (s.phase) {
    case 'available':
      if (s.external) return [[T.downloadPage, 'download_page', true], close];
      // A portable copy replaces its files while it runs; it keeps nothing for the next start.
      return s.portable ? [[T.now, 'now', true], close] : [[T.now, 'now', true], [T.later, 'later']];
    case 'ready':
      return [[T.installNow, 'now', true], close];
    case 'waiting':
      return [[T.now, 'now', true], close];
    case 'failed': {
      const retriable = s.version && !s.external;
      return [[T.retry, retriable ? RETRY[s.mode] || 'now' : 'check', true], close];
    }
    case 'up_to_date':
    case 'idle':
      return [[T.check, 'check'], close];
    default:
      return [];
  }
}

function render(s) {
  const key = JSON.stringify(s);
  if (key === last) return;
  last = key;
  document.documentElement.lang = de ? 'de' : 'en';
  document.title = titleFor(s);
  $('title').textContent = titleFor(s);
  document.querySelector('.mark').className = 'mark' + (s.phase === 'failed' ? ' err' : '');
  shown = s;
  $('detail').textContent = detailFor(s);
  clearTimeout(copiedTimer);
  $('copy').hidden = s.phase !== 'failed';
  $('copy').textContent = T.copy;
  const notes = $('notes');
  notes.hidden = !(s.notes && (s.phase === 'available' || s.phase === 'ready') && !confirming(s));
  notes.textContent = s.notes || '';
  const bar = $('bar');
  const busy = s.phase === 'downloading' || s.phase === 'installing' || s.phase === 'checking';
  bar.hidden = !busy;
  bar.classList.toggle('indeterminate', !(s.phase === 'downloading' && s.total));
  const pct = s.phase === 'downloading' && s.total ? Math.min(100, Math.round((s.received / s.total) * 100)) : 0;
  $('fill').style.width = pct ? pct + '%' : '';
  if (pct) bar.setAttribute('aria-valuenow', String(pct)); else bar.removeAttribute('aria-valuenow');
  $('actions').replaceChildren(...actionsFor(s).map(([label, action, primary]) => button(label, action, primary)));
  const skippable = s.version && (s.phase === 'available' || s.phase === 'failed');
  $('skip').hidden = !skippable;
  $('skip').textContent = T.skip;
  $('version').textContent = T.version(s.current);
  $('release').textContent = T.releaseLink;
}

async function act(action) {
  try { render(await invoke('native_update_action', { action })); } catch (_) { /* the next poll shows the state */ }
}

async function poll() {
  try { render(await invoke('native_update_state')); } catch (_) { /* host gone */ }
}

$('release').addEventListener('click', () => act('release_page'));
$('copy').addEventListener('click', async () => {
  if (!shown || !(await copyText(errorText(shown)))) return;
  $('copy').textContent = T.copied;
  clearTimeout(copiedTimer);
  copiedTimer = setTimeout(() => { $('copy').textContent = T.copy; }, 2000);
});
$('skip').addEventListener('click', () => act('skip'));
poll();
setInterval(poll, 500);
