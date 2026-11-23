/* global OTP, IssuerIcons */
'use strict';

function escapeHtml(value) {
  return String(value).replace(/[&<>"']/g, c => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
  })[c]);
}

// Render the icon HTML for an entry: brand SVG when we have one, otherwise
// the legacy initial-letter circle. Returns a string (innerHTML-friendly).
function entryIconHTML(entry) {
  const issuer = entry.issuer || entry.account || '';
  const icon = (window.IssuerIcons && window.IssuerIcons.iconForIssuer)
    ? window.IssuerIcons.iconForIssuer(issuer)
    : null;
  if (icon) {
    return `<svg class="account-icon-svg" viewBox="0 0 24 24" style="color:${icon.color}" aria-hidden="true"><use href="#${icon.id}"/></svg>`;
  }
  const initial = (issuer || '?')[0].toUpperCase();
  return `<div class="account-icon">${escapeHtml(initial)}</div>`;
}

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

// ── State ──────────────────────────────────────────────────────────────────
let entries = [];
let filteredEntries = [];
let selectedId = null;
let tickInterval = null;
let fromView = 'view-list'; // view to return to when pressing Back

let sessionUnlocked = false;

// ── DOM refs ───────────────────────────────────────────────────────────────
const $ = id => document.getElementById(id);

function bufToBase64(buf) {
  let binary = '';
  const bytes = new Uint8Array(buf);
  for (let i = 0; i < bytes.length; i++) binary += String.fromCharCode(bytes[i]);
  return btoa(binary);
}

// ── Theme ──────────────────────────────────────────────────────────────────
const mq = window.matchMedia('(prefers-color-scheme: light)');

function applyTheme(theme) {
  const isLight = theme === 'light' || (theme === 'system' && mq.matches);
  document.documentElement.setAttribute('data-theme', isLight ? 'light' : 'dark');
}

mq.addEventListener('change', () => {
  const current = document.documentElement.getAttribute('data-theme-pref') || 'system';
  if (current === 'system') applyTheme('system');
});

// ── View router ────────────────────────────────────────────────────────────
function setLogoEye(open) {
  const symbol = open ? '#tofa-open' : '#tofa-wink';
  const animClass = open ? 'eye-opening' : 'eye-closing';
  const removeClass = open ? 'eye-closing' : 'eye-opening';
  const logos = ['logo-hero', 'logo-header-locked', 'logo-header-list'];
  logos.forEach(id => {
    const el = $(id);
    if (!el) return;
    const use = el.querySelector('use');
    if (!use) return;
    if (use.getAttribute('href') === symbol) return;
    use.setAttribute('href', symbol);
    el.classList.remove(removeClass, 'eye-opening', 'eye-closing');
    el.classList.add(animClass);
    el.addEventListener('animationend', () => el.classList.remove(animClass), { once: true });
  });
}

function showView(id) {
  document.querySelectorAll('.view').forEach(v => v.classList.remove('active'));
  $(id).classList.add('active');
  if (id === 'view-locked') setLogoEye(false);
}

function currentView() {
  const el = document.querySelector('.view.active');
  return el ? el.id : 'view-list';
}

// ── Loader ─────────────────────────────────────────────────────────────────
function loaderStart() { $('loader-bar-inner').style.width = '60%'; }
function loaderDone() {
  $('loader-bar-inner').style.width = '100%';
  setTimeout(() => { $('loader-bar-inner').style.width = '0%'; }, 300);
}

// ── Popover pinning ────────────────────────────────────────────────────────
// The Tauri popover hides on focus loss by default. For operations that
// legitimately steal focus (file picker, screen scan, camera scan, native
// folder dialog), pin the popover open while the operation runs so the
// user doesn't lose their place.
async function withPopoverPinned(fn) {
  try { await invoke('set_popover_pinned', { pinned: true }); } catch (_) {}
  try {
    return await fn();
  } finally {
    try { await invoke('set_popover_pinned', { pinned: false }); } catch (_) {}
  }
}

// Blocking overlay — high-visibility feedback for operations long enough
// that the user needs to know something is happening (unlock, scan,
// import). The thin loader bar is a secondary indicator.
function showBlocking(message) {
  const msg = $('blocking-overlay-message');
  if (msg) msg.textContent = message;
  $('blocking-overlay').classList.add('visible');
}
function hideBlocking() {
  $('blocking-overlay').classList.remove('visible');
}

// ── Toast ──────────────────────────────────────────────────────────────────
let toastTimer;
function toast(msg, error = false) {
  const el = $('toast');
  el.textContent = msg;
  el.className = 'toast visible' + (error ? ' error' : '');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { el.className = 'toast'; }, 2500);
}

