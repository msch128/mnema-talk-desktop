'use strict';
// The server rail: one tile per server with its unread dot and mention count,
// servers grouped in folders, and the community's own icon where the server has
// one. The page draws what the host tells it (`window.mnemaRail.set(state)`)
// and asks the host to switch, add, edit the layout or remove; it holds no
// state of its own and fetches nothing. Menus and questions are not drawn here:
// a menu is shown by the system, above every page (`rail_menu`, the choice
// comes back as `window.mnemaRail.chosen`), and a question gets a window of its
// own (`rail_dialog`). Both hosts (Tauri and Electron) ship these files unchanged.
// The page is split by responsibility into plain scripts that share one scope and
// load in this order: rail-text, rail-state, rail-render, rail-commands, rail-menu,
// rail-drag, and this last one, which wires the page up.

// ---------------------------------------------------------------- page

$('add').addEventListener('click', () => {
  call(state.adding && state.instances.length > 0 ? 'rail_cancel_add' : 'rail_add');
});
// What is dropped below the last item goes to the end of the rail.
$('rail').addEventListener('dragover', (event) => {
  if (!drag || event.target.closest('button, .box')) return;
  event.preventDefault();
  clearDrop();
});
$('rail').addEventListener('drop', (event) => {
  if (!drag || event.target.closest('button, .box')) return;
  event.preventDefault();
  finishDrag((d) => edit({ op: 'place', id: d.id, target: '', before: '' }));
});
// A drag that ended where this page saw no event (the tile was replaced) is over once the pointer is used again.
document.addEventListener('pointerdown', () => { if (drag) endDrag(); });
$('add').addEventListener('contextmenu', (event) => { event.preventDefault(); addMenu($('add'), false); });
$('add').addEventListener('keydown', (event) => {
  if (event.key === 'ContextMenu' || (event.shiftKey && event.key === 'F10')) { event.preventDefault(); addMenu($('add'), true); }
});
$('scroll').addEventListener('scroll', markEdges, { passive: true });
window.addEventListener('resize', markEdges);
window.mnemaRail = { set, chosen, quickSwitch };
render();
call('rail_state').then(set);
