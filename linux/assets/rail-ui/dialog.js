'use strict';
// The question of the server rail, shown in a window of its own: a title, a
// note, a text field, a choice of colour and up to three buttons, as the rail
// described them. The page takes the description from the host, answers once
// (`done`) and fetches nothing. Both hosts (Tauri and Electron) ship this file
// unchanged.
const host = window.__MNEMA_DIALOG_HOST__ || {
  spec: window.__MNEMA_DIALOG__,
  done: (result) => window.__TAURI_INTERNALS__.invoke('dialog_done', result),
};
const spec = host.spec && typeof host.spec === 'object' ? host.spec : { title: '', buttons: [], cancel: '' };
const $ = (id) => document.getElementById(id);
let answered = false;
let color = spec.colors ? String(spec.colors.value || '') : null;
// What the person picked from the list, or the text of the file they chose.
let picked = null;

function answer(button) {
  if (answered) return;
  answered = true;
  const input = $('input');
  try {
    Promise.resolve(host.done({ button, value: spec.input ? input.value : picked, color })).catch(() => undefined);
  } catch {
    // The window goes away with the host's answer; there is nothing else to tell.
  }
}

function swatches() {
  const row = $('swatches');
  const radios = spec.colors.choices.map((choice) => {
    const radio = document.createElement('button');
    radio.type = 'button';
    radio.className = choice.value ? 'swatch' : 'swatch none';
    if (choice.value) radio.style.setProperty('--swatch', choice.value);
    radio.dataset.value = choice.value;
    radio.title = choice.label;
    radio.setAttribute('role', 'radio');
    radio.setAttribute('aria-label', choice.label);
    return radio;
  });
  const pick = (radio, focus) => {
    color = radio.dataset.value;
    for (const other of radios) {
      other.setAttribute('aria-checked', other === radio ? 'true' : 'false');
      other.tabIndex = other === radio ? 0 : -1;
    }
    if (focus) radio.focus();
  };
  radios.forEach((radio, i) => {
    radio.addEventListener('click', () => pick(radio, false));
    radio.addEventListener('keydown', (event) => {
      const step = event.key === 'ArrowRight' || event.key === 'ArrowDown' ? 1 : event.key === 'ArrowLeft' || event.key === 'ArrowUp' ? -1 : 0;
      if (!step) return;
      event.preventDefault();
      pick(radios[(i + step + radios.length) % radios.length], true);
    });
  });
  row.replaceChildren(...radios);
  pick(radios.find((radio) => radio.dataset.value === color) || radios[0], false);
}

// A list to filter by typing: arrows move, Enter takes the marked one.
function chooser(primaryButton) {
  const input = $('input');
  const list = $('choices');
  $('field').hidden = false;
  $('label').textContent = spec.list.label;
  if (spec.list.placeholder) input.placeholder = spec.list.placeholder;
  list.hidden = false;
  list.setAttribute('aria-label', spec.list.label);
  let shown = [];
  let at = 0;
  const mark = () => {
    [...list.children].forEach((el, i) => el.setAttribute('aria-selected', i === at ? 'true' : 'false'));
    picked = shown[at] ? shown[at].id : null;
    if (primaryButton) primaryButton.disabled = picked === null;
    const marked = list.children[at];
    if (marked && marked.scrollIntoView) marked.scrollIntoView({ block: 'nearest' });
  };
  const fill = () => {
    const words = input.value.trim().toLowerCase();
    shown = spec.list.choices.filter((c) => !words || `${c.text} ${c.hint || ''}`.toLowerCase().includes(words));
    at = 0;
    list.replaceChildren(...shown.map((c, i) => {
      const li = document.createElement('li');
      li.setAttribute('role', 'option');
      const text = document.createElement('span');
      text.textContent = c.text;
      li.append(text);
      if (c.hint) {
        const hint = document.createElement('span');
        hint.className = 'hint';
        hint.textContent = c.hint;
        li.append(hint);
      }
      li.addEventListener('click', () => { at = i; mark(); answer(primaryButton ? spec.buttons.find((b) => b.primary).id : spec.cancel); });
      return li;
    }));
    mark();
  };
  input.value = '';
  input.addEventListener('input', fill);
  input.addEventListener('keydown', (event) => {
    if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
    event.preventDefault();
    if (shown.length === 0) return;
    at = (at + (event.key === 'ArrowDown' ? 1 : -1) + shown.length) % shown.length;
    mark();
  });
  fill();
}

// A file to choose; its text goes back as the answer. Nothing leaves the page but that text.
function filer(primaryButton) {
  $('filebox').hidden = false;
  $('filelabel').textContent = spec.file.label;
  if (primaryButton) primaryButton.disabled = true;
  $('file').addEventListener('change', () => {
    const file = $('file').files[0];
    picked = null;
    if (primaryButton) primaryButton.disabled = true;
    if (!file || file.size > spec.file.max) return;
    file.text().then((text) => {
      picked = text;
      if (primaryButton) primaryButton.disabled = false;
    }, () => undefined);
  });
}

function build() {
  document.title = spec.title;
  $('title').textContent = spec.title;
  if (spec.message) { $('message').textContent = spec.message; $('message').hidden = false; }
  else $('dialog').removeAttribute('aria-describedby');
  if (spec.input) {
    $('field').hidden = false;
    $('label').textContent = spec.input.label;
    const input = $('input');
    input.value = spec.input.value || '';
    input.maxLength = spec.input.max || 40;
    if (spec.input.placeholder) input.placeholder = spec.input.placeholder;
  }
  if (spec.colors) {
    $('colors').hidden = false;
    $('colors-label').textContent = spec.colors.label;
    swatches();
  }
  const primary = spec.buttons.find((button) => button.primary) || spec.buttons[0];
  const actions = spec.buttons.map((button) => {
    const el = document.createElement('button');
    el.type = 'button';
    el.textContent = button.text;
    if (button.primary) el.classList.add('primary');
    if (button.danger) el.classList.add('danger');
    el.addEventListener('click', () => answer(button.id));
    return el;
  });
  $('actions').replaceChildren(...actions);
  const main = actions[spec.buttons.indexOf(primary)] || null;
  if (spec.list) chooser(main);
  if (spec.file) filer(main);
  document.addEventListener('keydown', (event) => {
    if (event.key === 'Escape') { event.preventDefault(); answer(spec.cancel); }
    else if (event.key === 'Enter' && !event.isComposing && event.target.tagName !== 'BUTTON' && primary) {
      event.preventDefault();
      if (!(spec.list || spec.file) || picked !== null) answer(primary.id);
    }
  });
  const first = spec.input || spec.list ? $('input') : spec.file ? $('file') : actions[spec.buttons.indexOf(primary)] || actions[0];
  if (first) {
    first.focus();
    if (spec.input) first.select();
  }
}

build();