// ── Init ───────────────────────────────────────────────────────────────────
async function init() {
  try {
    const s = await invoke('get_settings');
    applyTheme(s.theme || 'system');
    document.documentElement.setAttribute('data-theme-pref', s.theme || 'system');
  } catch (_) {
    applyTheme('system');
  }
  const exists = await invoke('vault_exists');
  $('confirm-wrap').style.display = exists ? 'none' : 'block';
  $('btn-unlock-submit').textContent = exists ? '解锁' : '创建保险库';
  showView('view-locked');
  $('input-passphrase').focus();
}

// ── Render list ────────────────────────────────────────────────────────────
function renderList(data) {
  entries = data;
  applyFilter($('search-input').value);
}

function applyFilter(query) {
  const q = query.toLowerCase();
  filteredEntries = q
    ? entries.filter(e =>
        e.name.toLowerCase().includes(q) ||
        e.issuer.toLowerCase().includes(q) ||
        e.account.toLowerCase().includes(q))
    : entries.slice();

  const list = $('account-list');
  list.innerHTML = '';

  if (filteredEntries.length === 0) {
    const empty = document.createElement('div');
    empty.className = 'empty-state';
    if (q) {
      empty.innerHTML = `<p class="empty-state-title">无匹配结果</p><p class="empty-state-sub">没有匹配的账号：<em>${escapeHtml(q)}</em></p>`;
    } else {
      empty.innerHTML = `
        <svg width="40" height="40" style="color:var(--text-muted);margin-bottom:var(--s-3)" viewBox="0 0 128 128"><use href="#tofa-wink"/></svg>
        <p class="empty-state-title">暂无账号</p>
        <p class="empty-state-sub">添加一个 TOTP 账号</p>
        <button class="btn btn-primary empty-state-btn" id="empty-state-add">添加账号</button>`;
    }
    list.appendChild(empty);
    if (!q) {
      empty.querySelector('#empty-state-add').addEventListener('click', () => {
        fromView = 'view-list';
        showView('view-add');
      });
    }
    return;
  }

  filteredEntries.forEach(entry => {
    const item = document.createElement('div');
    item.className = 'account-item';
    item.dataset.id = entry.id;
    const secs = entry.seconds_left ?? OTP.secondsRemaining(entry.period);
    const timerColor = secs < 5 ? 'var(--danger)' : secs < 10 ? 'var(--warning)' : 'var(--brand)';
    item.innerHTML = `
      ${entryIconHTML(entry)}
      <div style="flex:1;min-width:0;overflow:hidden;">
        <div class="account-name" style="white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">${escapeHtml(entry.issuer || entry.name)}</div>
        <div class="account-login" style="white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">${escapeHtml(entry.account)}</div>
      </div>
      <div style="display:flex;align-items:center;gap:var(--s-2);flex-shrink:0;padding-left:var(--s-2);">
        <div class="item-code-col" style="text-align:right;cursor:pointer;" title="点击复制验证码">
          <div style="font-family:var(--font-mono);font-weight:700;font-size:14px;letter-spacing:0.08em;color:${timerColor}">${entry.code}</div>
          <div style="font-family:var(--font-mono);font-size:10px;color:${timerColor}" data-timer="${escapeHtml(entry.id)}">${secs}s</div>
        </div>
        <button class="btn btn-ghost btn-copy-item" data-id="${escapeHtml(entry.id)}" style="padding:4px 6px;font-size:14px;flex-shrink:0;" title="复制验证码">⎘</button>
      </div>`;
    item.addEventListener('click', e => {
      if (e.target.closest('.btn-copy-item') || e.target.closest('.item-code-col')) return;
      openDetail(entry.id);
    });
    item.querySelector('.btn-copy-item').addEventListener('click', async e => {
      e.stopPropagation();
      try {
        await invoke('copy_code', { id: entry.id });
        toast('已复制');
      } catch (err) { toast(String(err), true); }
    });
    item.querySelector('.item-code-col').addEventListener('click', async e => {
      e.stopPropagation();
      try {
        await invoke('copy_code', { id: entry.id });
        toast('已复制');
      } catch (err) { toast(String(err), true); }
    });
    list.appendChild(item);
  });

}

// ── Tick (countdown) ───────────────────────────────────────────────────────
function startTick() {
  stopTick();
  tick();
  tickInterval = setInterval(tick, 1000);
}

function stopTick() {
  if (tickInterval) { clearInterval(tickInterval); tickInterval = null; }
}

