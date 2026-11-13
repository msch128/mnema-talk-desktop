'use strict';
// The server rail, last part: the update pill at the foot of the rail. It shows
// while a new version of Mnema Desktop is on offer, being downloaded, waiting
// for a Talk to end or being installed, and stays until the update is done or
// the version is skipped: calm, never a dialog of its own. A click is "Update
// now" (the host asks first during a Talk); while an update runs, waits or
// failed it opens the update window with the details. The host pushes every
// change (`window.mnemaRail.update`); the page also asks while a download runs,
// for the progress. A host without an updater answers nothing, and no pill shows.

const U = de
  ? {
      update: 'Update', afterCall: 'Nach Talk', retry: 'Erneut',
      available: (v) => `Update verfügbar: Version ${v}. Jetzt aktualisieren`,
      downloading: (v, pct) => `Version ${v} wird geladen${pct === null ? '' : `, ${pct} %`}`,
      waiting: (v) => `Version ${v} wird nach dem Talk installiert. Klicken, um jetzt zu aktualisieren`,
      installing: (v) => `Version ${v} wird installiert. Die App startet neu`,
      failed: (v) => `Update auf Version ${v} fehlgeschlagen. Klicken für Details`,
    }
  : {
      update: 'Update', afterCall: 'After call', retry: 'Retry',
      available: (v) => `Update available: version ${v}. Update now`,
      downloading: (v, pct) => `Downloading version ${v}${pct === null ? '' : `, ${pct}%`}`,
      waiting: (v) => `Version ${v} installs after the Talk. Click to update now`,
      installing: (v) => `Installing version ${v}. The app restarts`,
      failed: (v) => `Updating to version ${v} failed. Click for details`,
    };

let updateState = null;
let updateTimer = 0;

const percent = (s) => (s.total ? Math.min(100, Math.round((s.received / s.total) * 100)) : null);

// What the pill shows for a state, or null for no pill.
function pillFor(s) {
  if (!s || !s.version) return null;
  const v = s.version;
  switch (s.phase) {
    case 'available':
    case 'ready':
      return s.skipped ? null : { text: U.update, label: U.available(v), action: 'now', kind: 'offer', fill: null };
    case 'downloading': {
      const pct = percent(s);
      return { text: pct === null ? '…' : `${pct}%`, label: U.downloading(v, pct), action: 'open', kind: 'busy', fill: pct };
    }
    case 'waiting':
      return { text: U.afterCall, label: U.waiting(v), action: 'open', kind: 'busy', fill: null };
    case 'installing':
      return { text: '…', label: U.installing(v), action: 'open', kind: 'busy', fill: null };
    case 'failed':
      return { text: U.retry, label: U.failed(v), action: 'open', kind: 'failed', fill: null };
    default:
      return null;
  }
}

function drawUpdate() {
  const pill = $('update');
  if (!pill) return;
  const shown = pillFor(updateState);
  pill.hidden = !shown;
  document.body.classList.toggle('has-update', Boolean(shown));
  if (!shown) return;
  pill.className = `update ${shown.kind}`;
  pill.textContent = shown.text;
  pill.setAttribute('aria-label', shown.label);
  pill.title = shown.label;
  pill.dataset.action = shown.action;
  pill.style.setProperty('--fill', shown.fill === null ? '0%' : `${shown.fill}%`);
}

// While something runs, the page asks for the progress; otherwise it waits for the host's push.
function followUpdate() {
  clearTimeout(updateTimer);
  if (!updateState || !['checking', 'downloading', 'installing'].includes(updateState.phase)) return;
  updateTimer = setTimeout(() => { askUpdate(); }, 1000);
}

function showUpdate(s) {
  if (s && typeof s === 'object' && typeof s.phase === 'string') updateState = s;
  drawUpdate();
  followUpdate();
}

function askUpdate() {
  return call('rail_update_state', { german: de }).then(showUpdate);
}

$('update').addEventListener('click', () => {
  const action = $('update').dataset.action;
  if (action) call('rail_update_action', { action }).then(showUpdate);
});
window.mnemaRail.update = showUpdate;
askUpdate();
