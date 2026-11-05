'use strict';
// The server rail, part 6: drag and drop.
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

// Where a dragged item goes relative to `anchor`, in the list `list` of ids.
const beforeOf = (zone, list, anchor) => (zone === 'after' ? after(list, anchor) : anchor);

function dropOnServer(d, id, zone, top) {
  if (zone === 'into') {
    edit({ op: 'group', id: d.id, target: id }).then((ok) => { if (ok) { const s = server(d.id); if (s) say(T.grouped(s.name)); } });
    return;
  }
  const home = folderOf(id);
  edit({ op: 'place', id: d.id, target: home ? home.id : '', before: beforeOf(zone, home ? home.servers : top, id) });
}

function drop(d, kind, id, zone) {
  if (d.id === id) return;
  const top = entries().map((e) => e.id);
  if (d.kind === 'folder') {
    const anchor = topOf(kind, id);
    if (anchor !== d.id) edit({ op: 'place', id: d.id, target: '', before: beforeOf(zone, top, anchor) });
  } else if (kind === 'folder') {
    if (zone === 'into') edit({ op: 'place', id: d.id, target: id, before: '' });
    else edit({ op: 'place', id: d.id, target: '', before: beforeOf(zone, top, id) });
  } else dropOnServer(d, id, zone, top);
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