function tick() {
  const secs = OTP.secondsRemaining();
  const globalBar = $('global-progress-bar');
  const pct = (secs / 30) * 100;
  globalBar.style.setProperty('--progress', `${pct}%`);
  globalBar.style.background = secs < 5 ? 'var(--danger)' : secs < 10 ? 'var(--warning)' : 'var(--brand)';

  // Update per-item timers in the list, using each entry's own period
  // (a 60s TOTP entry must count down from 60, not 30).
  document.querySelectorAll('[data-timer]').forEach(el => {
    const entry = entries.find(e => e.id === el.dataset.timer);
    const period = entry?.period || 30;
    const s = OTP.secondsRemaining(period);
    const color = s < 5 ? 'var(--danger)' : s < 10 ? 'var(--warning)' : 'var(--brand)';
    el.textContent = `${s}s`;
    el.style.color = color;
    const codeEl = el.previousElementSibling;
    if (codeEl) codeEl.style.color = color;
  });

  if (secs >= 30) { refreshEntries(); return; }

  if (selectedId) {
    const entry = entries.find(e => e.id === selectedId);
    if (entry) {
      const period = entry.period || 30;
      const s = OTP.secondsRemaining(period);
      $('detail-seconds').textContent = `${s} 秒后刷新`;
      const bar = $('detail-progress-bar');
      bar.style.setProperty('--progress', `${(s / period) * 100}%`);
      bar.style.background = s < 5 ? 'var(--danger)' : s < 10 ? 'var(--warning)' : 'var(--brand)';
      $('detail-code').style.color = s < 5 ? 'var(--danger)' : s < 10 ? 'var(--warning)' : 'var(--brand)';
    }
  }
}

async function refreshEntries() {
  try {
    const data = await invoke('get_entries');
    renderList(data);
    if (selectedId) {
      const entry = data.find(e => e.id === selectedId);
      if (entry) updateDetailCode(entry);
    }
  } catch (_) {}
}

// ── Detail view ────────────────────────────────────────────────────────────
function openDetail(id) {
  const entry = entries.find(e => e.id === id);
  if (!entry) return;
  selectedId = id;
  $('detail-title').textContent = entry.issuer || entry.name;
  // Replace the detail icon container with our themed icon (SVG or initial).
  $('detail-icon').outerHTML =
    `<div id="detail-icon" class="account-icon-detail">${entryIconHTML(entry)}</div>`;
  updateDetailCode(entry);
  renderDetailMeta(entry);
  $('reveal-overlay').style.display = 'none';
  showView('view-detail');
}

function renderDetailMeta(entry) {
  // Order goes identifiers first, then technical params, then dates,
  // then sensitive fields at the bottom (sensitive last so the user's
  // eye lands on plain metadata before the bullets).
  const rows = [
    ['ID',        entry.id || '—'],
    ['服务商',    entry.issuer  || '—'],
    ['账号',   entry.account || '—'],
    ['算法', entry.algorithm || 'SHA1'],
    ['位数',    String(entry.digits || 6)],
    ['周期',    `${entry.period || 30}s`],
    ['添加时间',     entry.created_at || '—'],
    ['密钥',    null],
    ['URI',       null],
  ];
  const tbody = $('detail-meta-body');
  tbody.innerHTML = '';
  rows.forEach(([label, value]) => {
    const tr = document.createElement('tr');
    if (label === '密钥') {
      tr.innerHTML = `<td>${label}</td><td><span class="secret-masked" id="secret-cell" title="点击显示">●●●●●●●●●●●●●●●●</span></td>`;
      tbody.appendChild(tr);
      $('secret-cell').addEventListener('click', () => {
        $('reveal-passphrase').value = '';
        $('reveal-error').style.display = 'none';
        $('reveal-overlay').style.display = 'flex';
        $('reveal-passphrase').focus();
      });
    } else if (label === 'URI') {
      // Same trust boundary as the secret row — bullets by default,
      // unmasked by the same reveal-overlay passphrase prompt. The
      // backend hands us a pre-masked URI so we don't need the secret
      // to render the resting state.
      tr.innerHTML = `<td>${label}</td>` +
        `<td><span class="secret-masked" id="uri-cell" ` +
        `style="word-break:break-all;font-family:var(--font-mono);font-size:11px;" ` +
        `title="点击显示">加载中…</span></td>`;
      tbody.appendChild(tr);
      invoke('get_masked_uri', { id: entry.id })
        .then(uri => { $('uri-cell').textContent = uri; })
        .catch(_ => { $('uri-cell').textContent = '（不可用）'; });
      $('uri-cell').addEventListener('click', () => {
        $('reveal-passphrase').value = '';
        $('reveal-error').style.display = 'none';
        $('reveal-overlay').style.display = 'flex';
        $('reveal-passphrase').focus();
      });
    } else {
      tr.innerHTML = `<td>${label}</td><td>${escapeHtml(value)}</td>`;
      tbody.appendChild(tr);
    }
  });
}

