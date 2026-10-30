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

function answer(button) {
  if (answered) return;
  answered = true;
  const input = $('input');
  try {
    Promise.resolve(host.done({ button, value: spec.input ? input.value : null, color })).catch(() => undefined);
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
  document.addEventListener('keydown', (event) => {
    if (event.key === 'Escape') { event.preventDefault(); answer(spec.cancel); }
    else if (event.key === 'Enter' && !event.isComposing && event.target.tagName !== 'BUTTON' && primary) { event.preventDefault(); answer(primary.id); }
  });
  const first = spec.input ? $('input') : actions[spec.buttons.indexOf(primary)] || actions[0];
  if (first) {
    first.focus();
    if (spec.input) first.select();
  }
}

build();
