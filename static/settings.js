const $ = (id) => document.getElementById(id);

async function load() {
  try {
    const r = await fetch('/api/credentials');
    const j = await r.json();
    if (!j.ok) throw new Error(j.error || 'unknown error');
    render(j);
  } catch (e) {
    showError(e.message);
  }
}

function render(data) {
  $('config-path').textContent = data.path || '—';
  const tbody = document.querySelector('#cred-table tbody');
  tbody.innerHTML = '';
  for (const c of data.credentials) {
    const tr = document.createElement('tr');
    const valCell = c.is_set
      ? `<span class="set">${esc(c.value_masked)}</span>`
      : `<span class="unset">not set</span>`;
    tr.innerHTML = `
      <td class="key">${esc(c.key)}</td>
      <td class="desc">${esc(c.description || '')}</td>
      <td class="val">${valCell}</td>
      <td class="actions">
        <button data-act="edit" data-key="${esc(c.key)}">${c.is_set ? 'Update' : 'Set'}</button>
        ${c.is_set ? `<button data-act="del" data-key="${esc(c.key)}" class="danger">Unset</button>` : ''}
      </td>`;
    tbody.appendChild(tr);
  }
  document.querySelectorAll('button[data-act]').forEach((btn) => {
    btn.onclick = () => handleAction(btn.dataset.act, btn.dataset.key);
  });
}

async function handleAction(act, key) {
  if (act === 'del') {
    if (!confirm(`Remove ${key}?`)) return;
    await del(key);
  } else if (act === 'edit') {
    const v = prompt(`New value for ${key}:\n(leave empty to cancel)`);
    if (v === null || v === '') return;
    await set(key, v);
  }
}

async function set(key, value) {
  try {
    const r = await fetch('/api/credentials', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ key, value }),
    });
    const j = await r.json();
    if (!j.ok) throw new Error(j.error || 'unknown error');
    flash(`${key} saved — restart Aurifer for the change to take effect.`);
    load();
  } catch (e) {
    showError(e.message);
  }
}

async function del(key) {
  try {
    const r = await fetch('/api/credentials/' + encodeURIComponent(key), {
      method: 'DELETE',
    });
    const j = await r.json();
    if (!j.ok) throw new Error(j.error || 'unknown error');
    flash(`${key} removed — restart Aurifer for the change to take effect.`);
    load();
  } catch (e) {
    showError(e.message);
  }
}

$('add-form').onsubmit = async (ev) => {
  ev.preventDefault();
  const key = $('new-key').value.trim().toUpperCase();
  const value = $('new-value').value;
  if (!key || !value) return;
  await set(key, value);
  $('new-key').value = '';
  $('new-value').value = '';
};

function flash(msg) {
  const el = $('flash');
  el.textContent = msg;
  el.className = 'flash ok';
  clearTimeout(flash._t);
  flash._t = setTimeout(() => { el.className = 'flash'; }, 8000);
}

function showError(msg) {
  const el = $('flash');
  el.textContent = 'Error: ' + msg;
  el.className = 'flash err';
}

function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) =>
    ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}

load();