function updateDetailCode(entry) {
  $('detail-code').textContent = entry.code;
  const period = entry.period || 30;
  const secs = OTP.secondsRemaining(period);
  $('detail-seconds').textContent = `${secs} 秒后刷新`;
  const bar = $('detail-progress-bar');
  bar.style.setProperty('--progress', `${(secs / period) * 100}%`);
}

// ── Settings view ──────────────────────────────────────────────────────────
const ADD_INNER_HTML = (() => {
  const wrap = $('view-add-content');
  return wrap ? wrap.innerHTML : '';
})();

function restoreAddView() {
  const wrap = $('view-add-content');
  if (wrap) wrap.innerHTML = ADD_INNER_HTML;
  bindAddListeners();
}

async function openSettings(from) {
  fromView = from ?? currentView();
  let settings;
  try { settings = await invoke('get_settings'); } catch (_) {
    settings = { vault_path: '', theme: 'system' };
  }
  const theme = settings.theme || 'system';
  const wrap = $('view-add-content');
  if (!wrap) return;
  const approvedAccounts = sessionUnlocked
    ? await invoke('get_mcp_account_ids').catch(() => null)
    : null;
  const showMcpAccounts = sessionUnlocked && Array.isArray(approvedAccounts);

  wrap.innerHTML = `
    <div class="settings-section">
      <span class="settings-section-label">保险库路径</span>
      <div class="settings-row">
        <input id="settings-vault-path" class="input settings-path-input" placeholder="~/path/to/vault">
        <button id="btn-browse-vault" class="btn btn-secondary settings-browse-btn">浏览…</button>
      </div>
    </div>
    <div class="settings-section">
      <span class="settings-section-label">外观</span>
      <div class="settings-segmented">
        <button class="settings-seg-btn${theme==='light'?' active':''}" data-theme-btn="light">
          <svg class="seg-icon" width="12" height="12" viewBox="0 0 24 24"><use href="#icon-sun"/></svg>浅色
        </button>
        <button class="settings-seg-btn${theme==='dark'?' active':''}" data-theme-btn="dark">
          <svg class="seg-icon" width="12" height="12" viewBox="0 0 24 24"><use href="#icon-moon"/></svg>深色
        </button>
        <button class="settings-seg-btn${theme==='system'?' active':''}" data-theme-btn="system">
          <svg class="seg-icon" width="12" height="12" viewBox="0 0 24 24"><use href="#icon-monitor"/></svg>跟随系统
        </button>
      </div>
    </div>
    <div class="settings-section">
      <label class="settings-login-row" for="settings-login-item">
        <span>登录时启动</span>
        <input id="settings-login-item" type="checkbox" disabled aria-describedby="settings-login-status">
      </label>
      <p id="settings-login-status" class="settings-error" role="status" hidden></p>
    </div>
    ${showMcpAccounts ? `<div class="settings-section">
      <span class="settings-section-label">Codex 取码</span>
      <p class="settings-login-row">勾选本次解锁会话允许 Codex 取码的账号。</p>
      <div id="settings-mcp-accounts"></div>
    </div>` : ''}
    <button id="btn-settings-save" class="btn btn-primary settings-save-btn">保存</button>
    <p id="settings-error" class="settings-error"></p>`;

  $('settings-vault-path').value = settings.vault_path;

  if (showMcpAccounts) {
    const selected = new Set(approvedAccounts);
    const accounts = $('settings-mcp-accounts');
    for (const entry of entries) {
      const label = document.createElement('label');
      label.className = 'settings-login-row';
      const name = document.createElement('span');
      name.textContent = entry.name;
      const checkbox = document.createElement('input');
      checkbox.type = 'checkbox';
      checkbox.value = entry.id;
      checkbox.checked = selected.has(entry.id);
      checkbox.disabled = !entry.id;
      if (!entry.id) label.title = '此账号缺少稳定 ID，无法在 Codex 中授权';
      label.append(name, checkbox);
      accounts.append(label);
    }
  }

  const loginToggle = $('settings-login-item');
  const loginStatus = $('settings-login-status');
  function showLoginStatus(state, error = '') {
    loginToggle.checked = state === 'enabled' || state === 'approval_required';
    loginToggle.disabled = state === 'unavailable';
    loginStatus.textContent = error || (state === 'approval_required'
      ? '等待 macOS 在「登录项与扩展」中批准'
      : state === 'unavailable' ? '无法读取 macOS 登录项' : '');
    loginStatus.hidden = !loginStatus.textContent;
    loginStatus.style.display = loginStatus.hidden ? 'none' : 'block';
  }
  const readLoginStatus = () => invoke('get_login_item_status').catch(() => 'unavailable');
  readLoginStatus().then(showLoginStatus);
  loginToggle.addEventListener('change', async () => {
    const enabled = loginToggle.checked;
    loginToggle.disabled = true;
    try {
      const state = await withPopoverPinned(() => invoke('set_login_item_enabled', { enabled }));
      showLoginStatus(state);
    } catch (err) {
      showLoginStatus(await readLoginStatus(), String(err));
    }
  });

  // Theme segmented control
  let selectedTheme = theme;
  wrap.querySelectorAll('[data-theme-btn]').forEach(btn => {
    btn.addEventListener('click', () => {
      selectedTheme = btn.dataset.themeBtn;
      wrap.querySelectorAll('[data-theme-btn]').forEach(b => b.classList.toggle('active', b === btn));
      applyTheme(selectedTheme);
    });
  });

  $('btn-browse-vault').addEventListener('click', async () => {
    try {
      const picked = await withPopoverPinned(() => invoke('pick_vault_folder'));
      if (picked) $('settings-vault-path').value = picked;
    } catch (_) {}
  });

  $('btn-settings-save').addEventListener('click', async () => {
    const vault_path = $('settings-vault-path').value.trim();
    try {
      const vaultChanged = await invoke('save_settings', {
        settings: { vault_path, theme: selectedTheme },
        mcpAccountIds: showMcpAccounts
          ? [...wrap.querySelectorAll('#settings-mcp-accounts input:checked')].map(input => input.value)
          : null,
      });
      document.documentElement.setAttribute('data-theme-pref', selectedTheme);
      toast('设置已保存');
      if (!vaultChanged) {
        restoreAddView();
        showView(sessionUnlocked ? 'view-list' : 'view-locked');
      }
    } catch (err) {
      const errEl = $('settings-error');
      errEl.textContent = String(err);
      errEl.style.display = 'block';
    }
  });

  showView('view-add');
}

