(() => {
  'use strict';

  // ------------------------------------------------------------------ utils

  const $ = (selector, root = document) => root.querySelector(selector);
  const $$ = (selector, root = document) => Array.from(root.querySelectorAll(selector));

  const ESCAPES = {
    '&': '&amp;',
    '<': '&lt;',
    '>': '&gt;',
    '"': '&quot;',
    "'": '&#39;',
  };
  const esc = (value) =>
    String(value === null || value === undefined ? '' : value).replace(
      /[&<>"']/g,
      (char) => ESCAPES[char],
    );

  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const num = (value) => Number(value || 0).toLocaleString();
  const pct = (value) => `${Math.round(Number(value || 0) * 100)}%`;
  const initials = (name) =>
    String(name || '?')
      .split(/\s+/)
      .filter(Boolean)
      .slice(0, 2)
      .map((part) => part[0].toUpperCase())
      .join('');

  const CATEGORIES = ['issue', 'log', 'raw_event'];
  const catLabel = (category) =>
    String(category || '').replace('_', ' ').replace(/\b\w/g, (c) => c.toUpperCase());

  function relative(iso) {
    if (!iso) return '—';
    const then = new Date(iso).getTime();
    if (Number.isNaN(then)) return '—';
    const seconds = Math.max(0, Math.floor((Date.now() - then) / 1000));
    if (seconds < 5) return 'just now';
    if (seconds < 60) return `${seconds}s ago`;
    const minutes = Math.floor(seconds / 60);
    if (minutes < 60) return `${minutes}m ago`;
    const hours = Math.floor(minutes / 60);
    if (hours < 24) return `${hours}h ago`;
    const days = Math.floor(hours / 24);
    if (days < 30) return `${days}d ago`;
    return new Date(iso).toLocaleDateString();
  }

  function fullTime(iso) {
    if (!iso) return '—';
    const date = new Date(iso);
    if (Number.isNaN(date.getTime())) return '—';
    return date.toLocaleString(undefined, {
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
    });
  }

  function shortTime(iso) {
    if (!iso) return '—';
    const date = new Date(iso);
    if (Number.isNaN(date.getTime())) return '—';
    return date.toLocaleTimeString(undefined, {
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
    });
  }

  function levelBadge(level) {
    const known = ['error', 'fatal', 'warning', 'info', 'debug'].includes(level)
      ? level
      : 'debug';
    return `<span class="badge ${known}">${esc(level || 'info')}</span>`;
  }

  function option(value, label, current) {
    return `<option value="${esc(value)}" ${
      String(value) === String(current) ? 'selected' : ''
    }>${esc(label)}</option>`;
  }

  function randomHex(bytes) {
    const buffer = new Uint8Array(bytes);
    crypto.getRandomValues(buffer);
    return Array.from(buffer, (b) => b.toString(16).padStart(2, '0')).join('');
  }

  // ------------------------------------------------------------------ state

  const state = {
    token: localStorage.getItem('scout_token') || '',
    me: null,
    accounts: [],
    projects: [],
    projectId: localStorage.getItem('scout_project') || '',
    view: 'issues',
    issueId: null,
    issues: [],
    logs: [],
    filters: { q: '', state: 'open', level: '', sort: 'last_seen' },
    logFilters: { q: '', level: '' },
    searchQuery: '',
    issue: null,
    settings: null,
    members: [],
    outage: false,
  };

  const project = () => state.projects.find((p) => p.id === state.projectId) || null;
  const role = () => project()?.role || 'viewer';
  const canManage = () => ['admin', 'owner'].includes(role());
  const canTriage = () => ['triager', 'admin', 'owner'].includes(role());
  const canSend = () => canManage() && Boolean(project()?.key);

  // ------------------------------------------------------------------ api

  function handleSessionExpired() {
    state.token = '';
    state.me = null;
    localStorage.removeItem('scout_token');
    showLogin();
    toast('Your session expired. Sign in again.', 'error');
  }

  async function api(path, options = {}) {
    const { method = 'GET', body, auth = true, raw = false } = options;
    const headers = {};
    if (body !== undefined) headers['Content-Type'] = 'application/json';
    if (auth && state.token) headers.Authorization = `Bearer ${state.token}`;

    const response = await fetch(path, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
    });

    if (raw) {
      if (!response.ok) {
        const text = await response.text();
        let message = `Request failed (${response.status})`;
        try {
          message = JSON.parse(text).error || message;
        } catch (_) {
          /* keep default */
        }
        if (response.status === 401 && auth) handleSessionExpired();
        throw new Error(message);
      }
      return response.text();
    }

    const text = await response.text();
    let data = null;
    if (text) {
      try {
        data = JSON.parse(text);
      } catch (_) {
        data = text;
      }
    }

    if (!response.ok) {
      if (response.status === 401 && auth) handleSessionExpired();
      const message =
        data && typeof data === 'object' && data.error
          ? data.error
          : typeof data === 'string' && data
            ? data
            : `Request failed (${response.status})`;
      throw new Error(message);
    }
    return data;
  }

  // ------------------------------------------------------------------ toasts

  function toast(message, kind = 'info') {
    const root = $('#toasts');
    if (!root) return;
    const el = document.createElement('div');
    el.className = `toast ${kind}`;
    el.innerHTML = `<span class="dot"></span><span>${esc(message)}</span>`;
    root.appendChild(el);
    setTimeout(() => {
      el.style.opacity = '0';
      el.style.transform = 'translateY(5px)';
      el.style.transition = 'all .25s ease';
      setTimeout(() => el.remove(), 260);
    }, 4200);
  }

  // ------------------------------------------------------------------ modal

  function escClose(event) {
    if (event.key === 'Escape') closeModal();
  }

  function openModal(html, onMount, wide = false) {
    const root = $('#modal-root');
    root.innerHTML = `<div class="modal-backdrop"><div class="modal ${
      wide ? 'wide' : ''
    }">${html}</div></div>`;
    const backdrop = root.querySelector('.modal-backdrop');
    backdrop.addEventListener('click', (event) => {
      if (event.target === backdrop) closeModal();
    });
    root.querySelectorAll('[data-close]').forEach((button) => {
      button.addEventListener('click', closeModal);
    });
    document.addEventListener('keydown', escClose);
    if (onMount) onMount(root);
  }

  function closeModal() {
    const root = $('#modal-root');
    if (root) root.innerHTML = '';
    document.removeEventListener('keydown', escClose);
  }

  // ------------------------------------------------------------------ boot

  async function boot() {
    bindShell();
    await loadAccounts();

    if (state.token) {
      try {
        const data = await api('/api/me');
        state.me = data.user;
        await loadProjects();
        showShell();
        await getOutage();
        handleHash();
        return;
      } catch (_) {
        state.token = '';
        localStorage.removeItem('scout_token');
      }
    }

    showLogin();
  }

  async function loadAccounts() {
    try {
      const data = await api('/api/demo/accounts', { auth: false });
      state.accounts = data.accounts || [];
    } catch (_) {
      state.accounts = [];
    }
    if (!state.accounts.length) {
      state.accounts = [
        {
          name: 'Dana Demo',
          email: 'demo@scout.local',
          memberships: [
            { role: 'owner', project_name: 'scout-web' },
            { role: 'admin', project_name: 'platform-api' },
          ],
        },
        {
          name: 'Toni Triager',
          email: 'triager@scout.local',
          memberships: [{ role: 'triager', project_name: 'scout-web' }],
        },
        {
          name: 'Vic Viewer',
          email: 'viewer@scout.local',
          memberships: [{ role: 'viewer', project_name: 'scout-web' }],
        },
      ];
    }
  }

  function showLogin() {
    const shell = $('#shell');
    if (shell) shell.classList.add('hidden');
    const login = $('#login');
    if (!login) return;
    login.classList.remove('hidden');

    const cards = state.accounts
      .map((account) => {
        const roles =
          (account.memberships || [])
            .map(
              (membership) =>
                `<span class="badge ${esc(membership.role)}">${esc(
                  membership.role,
                )} · ${esc(membership.project_name)}</span>`,
            )
            .join('') || '<span class="muted small">no projects</span>';
        return `<button class="account-card" type="button" data-email="${esc(account.email)}">
          <span class="avatar">${esc(initials(account.name))}</span>
          <strong>${esc(account.name)}</strong>
          <span class="muted small">${esc(account.email)}</span>
          <span class="roles">${roles}</span>
          <span class="btn small primary" style="margin-top:auto">Sign in</span>
        </button>`;
      })
      .join('');

    login.innerHTML = `<div class="login-card">
      <div class="login-brand">
        <span class="brand-mark" aria-hidden="true">
          <svg viewBox="0 0 32 32" width="42" height="42">
            <rect width="32" height="32" rx="8" fill="currentColor" opacity="0.16" />
            <path d="M8 21l5.5-11 4.2 7.4 2.4-3.6L24 21" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round" />
          </svg>
        </span>
        <div>
          <h1>Scout</h1>
          <p class="muted">Self-hosted, Sentry-compatible error tracking &amp; logs</p>
        </div>
      </div>
      <p class="lede">Point an existing Sentry SDK at Scout — change only the DSN. Errors group into issues, logs sit beside them, and the decision model only ever suggests; a human confirms.</p>
      <h2 style="font-size:14px;margin-bottom:12px">Sign in as a demo account</h2>
      <div class="accounts">${cards}</div>
      <div class="login-note">Prototype: sessions are in-memory and seeded accounts have no passwords. Signing in scopes every request to the projects and role shown above. Restarting the server resets all data.</div>
    </div>`;

    $$('.account-card', login).forEach((button) => {
      button.addEventListener('click', () => loginAs(button.dataset.email));
    });
  }

  async function loginAs(email) {
    try {
      const data = await api('/api/login', {
        method: 'POST',
        body: { email },
        auth: false,
      });
      state.token = data.token;
      state.me = data.user;
      localStorage.setItem('scout_token', data.token);
      await loadProjects();
      $('#login').classList.add('hidden');
      showShell();
      await getOutage();
      if (!location.hash) location.hash = '#/issues';
      handleHash();
      toast(`Signed in as ${data.user.name}`, 'success');
    } catch (error) {
      toast(error.message, 'error');
    }
  }

  function showShell() {
    $('#shell').classList.remove('hidden');
    $('#login').classList.add('hidden');
    if (state.me) {
      $('#user-avatar').textContent = initials(state.me.name);
      $('#user-name').textContent = state.me.name;
      $('#user-email').textContent = state.me.email;
    }
    renderProjectSwitcher();
  }

  function logoutLocal() {
    state.token = '';
    state.me = null;
    state.issues = [];
    state.logs = [];
    state.issue = null;
    state.settings = null;
    localStorage.removeItem('scout_token');
    showLogin();
  }

  async function logout() {
    try {
      await api('/api/logout', { method: 'POST' });
    } catch (_) {
      /* session may already be gone */
    }
    logoutLocal();
  }

  function bindShell() {
    $('#logout').addEventListener('click', logout);
    $('#back-btn').addEventListener('click', () => navigate('issues'));
    $('#project-select').addEventListener('change', (event) => {
      state.projectId = event.target.value;
      localStorage.setItem('scout_project', state.projectId);
      renderProjectSwitcher();
      handleHash();
    });
    window.addEventListener('hashchange', handleHash);
  }

  async function loadProjects() {
    const data = await api('/api/projects');
    state.projects = data.projects || [];
    if (!state.projects.some((p) => p.id === state.projectId)) {
      state.projectId = state.projects[0]?.id || '';
    }
    localStorage.setItem('scout_project', state.projectId);
    renderProjectSwitcher();
  }

  async function refreshProjects() {
    try {
      const data = await api('/api/projects');
      state.projects = data.projects || [];
      renderProjectSwitcher();
    } catch (_) {
      /* keep the current view usable */
    }
  }

  function renderProjectSwitcher() {
    const select = $('#project-select');
    if (!select) return;
    select.innerHTML = state.projects
      .map(
        (p) =>
          `<option value="${esc(p.id)}" ${p.id === state.projectId ? 'selected' : ''}>${esc(
            p.name,
          )}</option>`,
      )
      .join('');
    const current = project();
    const roleLine = $('#project-role');
    if (roleLine) {
      roleLine.innerHTML = current
        ? `<span class="badge ${esc(current.role)}">${esc(current.role)}</span><span>${esc(
            current.org_name,
          )}</span>`
        : '<span class="muted">no project access</span>';
    }
    $('#nav-settings').classList.toggle('hidden', !canManage());
  }

  async function getOutage() {
    try {
      const response = await fetch('/mock-jev/outage');
      if (response.ok) {
        const data = await response.json();
        state.outage = Boolean(data.down);
      }
    } catch (_) {
      /* ignore */
    }
  }

  // ------------------------------------------------------------------ routing

  function navigate(view, id) {
    const hash = view === 'issue' ? `#/issues/${id}` : `#/${view}`;
    state.view = view;
    state.issueId = id || null;
    if (location.hash === hash) {
      updateNav();
      renderView();
    } else {
      location.hash = hash;
    }
  }

  function handleHash() {
    if (!state.projects.length) {
      renderView();
      return;
    }
    const raw = (location.hash || '').replace(/^#\/?/, '');
    const parts = raw.split('/').filter(Boolean);
    const view = parts[0] || 'issues';
    if (view === 'issues' && parts[1]) {
      state.view = 'issue';
      state.issueId = parts[1];
    } else if (['issues', 'logs', 'search', 'members', 'settings'].includes(view)) {
      state.view = view;
      state.issueId = null;
    } else {
      state.view = 'issues';
      state.issueId = null;
    }
    updateNav();
    renderView();
  }

  function updateNav() {
    const active = state.view === 'issue' ? 'issues' : state.view;
    $$('#nav a').forEach((link) => {
      link.classList.toggle('active', link.dataset.nav === active);
    });
    $('#back-btn').classList.toggle('hidden', state.view !== 'issue');
    $('#nav-settings').classList.toggle('hidden', !canManage());
  }

  function setTopbar(title, subtitle, actions = '') {
    $('#topbar-title').textContent = title;
    $('#topbar-sub').textContent = subtitle || '';
    $('#topbar-actions').innerHTML = actions;
  }

  function sendEventButton() {
    return canSend()
      ? '<button class="btn primary" id="send-event-btn" type="button">＋ <span class="label">Send test event</span></button>'
      : '';
  }

  function sendLogButton() {
    return canSend()
      ? '<button class="btn" id="send-log-btn" type="button">＋ <span class="label">Send test log</span></button>'
      : '';
  }

  function bindTopbarActions() {
    const sendEvent = $('#send-event-btn');
    if (sendEvent) sendEvent.addEventListener('click', openSendEventModal);
    const sendLog = $('#send-log-btn');
    if (sendLog) sendLog.addEventListener('click', openSendLogModal);
  }

  function renderView() {
    if (!state.projects.length) {
      setTopbar('No projects', '', '');
      $('#view').innerHTML = emptyState(
        'No project access',
        'This account is not a member of any project. Use a different demo account.',
        '',
      );
      return;
    }
    switch (state.view) {
      case 'issue':
        renderIssue(state.issueId);
        break;
      case 'logs':
        renderLogs();
        break;
      case 'search':
        renderSearch();
        break;
      case 'members':
        renderMembers();
        break;
      case 'settings':
        renderSettings();
        break;
      default:
        renderIssues();
    }
  }

  function emptyState(title, body, action) {
    return `<div class="panel empty"><strong>${esc(title)}</strong><span>${esc(
      body,
    )}</span>${action || ''}</div>`;
  }

  // ------------------------------------------------------------------ issues

  async function renderIssues() {
    const current = project();
    setTopbar(
      'Issues',
      `${current.name} · ${num(current.open_issues)} open`,
      sendEventButton(),
    );
    bindTopbarActions();

    const f = state.filters;
    $('#view').innerHTML = `
      <div class="toolbar">
        <input id="issue-q" class="input" placeholder="Search title, culprit, fingerprint…" value="${esc(
          f.q,
        )}" />
        <select id="issue-state" class="select">
          ${option('open', 'Open', f.state)}
          ${option('resolved', 'Resolved', f.state)}
          ${option('ignored', 'Ignored', f.state)}
          ${option('', 'All states', f.state)}
        </select>
        <select id="issue-level" class="select">
          ${option('', 'All levels', f.level)}
          ${option('error', 'Error', f.level)}
          ${option('fatal', 'Fatal', f.level)}
          ${option('warning', 'Warning', f.level)}
          ${option('info', 'Info', f.level)}
        </select>
        <select id="issue-sort" class="select">
          ${option('last_seen', 'Last seen', f.sort)}
          ${option('count', 'Occurrences', f.sort)}
          ${option('first_seen', 'First seen', f.sort)}
        </select>
        <span class="spacer"></span>
        <span class="muted small" id="issue-total"></span>
      </div>
      <div class="panel issue-list" id="issue-list">
        <div class="skeleton"></div><div class="skeleton"></div><div class="skeleton"></div>
      </div>`;

    let timer = null;
    $('#issue-q').addEventListener('input', (event) => {
      clearTimeout(timer);
      const value = event.target.value;
      timer = setTimeout(() => {
        state.filters.q = value;
        loadIssues();
      }, 220);
    });
    $('#issue-state').addEventListener('change', (event) => {
      state.filters.state = event.target.value;
      loadIssues();
    });
    $('#issue-level').addEventListener('change', (event) => {
      state.filters.level = event.target.value;
      loadIssues();
    });
    $('#issue-sort').addEventListener('change', (event) => {
      state.filters.sort = event.target.value;
      loadIssues();
    });

    await loadIssues();
  }

  async function loadIssues() {
    const current = project();
    if (!current) return;
    const f = state.filters;
    const params = new URLSearchParams({
      q: f.q,
      state: f.state,
      level: f.level,
      sort: f.sort,
    });
    try {
      const data = await api(`/api/projects/${current.id}/issues?${params.toString()}`);
      state.issues = data.issues || [];
      renderIssueList();
    } catch (error) {
      toast(error.message, 'error');
    }
  }

  function renderIssueList() {
    const list = $('#issue-list');
    if (!list) return;
    const total = $('#issue-total');
    if (total) {
      total.textContent = `${num(state.issues.length)} issue${state.issues.length === 1 ? '' : 's'}`;
    }
    if (!state.issues.length) {
      list.innerHTML = emptyState(
        'No issues match',
        'Try a different state, level, or search term. New errors appear here as soon as ingest is processed.',
        canSend()
          ? '<button class="btn primary" id="empty-send" type="button">Send a test error</button>'
          : '',
      );
      const emptySend = $('#empty-send');
      if (emptySend) emptySend.addEventListener('click', openSendEventModal);
      return;
    }
    list.innerHTML = state.issues.map(issueRow).join('');
  }

  function issueRow(issue) {
    let flag = '';
    if (issue.decision) {
      flag = `<span class="badge cat">categorised: ${esc(
        catLabel(issue.decision.category),
      )}</span>`;
    } else if (issue.suggestion) {
      flag = `<span class="badge suggest">suggested: ${esc(
        catLabel(issue.suggestion.category),
      )} · ${pct(issue.suggestion.confidence)}</span>`;
    }
    const stateBadge =
      issue.state !== 'open'
        ? `<span class="badge ${esc(issue.state)}">${esc(issue.state)}</span>`
        : '';
    return `<a class="issue-row" href="#/issues/${esc(issue.id)}">
      <span class="level-dot ${esc(issue.level)}"></span>
      <span class="issue-main">
        <span class="issue-title">${esc(issue.title)}</span>
        <span class="issue-culprit">${esc(issue.culprit || issue.fingerprint.slice(0, 20))}</span>
      </span>
      <span class="issue-meta"><span class="count">${num(
        issue.count,
      )}</span><span>${esc(relative(issue.last_seen))}</span></span>
      <span class="issue-flags">${stateBadge}${flag}</span>
    </a>`;
  }

  // ------------------------------------------------------------------ issue detail

  async function renderIssue(id) {
    if (!id) return navigate('issues');
    setTopbar('Issue', '', '');
    $('#view').innerHTML =
      '<div class="skeleton" style="height:150px"></div><div class="skeleton" style="height:120px;margin-top:16px"></div><div class="skeleton" style="height:320px;margin-top:16px"></div>';
    try {
      const data = await api(`/api/issues/${id}`);
      state.issue = data;
      renderIssueDetail(data);
    } catch (error) {
      setTopbar('Issue not found', '', '');
      $('#view').innerHTML = emptyState(
        'Issue not found',
        error.message,
        '<a class="btn" href="#/issues">Back to issues</a>',
      );
    }
  }

  function suggestionBlock(issue) {
    const actionable = canTriage();
    if (issue.decision) {
      return `<div class="suggestion decided">
        <span class="icon">✓</span>
        <div class="body">
          <strong>Categorised as ${esc(catLabel(issue.decision.category))}</strong>
          <div class="muted small">${issue.decision.overridden ? 'Override' : 'Confirmed'} by ${esc(
            issue.decision.by,
          )} · ${esc(fullTime(issue.decision.at))}${
            issue.suggestion
              ? ` · the model suggested ${esc(
                  catLabel(issue.suggestion.category),
                )} (${pct(issue.suggestion.confidence)})`
              : ''
          }</div>
        </div>
        ${actionable ? `<div class="actions">${overrideControls(issue)}</div>` : ''}
      </div>`;
    }
    if (issue.suggestion) {
      return `<div class="suggestion">
        <span class="icon">✦</span>
        <div class="body">
          <strong>Model suggests ${esc(catLabel(issue.suggestion.category))}</strong>
          <span class="confidence">${pct(issue.suggestion.confidence)} confidence · ${esc(
            issue.suggestion.source,
          )}</span>
          <div class="muted small">The item is already stored. Confirm or override — the model never decides silently.</div>
        </div>
        ${
          actionable
            ? `<div class="actions">
                <button class="btn primary small" data-confirm="1" type="button">Confirm ${esc(
                  catLabel(issue.suggestion.category),
                )}</button>
                ${overrideControls(issue)}
              </div>`
            : '<span class="badge">read only</span>'
        }
      </div>`;
    }
    return `<div class="suggestion none">
      <span class="icon">—</span>
      <div class="body">
        <strong>No suggestion</strong>
        <div class="muted small">The decision model is not configured, unreachable, or returned no usable answer. Ingest and storage were unaffected.</div>
      </div>
    </div>`;
  }

  function overrideControls(issue) {
    return `<select class="select" id="override-cat" style="width:auto">
      ${CATEGORIES.map((category) =>
        option(category, catLabel(category), issue.decision?.category || ''),
      ).join('')}
    </select>
    <button class="btn ghost small" data-override="1" type="button">Override</button>`;
  }

  function renderIssueDetail(data) {
    const issue = data.issue;
    const current = project();
    setTopbar(
      issue.title.length > 70 ? `${issue.title.slice(0, 70)}…` : issue.title,
      `${current.name} · ${issue.environment || 'unknown env'} · ${issue.release || 'no release'}`,
      '<a class="btn ghost small" href="#/issues">← All issues</a>',
    );

    const frames = issue.frames || [];
    const crumbs = issue.breadcrumbs || [];
    const tags = issue.tags && typeof issue.tags === 'object' ? issue.tags : {};
    const attachments = data.attachments || [];
    const stateAction =
      issue.state === 'resolved' ? 'Reopen issue' : 'Resolve issue';
    const stateValue = issue.state === 'resolved' ? 'open' : 'resolved';

    const stackHtml = frames.length
      ? frames
          .map(
            (frame) => `<div class="frame ${frame.in_app ? 'inapp' : ''}">
              <div class="frame-head">
                <span class="fn">${esc(frame.function || '<anonymous>')}</span>
                <span class="loc">${esc(frame.filename || '')}${
                  frame.lineno ? `:${frame.lineno}` : ''
                }</span>
                ${frame.in_app ? '<span class="tag-inapp">in app</span>' : ''}
              </div>
              ${
                frame.context_line
                  ? `<div class="ctx"><span class="ln">${esc(
                      frame.lineno || '',
                    )}</span>${esc(frame.context_line)}</div>`
                  : ''
              }
            </div>`,
          )
          .join('')
      : '<div class="muted small">No stack frames in this payload.</div>';

    const crumbsHtml = crumbs.length
      ? crumbs
          .map(
            (crumb) => `<div class="crumb">
              <span class="time">${esc(shortTime(crumb.timestamp))}</span>
              <span class="what">
                <span class="cat">${esc(crumb.category || 'event')}</span>
                <span class="msg">${esc(crumb.message || '')}</span>
                ${
                  crumb.data
                    ? `<div class="data">${esc(JSON.stringify(crumb.data))}</div>`
                    : ''
                }
              </span>
            </div>`,
          )
          .join('')
      : '<div class="muted small">No breadcrumbs in this payload.</div>';

    const tagsHtml = Object.keys(tags).length
      ? Object.entries(tags)
          .map(
            ([key, value]) =>
              `<span class="tag"><span class="k">${esc(key)}</span><span class="v">${esc(
                typeof value === 'object' ? JSON.stringify(value) : value,
              )}</span></span>`,
          )
          .join('')
      : '<span class="muted small">No tags.</span>';

    $('#view').innerHTML = `
      <section class="panel issue-hero">
        <div class="title-line">
          ${levelBadge(issue.level)}
          <h1>${esc(issue.title)}</h1>
          <span class="badge ${esc(issue.state)}">${esc(issue.state)}</span>
          <div class="hero-actions">
            ${
              canTriage()
                ? `<button class="btn small" id="state-btn" data-state="${stateValue}" type="button">${stateAction}</button>`
                : ''
            }
          </div>
        </div>
        <div class="subline">${esc(issue.culprit || 'no culprit')} · fingerprint ${esc(
          issue.fingerprint.slice(0, 16),
        )}</div>
        <div class="stat-row">
          <div class="stat"><span class="k">Occurrences</span><span class="v">${num(
            issue.count,
          )}</span></div>
          <div class="stat"><span class="k">First seen</span><span class="v" title="${esc(
            fullTime(issue.first_seen),
          )}">${esc(relative(issue.first_seen))}</span></div>
          <div class="stat"><span class="k">Last seen</span><span class="v" title="${esc(
            fullTime(issue.last_seen),
          )}">${esc(relative(issue.last_seen))}</span></div>
          <div class="stat"><span class="k">Release</span><span class="v">${esc(
            issue.release || '—',
          )}</span></div>
          <div class="stat"><span class="k">Environment</span><span class="v">${esc(
            issue.environment || '—',
          )}</span></div>
        </div>
      </section>

      ${suggestionBlock(issue)}

      <div class="detail-grid split">
        <section class="panel panel-pad">
          <div class="section-title"><h3>Stack trace</h3><span class="muted small">${
            frames.length
          } frame${frames.length === 1 ? '' : 's'} · crash site first</span></div>
          <div class="stack">${stackHtml}</div>
        </section>
        <section class="panel panel-pad">
          <div class="section-title"><h3>Breadcrumbs</h3><span class="muted small">${
            crumbs.length
          }</span></div>
          <div class="crumbs">${crumbsHtml}</div>
        </section>
      </div>

      <section class="panel panel-pad">
        <div class="section-title"><h3>Tags</h3></div>
        <div class="tags">${tagsHtml}</div>
      </section>

      <section class="panel panel-pad">
        <div class="section-title">
          <h3>Raw event</h3>
          <span class="muted small">${
            data.raw_available
              ? `${num(data.event?.size || 0)} bytes received · ${attachments.length} attachment(s)`
              : 'expired under retention'
          }</span>
        </div>
        ${
          data.raw_available
            ? `<div class="tabs">
                <button class="tab active" data-raw-tab="event" type="button">Event JSON</button>
                <button class="tab" data-raw-tab="envelope" type="button">Raw envelope</button>
                ${
                  attachments.length
                    ? `<button class="tab" data-raw-tab="attachments" type="button">Attachments (${attachments.length})</button>`
                    : ''
                }
              </div>
              <pre class="raw" id="raw-panel">${esc(
                JSON.stringify(data.event_json, null, 2),
              )}</pre>`
            : '<div class="empty"><strong>Raw payload expired</strong><span>The issue metadata is retained, but the original payload has passed this project\'s retention window.</span></div>'
        }
      </section>`;

    const stateBtn = $('#state-btn');
    if (stateBtn) {
      stateBtn.addEventListener('click', () => changeIssueState(issue.id, stateBtn.dataset.state));
    }
    $$('[data-confirm]').forEach((button) => {
      button.addEventListener('click', () => categorise(issue.id, 'confirm'));
    });
    $$('[data-override]').forEach((button) => {
      button.addEventListener('click', () => {
        const select = $('#override-cat');
        categorise(issue.id, 'override', select ? select.value : 'issue');
      });
    });

    $$('[data-raw-tab]').forEach((button) => {
      button.addEventListener('click', async () => {
        $$('[data-raw-tab]').forEach((b) => b.classList.remove('active'));
        button.classList.add('active');
        const panel = $('#raw-panel');
        if (!panel) return;
        const tab = button.dataset.rawTab;
        if (tab === 'event') {
          panel.textContent = JSON.stringify(data.event_json, null, 2);
        } else if (tab === 'envelope') {
          panel.textContent = 'Loading…';
          try {
            panel.textContent = await api(`/api/issues/${issue.id}/raw`, { raw: true });
          } catch (error) {
            panel.textContent = error.message;
          }
        } else {
          panel.textContent =
            attachments
              .map(
                (attachment) =>
                  `${attachment.filename}  (${attachment.content_type}, ${num(
                    attachment.size,
                  )} bytes)`,
              )
              .join('\n') || 'No attachments.';
        }
      });
    });
  }

  async function changeIssueState(id, nextState) {
    try {
      await api(`/api/issues/${id}/state`, {
        method: 'POST',
        body: { state: nextState },
      });
      toast(nextState === 'resolved' ? 'Issue resolved' : 'Issue reopened', 'success');
      await refreshProjects();
      renderIssue(id);
    } catch (error) {
      toast(error.message, 'error');
    }
  }

  async function categorise(id, action, category) {
    try {
      await api(`/api/issues/${id}/categorisation`, {
        method: 'POST',
        body: { action, category },
      });
      toast(action === 'confirm' ? 'Suggestion confirmed' : 'Categorisation overridden', 'success');
      renderIssue(id);
    } catch (error) {
      toast(error.message, 'error');
    }
  }

  // ------------------------------------------------------------------ logs

  async function renderLogs() {
    const current = project();
    setTopbar(
      'Logs',
      `${current.name} · searchable beside issues`,
      sendLogButton(),
    );
    bindTopbarActions();

    const f = state.logFilters;
    $('#view').innerHTML = `
      <div class="toolbar">
        <input id="log-q" class="input" placeholder="Search messages, sources, attributes…" value="${esc(
          f.q,
        )}" />
        <select id="log-level" class="select">
          ${option('', 'All levels', f.level)}
          ${option('error', 'Error', f.level)}
          ${option('warning', 'Warning', f.level)}
          ${option('info', 'Info', f.level)}
          ${option('debug', 'Debug', f.level)}
        </select>
        <span class="spacer"></span>
        <span class="muted small" id="log-total"></span>
      </div>
      <div class="panel" id="log-list"><div class="skeleton"></div><div class="skeleton"></div></div>`;

    let timer = null;
    $('#log-q').addEventListener('input', (event) => {
      clearTimeout(timer);
      const value = event.target.value;
      timer = setTimeout(() => {
        state.logFilters.q = value;
        loadLogs();
      }, 220);
    });
    $('#log-level').addEventListener('change', (event) => {
      state.logFilters.level = event.target.value;
      loadLogs();
    });

    await loadLogs();
  }

  async function loadLogs() {
    const current = project();
    if (!current) return;
    const params = new URLSearchParams({
      q: state.logFilters.q,
      level: state.logFilters.level,
      limit: '300',
    });
    try {
      const data = await api(`/api/projects/${current.id}/logs?${params.toString()}`);
      state.logs = data.logs || [];
      const total = $('#log-total');
      if (total) {
        total.textContent = `${num(state.logs.length)} line${state.logs.length === 1 ? '' : 's'}`;
      }
      const host = $('#log-list');
      if (!host) return;
      host.innerHTML = state.logs.length ? logTable(state.logs) : logEmpty();
    } catch (error) {
      toast(error.message, 'error');
    }
  }

  function logTable(logs) {
    return `<table class="table">
      <thead><tr><th style="width:110px">Time</th><th style="width:90px">Level</th><th>Message</th><th style="width:150px">Source</th><th style="width:220px">Attributes</th></tr></thead>
      <tbody>${logs
        .map(
          (log) => `<tr>
            <td class="time" title="${esc(fullTime(log.timestamp))}">${esc(
              shortTime(log.timestamp),
            )}</td>
            <td>${levelBadge(log.level)}</td>
            <td class="msg">${esc(log.message)}</td>
            <td class="src">${esc(log.source)}</td>
            <td class="attrs">${esc(
              log.attributes ? JSON.stringify(log.attributes) : '',
            )}</td>
          </tr>`,
        )
        .join('')}</tbody>
    </table>`;
  }

  function logEmpty() {
    return `<div class="empty"><strong>No log lines match</strong><span>Ingest structured logs through <code>/api/{project}/logs/</code> with the project key, or send a test log.</span>${
      canSend()
        ? '<button class="btn primary" id="empty-log-send" type="button">Send a test log</button>'
        : ''
    }</div>`;
  }

  // ------------------------------------------------------------------ search

  async function renderSearch() {
    const current = project();
    setTopbar('Search', `${current.name} · issues and logs in one query`, '');
    $('#view').innerHTML = `
      <div class="panel panel-pad" style="margin-bottom:16px">
        <div class="toolbar" style="margin:0">
          <input id="search-q" class="input" placeholder="Search issues and logs…" value="${esc(
            state.searchQuery,
          )}" style="max-width:520px" />
          <button class="btn primary" id="search-btn" type="button">Search</button>
        </div>
        <div class="hint">One query across grouped errors and log lines for this project.</div>
      </div>
      <div id="search-results"></div>`;

    const run = () => {
      state.searchQuery = $('#search-q').value.trim();
      if (state.searchQuery) performSearch();
    };
    $('#search-btn').addEventListener('click', run);
    $('#search-q').addEventListener('keydown', (event) => {
      if (event.key === 'Enter') run();
    });

    if (state.searchQuery) performSearch();
    else {
      $('#search-results').innerHTML = emptyState(
        'Search your telemetry',
        'Results include grouped issues and log lines from this project only.',
        '',
      );
    }
  }

  async function performSearch() {
    const current = project();
    const host = $('#search-results');
    if (!host) return;
    host.innerHTML = '<div class="skeleton" style="height:60px"></div>';
    try {
      const data = await api(
        `/api/projects/${current.id}/search?q=${encodeURIComponent(state.searchQuery)}`,
      );
      const issues = data.issues || [];
      const logs = data.logs || [];
      host.innerHTML = `
        <section class="panel panel-pad" style="margin-bottom:16px">
          <div class="section-title"><h3>Issues</h3><span class="muted small">${num(
            issues.length,
          )} match${issues.length === 1 ? '' : 'es'}</span></div>
          ${
            issues.length
              ? `<div class="issue-list">${issues.map(issueRow).join('')}</div>`
              : '<div class="muted small">No matching issues.</div>'
          }
        </section>
        <section class="panel">
          <div class="section-title" style="padding:16px 16px 0"><h3>Logs</h3><span class="muted small">${num(
            logs.length,
          )} match${logs.length === 1 ? '' : 'es'}</span></div>
          ${logs.length ? logTable(logs) : '<div class="empty small">No matching log lines.</div>'}
        </section>`;
    } catch (error) {
      toast(error.message, 'error');
      host.innerHTML = emptyState('Search failed', error.message, '');
    }
  }

  // ------------------------------------------------------------------ members

  async function renderMembers() {
    const current = project();
    setTopbar('Members', `${current.name} · project-scoped roles`, '');
    $('#view').innerHTML = '<div class="panel panel-pad"><div class="skeleton"></div><div class="skeleton"></div></div>';
    try {
      const data = await api(`/api/projects/${current.id}/members`);
      state.members = data.members || [];
      renderMembersPanel();
    } catch (error) {
      toast(error.message, 'error');
    }
  }

  function renderMembersPanel() {
    const current = project();
    const emails = state.accounts.map((account) => account.email);
    const rows = state.members
      .map((member) => {
        const controls = canManage()
          ? `<div style="display:flex;gap:6px;align-items:center">
                <select class="select" data-member="${esc(member.user_id)}" style="width:auto">
                  ${['owner', 'admin', 'triager', 'viewer']
                    .map((r) => option(r, r, member.role))
                    .join('')}
                </select>
                <button class="btn danger small" data-remove="${esc(
                  member.user_id,
                )}" type="button" title="Remove from project">Remove</button>
              </div>`
            : `<span class="badge ${esc(member.role)}">${esc(member.role)}</span>`;
        return `<div class="member-row">
          <div class="member-id">
            <span class="avatar">${esc(initials(member.name))}</span>
            <span class="who"><strong>${esc(member.name)}</strong><small>${esc(
              member.email,
            )}</small></span>
          </div>
          ${controls}
        </div>`;
      })
      .join('');

    $('#view').innerHTML = `
      <div class="panel panel-pad">
        <div class="section-title">
          <h3>Project access</h3>
          <span class="muted small">Roles are scoped to ${esc(current.name)}</span>
        </div>
        ${rows || '<div class="muted small">No members.</div>'}
        ${
          canManage()
            ? `<div class="inline-form">
                <label class="field">Email
                  <input id="member-email" class="input" list="account-emails" placeholder="person@example.com" />
                </label>
                <label class="field" style="max-width:150px">Role
                  <select id="member-role" class="select">
                    ${option('viewer', 'viewer', 'viewer')}
                    ${option('triager', 'triager', 'viewer')}
                    ${option('admin', 'admin', 'viewer')}
                    ${option('owner', 'owner', 'viewer')}
                  </select>
                </label>
                <button class="btn primary" id="add-member" type="button">Add member</button>
              </div>
              <datalist id="account-emails">${emails
                .map((email) => `<option value="${esc(email)}"></option>`)
                .join('')}</datalist>`
            : '<div class="hint">Only admins and owners can change access.</div>'
        }
      </div>`;

    $$('[data-member]').forEach((select) => {
      select.addEventListener('change', () => updateMember(select.dataset.member, select.value));
    });
    $$('[data-remove]').forEach((button) => {
      button.addEventListener('click', () => removeMember(button.dataset.remove));
    });
    const addButton = $('#add-member');
    if (addButton) addButton.addEventListener('click', addMember);
  }

  async function updateMember(userId, nextRole) {
    const current = project();
    try {
      await api(`/api/projects/${current.id}/members/${userId}`, {
        method: 'PUT',
        body: { role: nextRole },
      });
      toast('Member role updated', 'success');
      await refreshProjects();
      renderMembers();
    } catch (error) {
      toast(error.message, 'error');
      renderMembers();
    }
  }

  async function addMember() {
    const current = project();
    const email = $('#member-email').value.trim();
    const nextRole = $('#member-role').value;
    if (!email) {
      toast('Enter an email address', 'error');
      return;
    }
    try {
      await api(`/api/projects/${current.id}/members`, {
        method: 'POST',
        body: { email, role: nextRole },
      });
      toast('Member added', 'success');
      await refreshProjects();
      renderMembers();
    } catch (error) {
      toast(error.message, 'error');
    }
  }

  async function removeMember(userId) {
    const current = project();
    if (!window.confirm('Remove this member from the project?')) return;
    try {
      await api(`/api/projects/${current.id}/members/${userId}`, { method: 'DELETE' });
      toast('Member removed', 'success');
      await refreshProjects();
      renderMembers();
    } catch (error) {
      toast(error.message, 'error');
    }
  }

  // ------------------------------------------------------------------ settings

  async function renderSettings() {
    const current = project();
    setTopbar('Settings', `${current.name} · ingest, retention, decision model`, '');
    if (!canManage()) {
      $('#view').innerHTML = emptyState(
        'Admin access required',
        'Settings manage ingest keys, retention, and the decision model. Sign in as an owner or admin.',
        '',
      );
      return;
    }
    $('#view').innerHTML = '<div class="panel panel-pad"><div class="skeleton"></div><div class="skeleton"></div><div class="skeleton"></div></div>';
    await getOutage();
    try {
      const data = await api(`/api/projects/${current.id}/settings`);
      state.settings = data;
      renderSettingsPanel();
    } catch (error) {
      toast(error.message, 'error');
    }
  }

  function renderSettingsPanel() {
    const settings = state.settings;
    const current = project();
    if (!settings) return;
    const p = settings.project;
    const model = p.model || {};
    const keys = settings.keys || [];
    const keyRows = keys
      .map(
        (key) => `<div class="kv">
          <div>
            <div class="mono" style="font-size:12px">${esc(key.key)}</div>
            <div class="muted small">${esc(key.label)} · created ${esc(
              relative(key.created_at),
            )}${key.revoked ? ' · revoked' : ''}</div>
          </div>
          <div style="display:flex;gap:8px;align-items:center">
            <span class="badge ${key.revoked ? 'ignored' : 'resolved'}">${
              key.revoked ? 'revoked' : 'active'
            }</span>
            ${
              key.revoked
                ? ''
                : `<button class="btn danger small" data-revoke="${esc(
                    key.id,
                  )}" type="button">Revoke</button>`
            }
          </div>
        </div>`,
      )
      .join('');

    $('#view').innerHTML = `
      <div class="settings-grid">
        <section class="panel panel-pad">
          <div class="section-title"><h3>Ingest</h3><span class="badge">Sentry-compatible</span></div>
          <label class="field">DSN
            <div class="dsn-box">
              <input id="dsn" class="input" readonly value="${esc(settings.dsn || '')}" />
              <button class="btn" id="copy-dsn" type="button">Copy</button>
            </div>
          </label>
          <div class="hint">Change only this DSN in an existing Sentry SDK. Ingest path: <code>POST /api/${esc(
            p.dsn_id,
          )}/envelope/</code>. Keys are write-only and scoped to this project.</div>
          <div class="divider"></div>
          <div class="section-title"><h3>Ingest keys</h3><button class="btn small" id="new-key" type="button">Create key</button></div>
          <div id="key-list">${keyRows || '<div class="muted small">No keys.</div>'}</div>
        </section>

        <section class="panel panel-pad">
          <div class="section-title"><h3>Retention &amp; decision model</h3><span class="muted small">${
            p.model ? 'model connected' : 'no model'
          }</span></div>
          <div class="form-grid">
            <label class="field">Retention (days)
              <input id="retention" type="number" min="1" max="3650" class="input" value="${esc(
                p.retention_days,
              )}" />
            </label>
            <label class="field">Model id
              <input id="model-id" class="input" value="${esc(model.model || 'jev-latest')}" />
            </label>
            <label class="field full">Decision endpoint
              <input id="model-endpoint" class="input" placeholder="https://model.internal" value="${esc(
                model.endpoint || '',
              )}" />
            </label>
            <label class="field full">Bearer key (optional)
              <input id="model-key" class="input" value="${esc(model.api_key || '')}" />
            </label>
          </div>
          <div class="form-actions">
            <button class="btn ghost" id="disable-model" type="button">Disable model</button>
            <button class="btn ghost" id="demo-model" type="button">Use demo model</button>
            <button class="btn primary" id="save-settings" type="button">Save settings</button>
          </div>
          <div class="hint">Scout sends only a 2 KB summary. A slow or unreachable model leaves the item with no suggestion. Ingest never waits for it.</div>
        </section>

        <section class="panel panel-pad">
          <div class="section-title">
            <h3>Classifier resilience</h3>
            <span class="badge ${
              state.outage ? 'warning' : 'resolved'
            }">${state.outage ? 'outage simulated' : 'reachable'}</span>
          </div>
          <p class="muted small">Toggle a simulated outage of the built-in demo endpoint. Ingest keeps accepting and storing; new items simply carry no suggestion.</p>
          <div class="outage-strip" style="margin-top:13px">
            <span class="dot" style="width:8px;height:8px;border-radius:50%;background:${
              state.outage ? 'var(--amber)' : 'var(--green)'
            }"></span>
            <span>${
              state.outage
                ? 'Decision model is returning 503. Send an event to verify ingest is unaffected.'
                : 'Decision model responds to suggestions. Send an event to see one attached.'
            }</span>
          </div>
          <button class="btn ${
            state.outage ? 'primary' : ''
          }" id="toggle-outage" type="button">${
            state.outage ? 'Restore classifier' : 'Simulate classifier outage'
          }</button>
        </section>

        <section class="panel panel-pad">
          <div class="section-title"><h3>Demo data</h3><span class="muted small">in-memory</span></div>
          <p class="muted small">Reset the store to its seeded state. This signs you out and discards everything sent during this session.</p>
          <div class="form-actions" style="justify-content:flex-start;margin-top:18px">
            <button class="btn danger" id="reset-demo" type="button">Reset demo data</button>
          </div>
        </section>
      </div>`;

    const copy = $('#copy-dsn');
    if (copy)
      copy.addEventListener('click', async () => {
        try {
          await navigator.clipboard.writeText(settings.dsn || '');
          toast('DSN copied', 'success');
        } catch (_) {
          toast('Copy failed — select the field manually', 'error');
        }
      });

    const newKey = $('#new-key');
    if (newKey) newKey.addEventListener('click', createKey);
    $$('[data-revoke]').forEach((button) => {
      button.addEventListener('click', () => revokeKey(button.dataset.revoke));
    });

    const save = $('#save-settings');
    if (save) save.addEventListener('click', () => saveSettings(modelFromForm()));
    const demo = $('#demo-model');
    if (demo)
      demo.addEventListener('click', () => {
        $('#model-endpoint').value = settings.mock_model_endpoint || '';
        $('#model-id').value = 'jev-latest';
        saveSettings({
          endpoint: settings.mock_model_endpoint,
          model: 'jev-latest',
          api_key: null,
        });
      });
    const disable = $('#disable-model');
    if (disable) disable.addEventListener('click', () => saveSettings(null));

    const toggle = $('#toggle-outage');
    if (toggle) toggle.addEventListener('click', toggleOutage);
    const reset = $('#reset-demo');
    if (reset) reset.addEventListener('click', resetDemo);
  }

  function modelFromForm() {
    const endpoint = $('#model-endpoint').value.trim();
    if (!endpoint) return null;
    const model = $('#model-id').value.trim() || 'jev-latest';
    const apiKey = $('#model-key').value.trim();
    return { endpoint, model, api_key: apiKey || null };
  }

  async function saveSettings(model) {
    const current = project();
    const retention = Number($('#retention').value) || 30;
    try {
      await api(`/api/projects/${current.id}/settings`, {
        method: 'PUT',
        body: { retention_days: retention, model },
      });
      toast('Settings saved', 'success');
      await refreshProjects();
      renderSettings();
    } catch (error) {
      toast(error.message, 'error');
    }
  }

  async function createKey() {
    const current = project();
    try {
      const data = await api(`/api/projects/${current.id}/keys`, {
        method: 'POST',
        body: { label: 'Created in prototype' },
      });
      toast(`Created ${data.key.key}`, 'success');
      renderSettings();
    } catch (error) {
      toast(error.message, 'error');
    }
  }

  async function revokeKey(keyId) {
    const current = project();
    if (!window.confirm('Revoke this ingest key? Senders using it will be rejected.')) return;
    try {
      await api(`/api/projects/${current.id}/keys/${keyId}`, { method: 'DELETE' });
      toast('Key revoked', 'success');
      renderSettings();
    } catch (error) {
      toast(error.message, 'error');
    }
  }

  async function toggleOutage() {
    try {
      const response = await fetch('/mock-jev/outage', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ down: !state.outage }),
      });
      const data = await response.json();
      state.outage = Boolean(data.down);
      toast(state.outage ? 'Classifier outage simulated' : 'Classifier restored', 'success');
      renderSettings();
    } catch (error) {
      toast(error.message, 'error');
    }
  }

  async function resetDemo() {
    if (!window.confirm('Reset all demo data and sign out?')) return;
    try {
      await fetch('/api/demo/reset', { method: 'POST' });
    } catch (_) {
      /* ignore */
    }
    localStorage.removeItem('scout_token');
    location.reload();
  }

  // ------------------------------------------------------------------ send test event

  function openSendEventModal() {
    if (!canSend()) {
      toast('You need an ingest key — sign in as an owner or admin.', 'error');
      return;
    }
    const current = project();
    const dsn = `http://${current.key}@${location.host}/${current.dsn_id}`;
    openModal(
      `<div class="modal-head">
        <div>
          <h2>Send a test error</h2>
          <p>Posts a real Sentry envelope to <code>/api/${esc(
            current.dsn_id,
          )}/envelope/</code> with the project key.</p>
        </div>
        <button class="close" data-close type="button">×</button>
      </div>
      <div class="form-grid">
        <label class="field full">Exception type
          <input id="te-type" class="input" value="TypeError" />
        </label>
        <label class="field full">Message
          <input id="te-message" class="input" value="Cannot read properties of undefined (reading 'currency')" />
        </label>
        <label class="field">Level
          <select id="te-level" class="select">
            <option value="error">error</option>
            <option value="fatal">fatal</option>
            <option value="warning">warning</option>
            <option value="info">info</option>
          </select>
        </label>
        <label class="field">Release
          <input id="te-release" class="input" value="web@2.4.2" />
        </label>
        <label class="field">Occurrences
          <input id="te-count" type="number" min="1" max="1000" value="1" class="input" />
        </label>
        <label class="field" style="justify-content:flex-end">
          <span style="display:flex;gap:7px;align-items:center;font-size:12.5px">
            <input type="checkbox" id="te-attachment" checked /> include attachment
          </span>
        </label>
      </div>
      <div class="hint">Sending 1,000 identical events demonstrates grouping: exactly one issue with count 1,000. Events carry an explicit fingerprint, breadcrumbs, a release tag, and an attachment.</div>
      <div class="hint">DSN <span class="mono">${esc(dsn)}</span></div>
      <div class="modal-error hidden" id="te-error"></div>
      <div class="progress hidden" id="te-progress"><div></div></div>
      <div class="form-actions">
        <button class="btn ghost" data-close type="button">Cancel</button>
        <button class="btn primary" id="te-send" type="button">Send events</button>
      </div>`,
      (root) => {
        const send = root.querySelector('#te-send');
        send.addEventListener('click', async () => {
          const errorBox = root.querySelector('#te-error');
          const progress = root.querySelector('#te-progress');
          errorBox.classList.add('hidden');
          send.disabled = true;
          const total = Math.max(
            1,
            Math.min(1000, Number(root.querySelector('#te-count').value) || 1),
          );
          progress.classList.remove('hidden');
          const bar = progress.querySelector('div');
          try {
            const fingerprint = `${root.querySelector('#te-type').value}|${
              root.querySelector('#te-message').value
            }|src/lib/validate.ts|requireCurrency`;
            await sendTestEvents(
              {
                type: root.querySelector('#te-type').value,
                message: root.querySelector('#te-message').value,
                level: root.querySelector('#te-level').value,
                release: root.querySelector('#te-release').value,
                count: total,
                attachment: root.querySelector('#te-attachment').checked,
                fingerprint,
              },
              (done) => {
                bar.style.width = `${Math.round((done / total) * 100)}%`;
              },
            );
            toast(
              `Sent ${num(total)} event${total === 1 ? '' : 's'} to ${current.name}`,
              'success',
            );
            closeModal();
            navigate('issues');
            pollIssues();
          } catch (error) {
            errorBox.textContent = error.message;
            errorBox.classList.remove('hidden');
            send.disabled = false;
            progress.classList.add('hidden');
          }
        });
      },
    );
  }

  async function sendTestEvents(options, onProgress) {
    const current = project();
    const { type, message, level, release, count, attachment, fingerprint } = options;
    const chunkSize = 20;
    let done = 0;
    for (let start = 0; start < count; start += chunkSize) {
      const size = Math.min(chunkSize, count - start);
      const batch = [];
      for (let i = 0; i < size; i += 1) {
        batch.push(
          postTestEvent({
            dsnId: current.dsn_id,
            key: current.key,
            type,
            message,
            level,
            release,
            fingerprint,
            attachment,
          }),
        );
      }
      await Promise.all(batch);
      done += size;
      if (onProgress) onProgress(done, count);
    }
  }

  async function postTestEvent({ dsnId, key, type, message, level, release, fingerprint, attachment }) {
    const eventId = randomHex(16);
    const now = Date.now();
    const event = {
      event_id: eventId,
      timestamp: now / 1000,
      platform: 'javascript',
      level,
      release,
      environment: 'production',
      message: `${type}: ${message}`,
      fingerprint: [fingerprint],
      exception: {
        values: [
          {
            type,
            value: message,
            stacktrace: {
              frames: [
                {
                  filename: 'src/checkout/index.ts',
                  function: 'handleCheckout',
                  lineno: 57,
                  colno: 5,
                  in_app: true,
                  context_line: 'await payment.chargeCard(cart, discount);',
                },
                {
                  filename: 'src/checkout/payment.ts',
                  function: 'chargeCard',
                  lineno: 88,
                  colno: 9,
                  in_app: true,
                  context_line: 'const amount = cart.total(currency);',
                },
                {
                  filename: 'src/lib/validate.ts',
                  function: 'requireCurrency',
                  lineno: 19,
                  colno: 7,
                  in_app: true,
                  context_line: 'if (!value.currency) throw new TypeError("missing currency");',
                },
              ],
            },
          },
        ],
      },
      breadcrumbs: {
        values: [
          {
            timestamp: new Date(now - 6000).toISOString(),
            category: 'console',
            level: 'info',
            message: 'applying discount SAVE10',
          },
          {
            timestamp: new Date(now - 3000).toISOString(),
            category: 'http',
            level: 'info',
            message: 'POST /api/discounts 200',
            data: { status: 200, duration_ms: 84 },
          },
          {
            timestamp: new Date(now - 1000).toISOString(),
            category: 'ui.click',
            level: 'info',
            message: 'button#pay',
          },
        ],
      },
      tags: { service: 'checkout-web', region: 'eu-west-1', source: 'prototype' },
      user: { id: 'u_8241', email: 'oncall@acme.dev' },
      sdk: { name: 'sentry.javascript.browser', version: '8.42.0' },
    };

    const body = JSON.stringify(event);
    const encoder = new TextEncoder();
    let envelope = `${JSON.stringify({
      event_id: eventId,
      sent_at: new Date().toISOString(),
    })}\n`;
    envelope += `${JSON.stringify({ type: 'event', length: encoder.encode(body).length })}\n`;
    envelope += `${body}\n`;
    if (attachment) {
      const attachmentText = 'cart=8f21\ncurrency=undefined\nroute=/checkout\n';
      const bytes = encoder.encode(attachmentText);
      envelope += `${JSON.stringify({
        type: 'attachment',
        length: bytes.length,
        filename: 'checkout-context.txt',
        content_type: 'text/plain',
      })}\n`;
      envelope += attachmentText;
      envelope += '\n';
    }

    const response = await fetch(
      `/api/${encodeURIComponent(dsnId)}/envelope/?sentry_key=${encodeURIComponent(key)}`,
      {
        method: 'POST',
        headers: { 'Content-Type': 'application/x-sentry-envelope' },
        body: envelope,
      },
    );
    if (!response.ok) {
      const text = await response.text();
      let messageText = `Ingest failed (${response.status})`;
      try {
        messageText = JSON.parse(text).error || messageText;
      } catch (_) {
        /* keep default */
      }
      throw new Error(messageText);
    }
    return response.json();
  }

  function pollIssues(attempts = 6) {
    let attempt = 0;
    const tick = async () => {
      if (state.view !== 'issues') return;
      await loadIssues();
      await refreshProjects();
      attempt += 1;
      if (attempt < attempts) setTimeout(tick, 450);
    };
    setTimeout(tick, 300);
  }

  // ------------------------------------------------------------------ send test log

  function openSendLogModal() {
    if (!canSend()) {
      toast('You need an ingest key — sign in as an owner or admin.', 'error');
      return;
    }
    const current = project();
    openModal(
      `<div class="modal-head">
        <div>
          <h2>Send a test log</h2>
          <p>Posts a structured log line to <code>/api/${esc(
            current.dsn_id,
          )}/logs/</code> with the project key.</p>
        </div>
        <button class="close" data-close type="button">×</button>
      </div>
      <div class="form-grid">
        <label class="field full">Message
          <input id="tl-message" class="input" value="checkout failed for cart 8f21: TypeError: missing currency" />
        </label>
        <label class="field">Level
          <select id="tl-level" class="select">
            <option value="info">info</option>
            <option value="warning">warning</option>
            <option value="error" selected>error</option>
            <option value="debug">debug</option>
          </select>
        </label>
        <label class="field">Source
          <input id="tl-source" class="input" value="checkout-web" />
        </label>
      </div>
      <div class="hint">The line is searchable alongside issues immediately. Attributes are attached to show structured metadata.</div>
      <div class="modal-error hidden" id="tl-error"></div>
      <div class="form-actions">
        <button class="btn ghost" data-close type="button">Cancel</button>
        <button class="btn primary" id="tl-send" type="button">Send log</button>
      </div>`,
      (root) => {
        root.querySelector('#tl-send').addEventListener('click', async () => {
          const errorBox = root.querySelector('#tl-error');
          errorBox.classList.add('hidden');
          const payload = {
            logs: [
              {
                timestamp: Date.now() / 1000,
                level: root.querySelector('#tl-level').value,
                message: root.querySelector('#tl-message').value,
                source: root.querySelector('#tl-source').value || 'prototype',
                attributes: { service: 'checkout-web', sent_by: 'prototype-ui' },
              },
            ],
          };
          try {
            const response = await fetch(
              `/api/${encodeURIComponent(current.dsn_id)}/logs/?sentry_key=${encodeURIComponent(
                current.key,
              )}`,
              {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify(payload),
              },
            );
            if (!response.ok) {
              const text = await response.text();
              let message = `Log ingest failed (${response.status})`;
              try {
                message = JSON.parse(text).error || message;
              } catch (_) {
                /* keep default */
              }
              throw new Error(message);
            }
            toast('Log line ingested', 'success');
            closeModal();
            navigate('logs');
          } catch (error) {
            errorBox.textContent = error.message;
            errorBox.classList.remove('hidden');
          }
        });
      },
    );
  }

  // ------------------------------------------------------------------ go

  boot();
})();