// ── Add view listeners ─────────────────────────────────────────────────────
function bindAddListeners() {
  const btnSubmit = $('btn-add-submit');
  if (btnSubmit) {
    btnSubmit.addEventListener('click', async () => {
      const uri = $('input-uri').value.trim();
      const errEl = $('add-error');
      errEl.style.display = 'none';
      if (!uri) { errEl.textContent = '请输入 otpauth:// URI。'; errEl.style.display = ''; return; }
      loaderStart();
      try {
        const added = await invoke('add_from_uri', { uri, name: '' });
        $('input-uri').value = '';
        const data = await invoke('get_entries');
        renderList(data);
        showView('view-list');
        toast(`已添加： ${added.join(', ')}`);
      } catch (err) {
        errEl.textContent = String(err);
        errEl.style.display = '';
      } finally { loaderDone(); }
    });
  }

  const btnScan = $('btn-scan-screen');
  if (btnScan) {
    btnScan.addEventListener('click', async () => {
      btnScan.disabled = true;
      loaderStart();
      showBlocking('正在扫描屏幕中的二维码…');
      try {
        const added = await withPopoverPinned(() => invoke('scan_screen'));
        if (added.length === 0) return;
        const data = await invoke('get_entries');
        renderList(data);
        showView('view-list');
        startTick();
        toast(`已添加 ${added.length} 个账号`);
      } catch (err) { toast(String(err), true); }
      finally {
        btnScan.disabled = false;
        loaderDone();
        hideBlocking();
      }
    });
  }

  const btnCam = $('btn-scan-camera');
  if (btnCam) {
    btnCam.addEventListener('click', async () => {
      toast('正在打开摄像头…');
      loaderStart();
      showBlocking('等待摄像头扫描…');
      try {
        const added = await withPopoverPinned(() => invoke('scan_camera'));
        const data = await invoke('get_entries');
        renderList(data);
        showView('view-list');
        startTick();
        toast(`已添加： ${added.join(', ')}`);
      } catch (err) { toast(String(err), true); }
      finally { loaderDone(); hideBlocking(); }
    });
  }

  const btnFile = $('btn-open-file');
  if (btnFile) {
    btnFile.addEventListener('click', async () => {
      loaderStart();
      showBlocking('正在导入文件…');
      try {
        const added = await withPopoverPinned(() => invoke('pick_and_import_file'));
        if (added.length === 0) return;
        const data = await invoke('get_entries');
        renderList(data);
        showView('view-list');
        // Many-account imports get a count + first-few preview;
        // single imports keep the original "已添加： <name>" form.
        if (added.length <= 3) {
          toast(`已添加： ${added.join(', ')}`);
        } else {
          toast(`已添加 ${added.length} 个账号 (${added.slice(0, 3).join(', ')}, …)`);
        }
      } catch (err) { toast(String(err), true); }
      finally { loaderDone(); hideBlocking(); }
    });
  }

}

// ── Static event listeners ─────────────────────────────────────────────────

$('form-unlock').addEventListener('submit', async e => {
  e.preventDefault();
  const passphrase = $('input-passphrase').value;
  const errEl = $('unlock-error');
  errEl.style.visibility = 'hidden';
  errEl.textContent = '';
  const btn = $('btn-unlock-submit');
  const originalBtnText = btn.textContent;

  // Vault check has to happen *before* we show the spinner so the message
  // matches the action (unlock vs create). It's a cheap check — pure file
  // exists() — so the user doesn't perceive the wait.
  let vaultExists;
  try { vaultExists = await invoke('vault_exists'); } catch (_) { vaultExists = true; }

  // Disable the button + flip its label so it's obvious the click landed.
  // Argon2id KDF in debug builds can run several seconds; without this
  // affordance the UI feels frozen.
  btn.disabled = true;
  btn.textContent = vaultExists ? '正在解锁…' : '正在创建…';
  loaderStart();
  setLogoEye(true);
  showBlocking(vaultExists ? '正在解密保险库…' : '正在创建保险库…');

  try {
    let data;
    if (vaultExists) {
      data = await invoke('unlock', { passphrase });
    } else {
      const confirm = $('input-passphrase-confirm').value;
      if (passphrase !== confirm) {
        errEl.textContent = '两次输入的口令不一致。';
        errEl.style.visibility = 'visible';
        setLogoEye(false);
        return;
      }
      data = await invoke('create_vault', { passphrase });
    }
    renderList(data);
    sessionUnlocked = true;
    startTick();
    showView('view-list');
  } catch (err) {
    errEl.textContent = String(err);
    errEl.style.visibility = 'visible';
    setLogoEye(false); // wrong passphrase — close the eye
  } finally {
    loaderDone();
    hideBlocking();
    btn.disabled = false;
    btn.textContent = originalBtnText;
    $('input-passphrase').value = '';
    $('input-passphrase-confirm').value = '';
  }
});

$('btn-add').addEventListener('click', () => {
  fromView = 'view-list';
  restoreAddView();
  showView('view-add');
});

$('btn-settings').addEventListener('click', () => openSettings());
$('btn-settings-locked').addEventListener('click', () => openSettings());

$('btn-detail-back').addEventListener('click', () => {
  selectedId = null;
  showView('view-list');
});

$('btn-add-back').addEventListener('click', () => {
  restoreAddView();
  showView(fromView);
});

$('btn-detail-copy').addEventListener('click', async () => {
  if (!selectedId) return;
  try {
    await invoke('copy_code', { id: selectedId });
    toast('已复制');
  } catch (err) { toast(String(err), true); }
});

$('btn-detail-copy-uri').addEventListener('click', async () => {
  if (!selectedId) return;
  try {
    await invoke('copy_uri', { id: selectedId });
    toast('URI 已复制');
  } catch (err) { toast(String(err), true); }
});

$('btn-detail-del').addEventListener('click', async () => {
  if (!selectedId) return;
  showBlocking('正在删除…');
  loaderStart();
  try {
    await invoke('delete_entry', { id: selectedId });
    selectedId = null;
    const data = await invoke('get_entries');
    renderList(data);
    showView('view-list');
    toast('已删除');
  } catch (err) { toast(String(err), true); }
  finally {
    hideBlocking();
    loaderDone();
  }
});

$('btn-detail-qr').addEventListener('click', async () => {
  if (!selectedId) return;
  loaderStart();
  try {
    const dataUri = await invoke('generate_entry_qr', { id: selectedId });
    $('qr-overlay-title').textContent = $('detail-title').textContent;
    $('qr-overlay-img').src = dataUri;
    $('qr-overlay').style.display = 'flex';
  } catch (err) { toast(String(err), true); }
  finally { loaderDone(); }
});

// ── QR list pagination state ────────────────────────────────────────────────
// When the user picks "List of QRs" multi-export, qrList holds the per-entry
// items and qrListIndex tracks which one is currently displayed in the
// shared qr-overlay. qrList is empty for single-QR (compatible) mode.
let qrList = [];
let qrListIndex = 0;

function showQrItem(idx) {
  if (qrList.length === 0) return;
  qrListIndex = Math.max(0, Math.min(idx, qrList.length - 1));
  const item = qrList[qrListIndex];
  $('qr-overlay-title').textContent = item.name;
  $('qr-overlay-img').src = item.data_uri;
  $('qr-overlay-counter').textContent = `${qrListIndex + 1} / ${qrList.length}`;
}

function resetQrOverlayToSingle() {
  qrList = [];
  qrListIndex = 0;
  $('qr-overlay-pager').style.display = 'none';
  $('btn-qr-save-all').style.display = 'none';
  $('btn-qr-save').style.display = '';
}

$('btn-qr-close').addEventListener('click', () => {
  $('qr-overlay').style.display = 'none';
  $('qr-overlay-img').src = '';
  resetQrOverlayToSingle();
});

$('btn-qr-save').addEventListener('click', async () => {
  const img = $('qr-overlay-img');
  if (!img.src) return;
  const filename = `${$('qr-overlay-title').textContent || 'tofa-qr'}.png`;
  try {
    await withPopoverPinned(() => invoke('save_qr_png', { dataUri: img.src, filename }));
  } catch (err) { toast(String(err), true); }
});

$('btn-qr-prev').addEventListener('click', () => showQrItem(qrListIndex - 1));
$('btn-qr-next').addEventListener('click', () => showQrItem(qrListIndex + 1));

$('btn-qr-save-all').addEventListener('click', async () => {
  if (qrList.length === 0) return;
  // One save dialog → one zip containing per-entry PNGs and a printable
  // one-pager (print.html). The backend handles zip composition.
  try {
    await withPopoverPinned(() => invoke('save_qr_zip', { items: qrList }));
  } catch (err) { toast(String(err), true); }
});

// ── Export QR (multi-select) ────────────────────────────────────────────────

$('btn-export-qr').addEventListener('click', () => {
  const list = $('export-qr-list');
  list.innerHTML = entries.map(e => {
    // Non-30s entries can't fit in a Google Authenticator migration QR;
    // mark them so the user knows the "Single QR" path will refuse the
    // selection and they should reach for "List of QRs" instead.
    const isNon30 = e.period && e.period !== 30;
    const badge = isNon30
      ? `<span title="${e.period} 秒周期与迁移二维码不兼容，请逐条导出" style="font-family:var(--font-mono);font-size:10px;color:var(--warning);border:1px solid var(--warning);border-radius:var(--r-sm);padding:1px 5px;margin-left:var(--s-2);">${e.period}s</span>`
      : '';
    const accountSpan = e.issuer
      ? `<span style="font-size:11px;color:var(--text-muted);margin-left:auto;">${escapeHtml(e.account)}</span>`
      : '';
    return `
    <label style="display:flex;align-items:center;gap:var(--s-3);padding:var(--s-2) var(--s-2);border-radius:var(--r-md);cursor:pointer;">
      <input type="checkbox" data-id="${escapeHtml(e.id)}" checked style="width:14px;height:14px;accent-color:var(--brand);">
      <span style="font-size:13px;">${escapeHtml(e.issuer || e.name)}</span>
      ${badge}
      ${accountSpan}
    </label>
  `;
  }).join('');
  $('export-qr-overlay').style.display = 'flex';
});

$('btn-export-qr-back').addEventListener('click', () => {
  $('export-qr-overlay').style.display = 'none';
});

$('btn-export-qr-generate').addEventListener('click', async () => {
  const ids = [...$('export-qr-list').querySelectorAll('input[type=checkbox]:checked')]
    .map(cb => cb.dataset.id);
  if (ids.length === 0) { toast('请至少选择一个账号', true); return; }
  loaderStart();
  try {
    const dataUri = await invoke('generate_selection_qr', { ids });
    resetQrOverlayToSingle();
    $('export-qr-overlay').style.display = 'none';
    $('qr-overlay-title').textContent = `${ids.length} 个账号`;
    $('qr-overlay-img').src = dataUri;
    $('qr-overlay').style.display = 'flex';
  } catch (err) { toast(String(err), true); }
  finally { loaderDone(); }
});

$('btn-export-qr-generate-multi').addEventListener('click', async () => {
  const ids = [...$('export-qr-list').querySelectorAll('input[type=checkbox]:checked')]
    .map(cb => cb.dataset.id);
  if (ids.length === 0) { toast('请至少选择一个账号', true); return; }
  loaderStart();
  try {
    const items = await invoke('generate_otpauth_list', { ids });
    qrList = items;
    qrListIndex = 0;
    $('export-qr-overlay').style.display = 'none';
    $('qr-overlay-pager').style.display = 'flex';
    $('btn-qr-save-all').style.display = '';
    $('btn-qr-save').style.display = 'none';
    showQrItem(0);
    $('qr-overlay').style.display = 'flex';
  } catch (err) { toast(String(err), true); }
  finally { loaderDone(); }
});

// Save selected entries as a plain-text URI list. Round-trips through
// `tofa import <file>.txt` and the desktop app's drop handler. Useful
// for moving accounts between authenticators when the receiving app
// can't read a Google-Authenticator migration QR.
$('btn-export-uri-list').addEventListener('click', async () => {
  const ids = [...$('export-qr-list').querySelectorAll('input[type=checkbox]:checked')]
    .map(cb => cb.dataset.id);
  if (ids.length === 0) { toast('请至少选择一个账号', true); return; }
  try {
    await withPopoverPinned(() => invoke('save_uri_list', { ids }));
    $('export-qr-overlay').style.display = 'none';
    toast(`已保存 ${ids.length} 条 URI`);
  } catch (err) { toast(String(err), true); }
});

$('btn-reveal-cancel').addEventListener('click', () => {
  $('reveal-overlay').style.display = 'none';
  $('reveal-passphrase').value = '';
});

$('btn-reveal-confirm').addEventListener('click', async () => {
  const passphrase = $('reveal-passphrase').value;
  const errEl = $('reveal-error');
  errEl.style.display = 'none';
  try {
    // get_full_uri also revalidates the passphrase server-side, so
    // wrong-passphrase paths surface here before we touch any UI.
    const [secret, fullUri] = await Promise.all([
      invoke('get_secret', { id: selectedId, passphrase }),
      invoke('get_full_uri', { id: selectedId, passphrase }),
    ]);
    $('reveal-overlay').style.display = 'none';
    $('reveal-passphrase').value = '';
    // Show secret + URI in their cells, re-mask both after 30s.
    const cell = $('secret-cell');
    cell.textContent = secret;
    cell.className = '';
    cell.style.color = 'var(--brand)';
    const uriCell = $('uri-cell');
    if (uriCell) {
      uriCell.dataset.masked = uriCell.textContent;
      uriCell.textContent = fullUri;
      uriCell.className = '';
      uriCell.style.color = 'var(--brand)';
    }
    setTimeout(() => {
      cell.textContent = '●●●●●●●●●●●●●●●●';
      cell.className = 'secret-masked';
      cell.style.color = '';
      if (uriCell && uriCell.dataset.masked) {
        uriCell.textContent = uriCell.dataset.masked;
        uriCell.className = 'secret-masked';
        uriCell.style.color = '';
      }
    }, 30000);
  } catch (err) {
    errEl.textContent = String(err);
    errEl.style.display = '';
  }
});

$('reveal-passphrase').addEventListener('keydown', e => {
  if (e.key === 'Enter') $('btn-reveal-confirm').click();
  if (e.key === 'Escape') $('btn-reveal-cancel').click();
});

$('search-input').addEventListener('input', e => applyFilter(e.target.value));

// 切换保险库后，需要重新解锁。
function onSessionLocked() {
  stopTick();
  sessionUnlocked = false;
  entries = [];
  filteredEntries = [];
  selectedId = null;
  fromView = 'view-list';
  init();
}
listen('session-locked', onSessionLocked);

// Tray menu events
listen('tray-action', ({ payload }) => {
  if (payload === 'mfa') {
    $('btn-qr-close').click();
    $('export-qr-overlay').style.display = 'none';
    $('reveal-overlay').style.display = 'none';
    $('reveal-passphrase').value = '';
    selectedId = null;
    fromView = 'view-list';
    restoreAddView();
    showView(sessionUnlocked ? 'view-list' : 'view-locked');
  } else if (payload === 'settings') {
    openSettings();
  } else if (payload === 'scan-screen') {
    $('btn-add').click();
    $('btn-scan-screen').click();
  } else if (payload === 'scan-camera') {
    const btnCam = $('btn-scan-camera');
    if (btnCam) btnCam.click();
  }
});

// Keyboard shortcuts
document.addEventListener('keydown', e => {
  if ((e.metaKey || e.ctrlKey) && e.key === 'n') { e.preventDefault(); $('btn-add').click(); }
  if ((e.metaKey || e.ctrlKey) && e.key === 'k') { e.preventDefault(); $('search-input').focus(); }
  if (e.key === 'Escape' && selectedId) { $('btn-detail-back').click(); }
});


// Scan step progress (emitted by scan_screen command)
listen('scan-step', ({ payload }) => { toast(String(payload)); });

// ── Boot ───────────────────────────────────────────────────────────────────
bindAddListeners();
init();
