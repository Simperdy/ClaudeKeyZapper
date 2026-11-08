// KeyZapper UI: renders the view the Rust side sends and calls its commands. No framework, no build step.
import { createHermesController, hermesPage } from './hermes.js';
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

let view = null;
let english = {};
/** Per profile: { checking, result: { success, message } } */
const checks = new Map();
/** Open dialogs that re-render when the view changes. */
const liveDialogs = new Set();
let alertOpen = false;
let whatsNewShown = false;

// MARK: Helpers

/** Translates a German source string and fills its `%@` placeholders in order. */
function t(key, ...args) {
  const text = view?.language === 'en' ? english[key] ?? key : key;
  let i = 0;
  return text.replace(/%@/g, () => (i < args.length ? String(args[i++]) : '%@'));
}

const PROPS = new Set(['value', 'checked', 'disabled', 'selected', 'hidden', 'required']);

function h(tag, props, ...children) {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(props ?? {})) {
    if (value == null || value === false) continue;
    if (key.startsWith('on')) el.addEventListener(key.slice(2), value);
    else if (key === 'class') el.className = value;
    else if (PROPS.has(key)) el[key] = value;
    else el.setAttribute(key, value === true ? '' : value);
  }
  for (const child of children.flat(Infinity)) {
    if (child == null || child === false) continue;
    el.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return el;
}

const ICONS = {
  plus: '<path d="M12 5v14M5 12h14"/>',
  folder: '<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>',
  folderPlus: '<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/><path d="M12 11v6M9 14h6"/>',
  refresh: '<path d="M21 12a9 9 0 1 1-3-6.7L21 8"/><path d="M21 3v5h-5"/>',
  shield: '<path d="M12 3l8 3v6c0 4.5-3.4 8.3-8 9-4.6-.7-8-4.5-8-9V6z"/><path d="m9 12 2 2 4-4"/>',
  archive: '<rect x="3" y="4" width="18" height="4" rx="1"/><path d="M5 8v11a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8M10 12h4"/>',
  info: '<circle cx="12" cy="12" r="9"/><path d="M12 11v5M12 8h.01"/>',
  key: '<circle cx="7.5" cy="15.5" r="4.5"/><path d="m10.7 12.3 9.3-9.3M17 6l3 3M14 9l2 2"/>',
  keyOff: '<circle cx="7.5" cy="15.5" r="4.5"/><path d="m10.7 12.3 9.3-9.3M17 6l3 3M3 3l18 18"/>',
  copy: '<rect x="9" y="9" width="12" height="12" rx="2"/><path d="M5 15H4a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1h10a1 1 0 0 1 1 1v1"/>',
  pencil: '<path d="M17 3l4 4L8 20H4v-4z"/>',
  rotate: '<path d="M3 12a9 9 0 0 1 15-6.7L21 8M21 3v5h-5M21 12a9 9 0 0 1-15 6.7L3 16M3 21v-5h5"/>',
  trash: '<path d="M3 6h18M8 6V4h8v2M6 6l1 14h10l1-14"/>',
  check: '<circle cx="12" cy="12" r="9"/><path d="m8 12 3 3 5-6"/>',
  warning: '<path d="M12 3 2 20h20z"/><path d="M12 9v5M12 17h.01"/>',
  error: '<path d="M8 2h8l6 6v8l-6 6H8l-6-6V8z"/><path d="m15 9-6 6M9 9l6 6"/>',
  question: '<circle cx="12" cy="12" r="9"/><path d="M9.5 9a2.5 2.5 0 1 1 3.5 2.3c-.6.3-1 .9-1 1.7M12 17h.01"/>',
  flame: '<path d="M12 22c4 0 7-3 7-7 0-4-3-6-4-9-1 2-2 3-3 3 0-2-1-4-3-6 0 4-4 6-4 12 0 4 3 7 7 7z"/>',
  dollar: '<circle cx="12" cy="12" r="9"/><path d="M15 9h-4a1.5 1.5 0 0 0 0 3h2a1.5 1.5 0 0 1 0 3H9M12 7v2M12 15v2"/>',
  pause: '<circle cx="12" cy="12" r="9"/><path d="M10 9v6M14 9v6"/>',
  turn: '<path d="M5 4v7a3 3 0 0 0 3 3h11"/><path d="m15 10 4 4-4 4"/>',
  close: '<path d="M6 6l12 12M18 6 6 18"/>',
  bolt: '<path d="M13 2 4 14h7l-1 8 9-12h-7z"/>',
  terminal: '<rect x="3" y="4" width="18" height="16" rx="2"/><path d="m7 9 3 3-3 3M13 15h4"/>',
  server: '<rect x="3" y="4" width="18" height="7" rx="1"/><rect x="3" y="13" width="18" height="7" rx="1"/><path d="M7 7.5h.01M7 16.5h.01"/>',
  fileLock: '<path d="M6 2h8l4 4v16H6z"/><path d="M14 2v4h4"/><rect x="9" y="13" width="6" height="5" rx="1"/><path d="M10 13v-2a2 2 0 0 1 4 0v2"/>',
  arrow: '<path d="M5 12h14M13 6l6 6-6 6"/>',
  sparkles: '<path d="M10 3l1.8 4.7 4.7 1.8-4.7 1.8L10 16l-1.8-4.7L3.5 9.5l4.7-1.8z"/><path d="M18 14l.9 2.1 2.1.9-2.1.9L18 20l-.9-2.1-2.1-.9 2.1-.9z"/>',
  plug: '<path d="M9 3v5M15 3v5M6 8h12v3a6 6 0 0 1-12 0z"/><path d="M12 17v4"/>',
  more: '<circle cx="5" cy="12" r="1"/><circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/>',
  activity: '<path d="M3 12h4l3-8 4 16 3-8h4"/>',
  upload: '<path d="M12 15V3M7 8l5-5 5 5"/><path d="M4 15v4a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-4"/>',
  download: '<path d="M12 3v12M7 10l5 5 5-5"/><path d="M4 15v4a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-4"/>',
  blank: '',
};

function icon(name, cls = '') {
  const span = document.createElement('span');
  span.innerHTML = `<svg class="icon ${cls}" viewBox="0 0 24 24" aria-hidden="true">${ICONS[name]}</svg>`;
  return span.firstElementChild;
}

function button(label, onClick, { iconName, cls, disabled, title, iconOnly } = {}) {
  return h('button', {
    type: 'button', class: [cls, iconOnly && 'icon-only'].filter(Boolean).join(' ') || null, disabled,
    title: title ?? (iconOnly ? label : null), 'aria-label': iconOnly ? label : null, onclick: onClick,
  }, iconName && icon(iconName), !iconOnly && label);
}

async function call(command, args, { quiet = false } = {}) {
  try {
    const result = await invoke(command, args);
    if (result?.profiles) setView(result);
    else if (Array.isArray(result) && result[1]?.profiles) setView(result[1]);
    else setView(await invoke('get_view'));
    return result;
  } catch (error) {
    if (!quiet) showAlert(String(error));
    return null;
  }
}

const hermesController = createHermesController({
  call: (command, args) => call(command, args, { quiet: true }),
  readView: () => invoke('get_view'),
  changed: () => { if (page === 'hermes') renderMain(); },
  confirm: async ({ action, target, selection, preview }) => {
    const profile = selection && view.profiles.find((item) => item.id === selection.profileId);
    const label = action === 'connect' ? t('Hermes verbinden') : action === 'disconnect' ? t('Hermes-Verbindung entfernen') : t('Hermes-Vorgang reparieren');
    const recoverySummary = preview?.summary?.startsWith('Clean up artifacts')
      ? t('Hermes räumt vorbereitete Artefakte des unterbrochenen Vorgangs am Ziel %@ auf.', target.displayName)
      : preview?.summary?.startsWith('Roll back')
        ? t('Hermes setzt die unvollständigen Änderungen des unterbrochenen Vorgangs am Ziel %@ zurück.', target.displayName)
        : t('Hermes wählt anhand der gespeicherten Vorgangsphase zwischen dem Aufräumen vorbereiteter Artefakte und dem Zurücksetzen unvollständiger Änderungen am Ziel %@.', target.displayName);
    const details = h('div', null,
      h('p', null, t('Installation: %@', target.displayName)),
      h('p', { class: 'mono selectable' }, t('Hermes-Home: %@', target.home)),
      h('p', { class: 'mono selectable' }, t('Programm: %@', target.executable)),
      profile && h('p', null, t('KeyZapper-Profil: %@', profile.name)),
      selection && h('p', null, t('Mandant: %@', selection.tenant || t('Kein Tenant'))),
      selection && h('p', null, t('Modell: %@', selection.modelId)),
      action === 'connect' && h('p', null, t('Die Aktivierung des Hermes-Standardmodells wird auf das ausgewählte Modell geändert.')),
      action === 'recover' && h('p', null, recoverySummary),
      h('p', null, t('Öffne eine neue oder neu gestartete Hermes-Sitzung, damit diese Verbindung verwendet wird. Bestehende Sitzungen behalten möglicherweise den zwischengespeicherten statischen Key.')));
    return confirmDialog(t('Hermes-Änderung bestätigen'), details, label);
  },
});
let hermesRefreshed = false;

const isMac = () => view?.platform !== 'windows';
const revealLabel = () => (isMac() ? t('Im Finder zeigen') : t('Im Explorer zeigen'));
const profileName = (id) => view.profiles.find((p) => p.id === id)?.name ?? t('Unbekanntes Profil');
/** Key stored, signed in, or (subscription) Claude Code's login present; unknown until checked counts as present. */
const credentialOk = (p) => (p.subscription ? view.subscription?.loggedIn !== false : p.sso ? p.ssoState === 'loggedIn' : p.hasKey);
const ssoStateText = (p) => ({ loggedIn: t('Angemeldet'), expired: t('Sitzung abgelaufen') }[p.ssoState] ?? t('Nicht angemeldet'));
/** Picker label: SSO and subscription profiles carry a badge, SSO also its login state. */
const profileLabel = (p) => (p.subscription ? `${p.name} · ${t('Claude-Abo')}` : p.sso ? `${p.name} · SSO (${ssoStateText(p)})` : p.name);
/** Signs an SSO profile in before it is used; false if it is (still) not signed in. */
async function ensureLogin(profileId) {
  const profile = view.profiles.find((p) => p.id === profileId);
  if (!profile?.sso || profile.ssoState === 'loggedIn') return true;
  await call('sso_login', { profileId });
  return view.profiles.find((p) => p.id === profileId)?.ssoState === 'loggedIn';
}
const locale = () => (navigator.language?.toLowerCase().startsWith(view.language) ? navigator.language : view.language);

// MARK: Render

function setView(next) {
  view = next;
  // Same non-secret backend state path as the controller's own reads: keep the Hermes page
  // snapshot in step with every published view instead of leaving it stale.
  if (view.hermes) hermesController.sync(view.hermes);
  const root = document.documentElement;
  root.lang = view.language;
  // The popover gets the system material only on macOS (vibrancy); on Windows it paints its own background.
  root.classList.toggle('vibrancy', !!view.vibrancy && (!isPopover() || isMac()));
  root.classList.toggle('mac', isMac());
  assignColors();
  if (isPopover()) return renderPopover();
  renderSidebar();
  renderBanners();
  renderMain();
  renderToast();
  for (const render of liveDialogs) render();
  if (view.error && !alertOpen) showAlert(view.error, () => call('dismiss_error'));
  if (view.whatsNew.length && !whatsNewShown) whatsNewDialog();
  if (!hermesRefreshed) { hermesRefreshed = true; hermesController.refresh(); }
}

// MARK: Navigation

const PAGES = () => [
  ['profiles', t('Profile'), 'key'],
  ['projects', t('Projekte'), 'folder'],
  ['mcp', t('MCP-Server'), 'plug'],
  ['hermes', t('Hermes'), 'plug'],
  view.claudeToolsEnabled && ['tools', t('Claude-Tools'), 'download'],
  ['settings', t('Claude-Einstellungen'), 'shield'],
  ['backup', t('Backup'), 'archive'],
].filter(Boolean);

let page = 'profiles';
try { page = localStorage.getItem('page') ?? page; } catch { /* storage unavailable */ }

function navigate(next) {
  page = next;
  try { localStorage.setItem('page', next); } catch { /* storage unavailable */ }
  if (next === 'hermes') hermesController.refresh();
  if (next === 'settings') {
    settingsSelection = null;
    call('refresh_all');
  }
  if (next === 'tools') {
    desktopSelection = null;
    call('load_tools');
  }
  renderSidebar();
  renderMain();
  document.getElementById('main').scrollTop = 0;
}

let sidebarSignature = '';

function renderSidebar() {
  const attention = view.disabled ? 0 : view.bindings.filter((b) => projectStatus(b).attention).length;
  const settingsProblems = view.globalFindings.filter((f) => f.severity !== 'info').length;
  const badges = {
    profiles: [view.profiles.length || null],
    projects: attention ? [attention, 'warning'] : [view.bindings.length || null],
    mcp: view.mcpCatalog?.problem ? ['!', 'warning'] : [view.mcpServers.length || null],
    tools: [null],
    settings: settingsProblems ? [settingsProblems, 'warning'] : [null],
    backup: [null],
  };
  const signature = JSON.stringify([page, view.language, view.disabled, badges, view.appVersion, view.updateCheckEnabled, view.betaUpdates, view.platform, view.claudeToolsEnabled]);
  if (signature === sidebarSignature) return;
  sidebarSignature = signature;
  const active = h('input', {
    type: 'checkbox', role: 'switch', checked: !view.disabled,
    onchange: async (event) => {
      if (event.target.checked) return call('set_disabled', { disabled: false });
      event.target.checked = true;
      const blocked = hermesMutationBlock(null, true);
      if (blocked) { await confirmDialog(t('Hermes zuerst trennen'), blocked, t('Verstanden')); return; }
      if (await confirmDialog(t('KeyZapper deaktivieren?'),
        t('KeyZapper entfernt seine Einträge aus allen Projekten und aus ~/.claude/settings.json. Die Zuordnungen bleiben gespeichert und werden beim Aktivieren wieder geschrieben.'),
        t('Deaktivieren'))) call('set_disabled', { disabled: true });
    },
  });
  const beta = h('input', {
    type: 'checkbox', role: 'switch', checked: view.betaUpdates,
    onchange: (event) => call('set_beta_updates', { enabled: event.target.checked }),
  });
  document.getElementById('sidebar').replaceChildren(
    h('div', { class: 'brand', 'data-tauri-drag-region': '' }, h('span', { class: 'logo' }, icon('bolt')), 'KeyZapper'),
    ...PAGES().map(([id, label, symbol]) => {
      const [count, tone] = badges[id];
      return h('button', { type: 'button', class: `nav-item${page === id ? ' active' : ''}`, 'aria-current': page === id ? 'page' : null, onclick: () => navigate(id) },
        icon(symbol), h('span', null, label), count != null && h('span', { class: `nav-badge ${tone ?? ''}` }, count));
    }),
    h('div', { class: 'sidebar-footer' },
      h('label', { class: 'switch', title: t('KeyZapper vorübergehend deaktivieren: Projekte nutzen dann die normale Claude-Anmeldung') }, active, t('KeyZapper aktiv')),
      view.updateCheckEnabled && view.appVersion && h('label', { class: 'switch', title: t('Bei der Update-Suche auch Beta-Versionen anbieten. Beta-Versionen sind zum Testen gedacht.') }, beta, t('Beta-Versionen')),
      button(t('So funktioniert’s'), howItWorksDialog, { iconName: 'info', cls: 'link' }),
      h('div', { class: 'version' },
        view.appVersion ? `KeyZapper ${view.appVersion}` : t('KeyZapper Entwicklungsversion'),
        view.updateCheckEnabled
          ? button(t('Nach Updates suchen'), () => call('check_for_updates'), { cls: 'link' })
          : h('span', null, t('Updates über die IT')),
        button(t('Projektseite'), () => call('open_url', { url: view.projectUrl }), { cls: 'link' }))),
  );
}

function banner(kind, title, detail, actions = [], onDismiss = null) {
  return h('div', { class: `banner ${kind}`, role: kind === 'warning' ? 'alert' : 'status' },
    icon(kind === 'warning' ? 'warning' : 'info'),
    h('div', { class: 'banner-text' },
      h('div', { class: `banner-title ${kind}` }, title),
      detail && h('div', { class: 'detail' }, detail)),
    actions.map(([label, action], i) => button(label, action, { cls: i === 0 ? 'primary' : null })),
    onDismiss && button(t('Schließen'), onDismiss, { iconName: 'close', iconOnly: true }));
}

function renderBanners() {
  const items = [];
  if (view.availableBackup) {
    items.push(banner('info',
      t('OneDrive-Backup gefunden: %@ Profil(e), %@ Projekt(e)', view.availableBackup.profiles, view.availableBackup.projects),
      t('Profile und Zuordnungen wiederherstellen? Keys sind nicht im Backup und müssen neu eingetragen werden.'),
      [[t('Wiederherstellen'), () => call('restore_from_backup')], [t('Verwerfen'), () => call('discard_backup')]]));
  }
  if (view.backupProblem) items.push(banner('warning', view.backupProblem));
  if (view.keychainKeys > 0) {
    items.push(banner('warning',
      t('%@ Key(s) liegen noch im macOS-Schlüsselbund', view.keychainKeys),
      t('KeyZapper speichert Keys jetzt ohne Schlüsselbund. Einmal übernehmen; macOS fragt dabei je Key nach der Erlaubnis.'),
      [[t('Übernehmen'), () => call('migrate_keychain')]]));
  }
  if (view.disabled) {
    items.push(banner('warning', t('KeyZapper ist deaktiviert'),
      t('Claude Code nutzt in allen Projekten die normale Anmeldung. Die Zuordnungen bleiben gespeichert.'),
      [[t('Aktivieren'), () => call('set_disabled', { disabled: false })]]));
  }
  const problems = view.globalFindings.filter((f) => f.severity !== 'info');
  if (problems.length && page !== 'settings') {
    items.push(banner('warning', t('~/.claude/settings.json: %@ Hinweis(e)', problems.length), problems[0].message,
      [[t('Prüfen'), () => navigate('settings')]]));
  }
  if (view.outdatedClis.length) {
    items.push(banner('warning', t('Claude-Code-CLI veraltet: %@', view.outdatedClis.join(', ')),
      t('IntelliJ nutzt diese CLI. Unter %@ gilt die Projektzuordnung nur beim Start direkt im Projektordner. Aktualisieren mit „claude update“.', view.minimumCliVersion)));
  }
  if (view.availableUpdate) {
    const { version, prerelease } = view.availableUpdate;
    items.push(banner('info',
      prerelease ? t('Beta: KeyZapper %@ ist verfügbar (installiert: %@)', version, view.appVersion ?? '?')
        : t('KeyZapper %@ ist verfügbar (installiert: %@)', version, view.appVersion ?? '?'),
      view.installingUpdate ? t('Update wird geladen, geprüft und installiert – KeyZapper startet danach neu …')
        : prerelease ? t('Beta-Versionen sind zum Testen gedacht. Die Installation benötigt Administratorrechte, danach startet KeyZapper neu.')
          : t('Die Installation benötigt Administratorrechte, danach startet KeyZapper neu.'),
      [[t('Installieren'), () => call('install_update')], [t('Versionshinweise'), () => call('open_url', { url: view.availableUpdate.pageUrl })]]));
  }
  document.getElementById('banners').replaceChildren(...items);
}

// MARK: Toasts

let toastText = null;
let toastTimer = null;

/** Notices show briefly at the bottom right, so they are seen wherever the user clicked. */
function renderToast() {
  if (view.notice === toastText) return;
  toastText = view.notice;
  clearTimeout(toastTimer);
  const container = document.getElementById('toasts');
  if (!toastText) return container.replaceChildren();
  const dismiss = () => call('dismiss_notice');
  const later = (ms) => { clearTimeout(toastTimer); toastTimer = setTimeout(dismiss, ms); };
  container.replaceChildren(h('div', { class: 'toast', onmouseenter: () => clearTimeout(toastTimer), onmouseleave: () => later(3000) },
    icon('check', 'green'), h('div', { class: 'toast-text selectable' }, toastText),
    button(t('Schließen'), dismiss, { iconName: 'close', iconOnly: true })));
  later(7000);
}

// MARK: Menus

let openedMenu = null;
/** The menu button whose menu the last outside click closed, so that click does not open it again. */
let closedByTrigger = null;

function closeMenu() {
  openedMenu?.remove();
  openedMenu = null;
}

/** Opens a menu below `anchor` (an element) or at a point; items are [label, action, { iconName, danger, disabled }] or '-'. */
function openMenu(anchor, items) {
  closeMenu();
  const menu = h('div', { class: 'menu', popover: 'manual', role: 'menu' },
    items.filter(Boolean).map((item) => (item === '-' ? h('div', { class: 'menu-separator', role: 'separator' })
      : h('button', { type: 'button', role: 'menuitem', class: item[2]?.danger ? 'danger' : null, disabled: item[2]?.disabled,
        onclick: () => { closeMenu(); item[1](); } }, icon(item[2]?.iconName ?? 'blank'), item[0]))));
  menu.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      closeMenu();
      if (anchor instanceof Element) anchor.focus();
      return;
    }
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
    e.preventDefault();
    const entries = [...menu.querySelectorAll('button:not(:disabled)')];
    const next = entries.indexOf(document.activeElement) + (e.key === 'ArrowDown' ? 1 : -1);
    entries[(next + entries.length) % entries.length]?.focus();
  });
  document.body.append(menu);
  menu.showPopover();
  openedMenu = menu;
  const element = anchor instanceof Element;
  const rect = element ? anchor.getBoundingClientRect() : { left: anchor.x, right: anchor.x, top: anchor.y, bottom: anchor.y };
  const { offsetWidth: width, offsetHeight: height } = menu;
  menu.style.left = `${Math.max(8, Math.min(element ? rect.right - width : rect.left, innerWidth - width - 8))}px`;
  menu.style.top = `${rect.bottom + 4 + height <= innerHeight ? rect.bottom + 4 : Math.max(8, rect.top - 4 - height)}px`;
  menu.querySelector('button:not(:disabled)')?.focus();
}

// Menus close on clicks outside, when the window loses focus or changes size.
document.addEventListener('pointerdown', (e) => {
  closedByTrigger = null;
  if (!openedMenu || openedMenu.contains(e.target)) return;
  closedByTrigger = e.target.closest?.('[aria-haspopup="menu"]') ?? null;
  closeMenu();
}, true);
window.addEventListener('blur', closeMenu);
window.addEventListener('resize', closeMenu);

function menuButton(label, items) {
  const trigger = button(label, () => {
    if (closedByTrigger === trigger) closedByTrigger = null;
    else openMenu(trigger, items());
  }, { iconName: 'more', iconOnly: true });
  trigger.setAttribute('aria-haspopup', 'menu');
  return trigger;
}

/** Right-click menu; the innermost element with one wins, text fields keep the system menu. */
const contextMenu = (items) => (event) => {
  if (event.target.closest('input, textarea, select, .selectable')) return;
  event.preventDefault();
  event.stopPropagation();
  openMenu({ x: event.clientX, y: event.clientY }, items());
};

// MARK: Pages

function renderMain() {
  const main = document.getElementById('main');
  const scroll = main.scrollTop;
  const pages = { profiles: profilesPage, projects: projectsPage, mcp: mcpPage, hermes: () => hermesPage({ view, t, h, field, pageHeader, controller: hermesController }), tools: view.claudeToolsEnabled ? toolsPage : null, settings: settingsPage, backup: backupPage };
  main.replaceChildren(...[(pages[page] ?? profilesPage)()].flat(Infinity).filter(Boolean));
  main.scrollTop = scroll;
}

function pageHeader(title, subtitle, ...actions) {
  return h('header', { class: 'page-header', 'data-tauri-drag-region': '' },
    h('div', { class: 'page-title', 'data-tauri-drag-region': '' },
      h('h1', { 'data-tauri-drag-region': '' }, title),
      subtitle && h('p', { 'data-tauri-drag-region': '' }, subtitle)),
    h('div', { class: 'page-actions' }, actions));
}

const refreshButton = () => button(t('Status aktualisieren'), () => call('refresh_all'), { iconName: 'refresh', iconOnly: true });

function noProfiles() {
  return h('div', { class: 'empty' },
    h('span', { class: 'logo big' }, icon('bolt')),
    h('h2', null, t('Noch keine Profile')),
    h('p', null, t('Lege ein Profil mit LiteLLM-Endpunkt, Modellalias und deinem freigegebenen Key an.')),
    button(t('Profil anlegen'), () => profileEditor(null), { cls: 'primary' }),
    button(t('Backup importieren …'), importDialog, { cls: 'link' }));
}

function profilesPage() {
  if (!view.profiles.length) return noProfiles();
  return [
    pageHeader(t('Profile'), t('Ein Key, eine SSO-Anmeldung oder dein Claude-Abo je Profil – zugeordnete Projekte nutzen es automatisch.'),
      refreshButton(),
      button(t('Projekt zuordnen'), () => addProjectDialog(null), { iconName: 'folderPlus' }),
      button(t('Neues Profil'), () => profileEditor(null), { iconName: 'plus', cls: 'primary' })),
    summaryBar(),
    view.profiles.map(profileCard),
    h('div', { class: 'hint' }, icon('bolt', 'accent'),
      t('Claude Code holt in jedem zugeordneten Projekt den passenden Key automatisch über den Helper – kein manuelles Wechseln nötig. Nach neuen Zuordnungen die Claude-Sitzung einmal neu starten.')),
  ];
}

function projectsPage() {
  if (!view.profiles.length) return noProfiles();
  return [
    pageHeader(t('Projekte'), t('Ordner aus dem %@ hierher ziehen, um sie zuzuordnen – auf ein Profil gezogen gilt direkt dieses.', isMac() ? 'Finder' : 'Explorer'),
      refreshButton(),
      button(t('Projekt zuordnen'), () => addProjectDialog(null), { iconName: 'folderPlus', cls: 'primary' })),
    summaryBar(),
    view.bindings.length
      ? h('div', { class: 'project-list standalone' }, view.bindings.map((b) => projectRow(b, profileOf(b))))
      : h('div', { class: 'empty compact' }, icon('folder'), h('h2', null, t('Noch keine Projekte')),
        h('p', null, t('Ziehe einen Projektordner hierher oder wähle „Projekt zuordnen“.'))),
  ];
}

/** "4 Projekte · 3 aktiv · 1 braucht Aufmerksamkeit" with one button that repairs what writing the settings again fixes. */
function summaryBar() {
  const bindings = view.bindings;
  if (!bindings.length) return null;
  if (view.disabled) {
    return h('div', { class: 'summary' }, icon('pause', 'muted'), t('Pausiert – Claude nutzt in allen Projekten die normale Anmeldung.'));
  }
  const states = bindings.map(projectStatus);
  const attention = states.filter((s) => s.attention).length;
  const repairable = bindings.filter((b, i) => states[i].repairable);
  const total = bindings.length === 1 ? t('1 Projekt') : t('%@ Projekte', bindings.length);
  const active = states.filter((s) => s.tone === 'green').length;
  return h('div', { class: `summary${attention ? ' attention' : ''}`, role: 'status' },
    icon(attention ? 'warning' : 'check', attention ? 'orange' : 'green'),
    h('span', null, h('strong', null, total),
      h('span', { class: 'muted' }, ` · ${t('%@ aktiv', active)}`),
      attention > 0 && h('span', { class: 'orange' }, ` · ${attention === 1 ? t('1 braucht Aufmerksamkeit') : t('%@ brauchen Aufmerksamkeit', attention)}`)),
    h('span', { class: 'spacer' }),
    repairable.length > 0 && button(repairable.length === 1 ? t('Reparieren') : t('Alle reparieren'),
      () => call('reapply_many', { bindingIds: repairable.map((b) => b.id) }), { iconName: 'rotate', cls: 'primary' }));
}

// MARK: Profiles

const PROFILE_COLORS = ['#4f46e5', '#0891b2', '#db2777', '#ea580c', '#059669', '#7c3aed', '#2563eb', '#ca8a04'];
let profileColors = new Map();

/** Stable color per profile (from its ID); collisions move to the next free color in profile order. */
function assignColors() {
  const taken = new Set();
  profileColors = new Map(view.profiles.map((p) => {
    let index = [...p.id].reduce((hash, c) => (hash * 31 + c.charCodeAt(0)) >>> 0, 7) % PROFILE_COLORS.length;
    for (let i = 0; i < PROFILE_COLORS.length && taken.has(index); i++) index = (index + 1) % PROFILE_COLORS.length;
    taken.add(index);
    return [p.id, PROFILE_COLORS[index]];
  }));
}

const colorOf = (profileId) => profileColors.get(profileId) ?? 'var(--muted)';
const profileOf = (binding) => view.profiles.find((p) => p.id === binding.profileId)
  ?? { id: binding.profileId, name: t('Unbekanntes Profil'), sso: false, hasKey: false };

function initials(name) {
  const words = name.trim().split(/\s+/).filter(Boolean);
  return (words.length > 1 ? words[0][0] + words[1][0] : (words[0] ?? '?').slice(0, 2)).toUpperCase();
}

function tierModels(p) {
  return [['Opus', p.opusModel], ['Sonnet', p.sonnetModel], ['Haiku', p.haikuModel]]
    .filter(([, model]) => model).map(([tier, model]) => `${tier} ${model}`).join(' · ');
}

function money(value) {
  return new Intl.NumberFormat(locale(), { style: 'currency', currency: 'USD' }).format(value);
}

/** LiteLLM writes Python ISO dates, e.g. `2026-11-01T00:00:00.123456+00:00`, sometimes without zone (UTC). */
function parseResetDate(text) {
  let value = text.replace(/\.\d+/, '');
  if (!/(Z|[+-]\d\d:?\d\d)$/.test(value)) value += 'Z';
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? null : date;
}

/** "in 14 Tagen", "morgen", "in 5 Stunden", "in 20 Minuten"; null once the date has passed. */
function relativeTime(date) {
  const hours = (date.getTime() - Date.now()) / 3_600_000;
  if (hours <= 0) return null;
  const format = new Intl.RelativeTimeFormat(locale(), { numeric: 'auto' });
  if (hours < 1) return format.format(Math.ceil(hours * 60), 'minute');
  return hours < 24 ? format.format(Math.ceil(hours), 'hour') : format.format(Math.round(hours / 24), 'day');
}

const dateTimeText = (date) => new Intl.DateTimeFormat(locale(), { dateStyle: 'medium', timeStyle: 'short' }).format(date);

/** What a budget bar is labeled with: its tenant, or the profile's own models when the profile shows several; nothing for a single one. */
const budgetLabel = (budget, count) => budget.tenant ?? (count > 1 ? t('Modelle des Profils') : null);

/** Budget bar: shrinks to what is left; green below 70 % used, orange below 90 %, red above. `label` names its tenant. */
function budgetMeter(budget, label) {
  const reset = budget.resetAt && parseResetDate(budget.resetAt);
  const resetText = reset && relativeTime(reset);
  const resetInfo = resetText && h('span', { class: 'muted small', title: dateTimeText(reset) }, t('Reset %@', resetText));
  const title = label && h('div', { class: 'muted small' }, label);
  if (budget.maxBudget == null) {
    return h('div', { class: 'budget' }, title, h('div', { class: 'budget-text' },
      h('strong', null, t('%@ verbraucht', money(budget.spend))), h('span', { class: 'muted' }, t('Kein Budgetlimit')),
      h('span', { class: 'spacer' }), resetInfo));
  }
  const used = budget.maxBudget > 0 ? Math.min(1, Math.max(0, budget.spend / budget.maxBudget)) : 1;
  const tone = used >= 0.9 ? 'red' : used >= 0.7 ? 'orange' : 'green';
  const percent = Math.round(used * 100);
  return h('div', { class: 'budget' },
    title,
    h('div', { class: 'budget-text' },
      h('strong', { class: tone }, t('%@ übrig', money(budget.remaining ?? 0))),
      h('span', { class: 'muted' }, t('von %@', money(budget.maxBudget))),
      h('span', { class: 'spacer' }),
      h('span', { class: 'muted small' }, t('%@ % verbraucht', percent)),
      resetInfo && h('span', { class: 'muted small' }, '·'), resetInfo),
    h('div', { class: 'meter', role: 'meter', 'aria-label': label ? `${t('Budget verbraucht')} · ${label}` : t('Budget verbraucht'), 'aria-valuemin': 0, 'aria-valuemax': 100, 'aria-valuenow': percent },
      h('div', { class: `meter-fill ${tone}`, style: `width: ${((1 - used) * 100).toFixed(1)}%` })));
}

// MARK: Claude subscription

const limitLabel = (limit) => ({ session: t('Aktuelle Sitzung'), weekly: t('Woche · alle Modelle') }[limit.kind] ?? t('Woche · %@', limit.model));

/** Colors like the budget bar: green below 70 % used, orange below 90 %, red above. */
const usageTone = (used) => (used >= 0.9 ? 'red' : used >= 0.7 ? 'orange' : 'green');

/** `fill` is the bar's share: what is used for subscription limits, what is left for budgets. */
function meterBar(used, label, fill = used) {
  const percent = Math.round(used * 100);
  return h('div', { class: 'meter', role: 'meter', 'aria-label': label, 'aria-valuemin': 0, 'aria-valuemax': 100, 'aria-valuenow': percent },
    h('div', { class: `meter-fill ${usageTone(used)}`, style: `width: ${(fill * 100).toFixed(1)}%` }));
}

/** One limit of the subscription as a bar; a limit whose reset has passed counts as free again. `compact` puts the reset below the bar. */
function limitMeter(limit, compact) {
  const reset = limit.resetsAt && parseResetDate(limit.resetsAt);
  const passed = reset && reset.getTime() <= Date.now();
  const used = passed ? 0 : Math.min(1, Math.max(0, limit.percent / 100));
  const resetText = passed ? t('Zurückgesetzt') : reset && relativeTime(reset) && t('Reset %@', relativeTime(reset));
  const resetInfo = resetText && h('span', { class: 'muted small', title: reset ? dateTimeText(reset) : null }, compact ? resetText : `· ${resetText}`);
  return h('div', { class: 'budget' },
    h('div', { class: 'budget-text' },
      h('strong', null, limitLabel(limit)),
      h('span', { class: 'spacer' }),
      h('span', { class: `small ${usageTone(used)}` }, t('%@ % verbraucht', Math.round(used * 100))),
      !compact && resetInfo),
    meterBar(used, limitLabel(limit)),
    compact && resetInfo);
}

/** The subscription's limits, or why they are missing; nothing while Claude Code is not logged in (the caller says so). */
function subscriptionUsage(compact = false) {
  const sub = view.subscription;
  if (!sub) return h('div', { class: 'muted small' }, t('Verbrauch wird geladen …'));
  if (!sub.loggedIn) return null;
  return [
    sub.limits.map((limit) => limitMeter(limit, compact)),
    !sub.limits.length && !sub.problem && h('div', { class: 'muted small' }, t('Claude meldet für diesen Tarif keine Nutzungslimits.')),
    sub.problem && h('div', { class: 'finding orange small' }, icon('warning'),
      h('span', { class: 'spacer' }, sub.problem, sub.limits.length && sub.checkedAt ? ` ${t('Stand: %@', dateTimeText(new Date(sub.checkedAt * 1000)))}` : '')),
  ];
}

function profileMenu(profile, bindings, testConnection) {
  const locked = profile.managed;
  return [
    [t('Bearbeiten'), () => profileEditor(profile), { iconName: 'pencil' }],
    profile.subscription ? null : profile.sso
      ? [profile.ssoState === 'loggedIn' ? t('Neu anmelden') : t('Anmelden'), () => call('sso_login', { profileId: profile.id }), { iconName: 'key', disabled: profile.ssoLoggingIn }]
      : [profile.hasKey ? t('Key ändern') : t('Key hinterlegen'), () => replaceKeyDialog(profile), { iconName: 'key' }],
    !profile.sso && !profile.subscription && [t('Key kopieren'), () => call('copy_key', { profileId: profile.id }), { iconName: 'copy', disabled: !profile.hasKey || !view.allowKeyExport }],
    profile.sso && [t('Abmelden'), () => call('sso_logout', { profileId: profile.id }), { iconName: 'keyOff', disabled: profile.ssoLoggingIn || profile.ssoState === 'loggedOut' }],
    [t('Verbindung prüfen'), testConnection, { iconName: 'activity', disabled: !credentialOk(profile) }],
    [t('Projekt zuordnen …'), () => addProjectDialog(profile.id), { iconName: 'folderPlus' }],
    '-',
    [t('Profil löschen'), () => deleteProfile(profile, bindings), { iconName: 'trash', danger: true, disabled: locked }],
  ];
}

function credentialRow(profile, testConnection, check) {
  const ok = credentialOk(profile);
  let text;
  if (profile.subscription) {
    const sub = view.subscription;
    text = !sub ? t('Claude-Anmeldung wird geprüft …')
      : sub.loggedIn ? (sub.plan ? t('Bei Claude angemeldet · %@', sub.plan) : t('Bei Claude angemeldet'))
        : t('Nicht bei Claude angemeldet – in Claude Code „/login“ ausführen.');
  } else if (profile.sso) {
    const expiry = profile.ssoExpiresAt && profile.ssoExpiresAt * 1000 > Date.now() ? new Date(profile.ssoExpiresAt * 1000) : null;
    text = profile.ssoLoggingIn ? t('Anmeldung läuft – bitte im Browser abschließen …')
      : ok ? (expiry ? t('Angemeldet, Token gültig bis %@', expiry.toLocaleString(locale())) : t('Angemeldet')) : ssoStateText(profile);
  } else {
    text = profile.hasKey ? `Key ${profile.keyHint ?? '••••'}` : t('Kein Key hinterlegt – Anfragen schlagen fehl');
  }
  return h('div', { class: 'key-row' },
    h('span', { class: `chip ${ok ? 'ok' : 'missing'}` }, icon(ok ? 'key' : 'keyOff'),
      h('span', { class: !profile.sso && profile.hasKey ? 'mono' : null }, text)),
    profile.ssoLoggingIn && h('span', { class: 'spinner', role: 'status', 'aria-label': t('Anmeldung läuft') }),
    h('span', { class: 'spacer' }),
    ok && button(check.checking ? t('Wird geprüft …') : t('Verbindung prüfen'), testConnection, { iconName: 'activity', disabled: check.checking }),
    !ok && !profile.subscription && (profile.sso
      ? button(t('Anmelden'), () => call('sso_login', { profileId: profile.id }), { iconName: 'key', cls: 'primary', disabled: profile.ssoLoggingIn })
      : button(t('Key hinterlegen'), () => replaceKeyDialog(profile), { iconName: 'key', cls: 'primary' })));
}

const OFFICE_APP_NAMES = { excel: 'Excel', word: 'Word', powerpoint: 'PowerPoint' };

/** Claude in Office of a profile: what is set up, or why not. Nothing for a profile without Office switched on. */
function officeRow(profile) {
  if (!profile.office?.enabled) return null;
  const office = view.office;
  const line = (tone, iconName, text) => h('div', { class: `line ${tone} small` }, icon(iconName), h('span', { class: 'selectable' }, text));
  if (view.disabled) return line('muted', 'pause', t('Claude in Office: aus, solange KeyZapper deaktiviert ist.'));
  if (office.profileId !== profile.id) {
    return line('muted', 'info', t('Claude in Office: nicht aktiv, das Profil „%@“ hat Vorrang.', office.profileId ? profileName(office.profileId) : ''));
  }
  if (office.state === 'error') return line('orange', 'warning', t('Claude in Office: %@', office.message ?? ''));
  if (office.state === 'off') return line('muted', 'info', t('Claude in Office wird eingerichtet …'));
  if (!office.apps.length) return line('muted', 'info', t('Claude in Office: Excel, Word oder PowerPoint wurde nicht gefunden oder noch nie gestartet. Nach dem ersten Start „Status aktualisieren“ wählen.'));
  return line('green', 'check', t('Claude in Office: eingerichtet für %@ – Office einmal neu starten und über „Einfügen → Meine Add-ins“ öffnen',
    office.apps.map((app) => OFFICE_APP_NAMES[app] ?? app).join(', ')));
}

function profileCard(profile) {
  const bindings = view.bindings.filter((b) => b.profileId === profile.id);
  const check = checks.get(profile.id) ?? {};
  const testConnection = async () => {
    checks.set(profile.id, { checking: true });
    renderMain();
    const result = await call('test_connection', { profileId: profile.id });
    checks.set(profile.id, { result });
    renderMain();
  };
  const menu = () => profileMenu(profile, bindings, testConnection);
  return h('section', { class: 'card profile-card', 'aria-label': profile.name, 'data-profile-id': profile.id, style: `--profile: ${colorOf(profile.id)}`,
    oncontextmenu: contextMenu(menu) },
    h('div', { class: 'card-head' },
      h('span', { class: 'avatar', 'aria-hidden': 'true' }, initials(profile.name)),
      h('div', { class: 'card-title' },
        h('h2', null, profile.name, profile.sso && h('span', { class: 'badge' }, 'SSO'), profile.subscription && h('span', { class: 'badge' }, t('Claude-Abo')),
          profile.managed && h('span', { class: 'badge' }, t('Von der IT vorgegeben'))),
        h('div', { class: 'muted small selectable' }, (profile.subscription ? t('Anmeldung von Claude Code') : profile.endpoint)
          + (profile.modelAlias ? t(' · Modell %@', profile.modelAlias) : '')),
        tierModels(profile) && h('div', { class: 'mono muted' }, tierModels(profile))),
      menuButton(t('Weitere Aktionen'), menu)),
    credentialRow(profile, testConnection, check),
    profile.budgets.map((budget) => budgetMeter(budget, budgetLabel(budget, profile.budgets.length))),
    profile.subscription && subscriptionUsage(),
    officeRow(profile),
    profile.hasKey && profile.killers.length > 0 && h('div', { class: 'line orange small' }, icon('flame'),
      t('Wird vom Budget-Killer in „%@“ verbrannt', profile.killers.join(', '))),
    check.result && h('div', { class: `check-result ${check.result.success ? 'green' : 'orange'}` },
      icon(check.result.success ? 'check' : 'warning'), h('span', { class: 'selectable' }, check.result.message)),
    h('div', { class: 'projects' },
      bindings.length > 0 && h('div', { class: 'project-list' }, bindings.map((b) => projectRow(b, profile))),
      h('button', { type: 'button', class: 'add-row', onclick: () => addProjectDialog(profile.id) }, icon('folderPlus'),
        bindings.length ? t('Weiteres Projekt zuordnen – oder Ordner hierher ziehen') : t('Projekt zuordnen – oder Ordner hierher ziehen'))));
}

// MARK: Projects

function describeHealth(health, keyPresent, { sso, subscription }) {
  if (!keyPresent && health?.state === 'active' && subscription) return t('Nicht bei Claude angemeldet – in Claude Code „/login“ ausführen.');
  if (!keyPresent && health?.state === 'active' && sso) return t('Nicht angemeldet – Claude-Anfragen schlagen fehl. Beim Profil „Anmelden“ wählen.');
  if (!keyPresent && health?.state === 'active') return t('Für das Profil ist kein Key hinterlegt – Claude-Anfragen schlagen fehl.');
  switch (health?.state) {
    case 'active': return t('Aktiv');
    case 'notApplied': return t('Nicht eingerichtet – „Erneut anwenden“ wählen.');
    case 'drifted': return t('Abweichung in %@ – „Erneut anwenden“ wählen.', health.keys.join(', '));
    case 'folderMissing': return t('Ordner nicht gefunden (verschoben oder gelöscht). Zuordnung entfernen und neu zuordnen.');
    default: return t('Status unbekannt');
  }
}

/** Pill, tone and whether "Alle reparieren" can fix it (writing the settings again). */
function projectStatus(binding) {
  const profile = profileOf(binding);
  const keyPresent = credentialOk(profile);
  const status = binding.status;
  const state = status?.health.state;
  const blocking = status?.conflicts.some((c) => c.blocking);
  const result = (symbol, tone, label, attention = true, repairable = false) => ({ symbol, tone, label, attention, repairable });
  if (view.disabled) return result('pause', 'muted', t('Pausiert'), false);
  if (!status) return result('question', 'muted', t('Wird geprüft'), false);
  if (state === 'folderMissing') return result('error', 'red', t('Ordner fehlt'));
  if (!keyPresent) return result('keyOff', 'red', profile.sso || profile.subscription ? t('Nicht angemeldet') : t('Kein Key'));
  if (blocking) return result('error', 'red', t('Konflikt'));
  if (state === 'notApplied') return result('warning', 'orange', t('Nicht eingerichtet'), true, true);
  if (state === 'drifted') return result('warning', 'orange', t('Abweichung'), true, true);
  if (!binding.mcpActive) return result('warning', 'orange', t('MCP weicht ab'), true, true);
  return result('check', 'green', t('Aktiv'), false);
}

function projectMenu(binding, folderMissing, confirmUnbind) {
  return [
    [t('Für Agenten optimieren'), () => agentsDialog(binding, profileOf(binding)), { iconName: 'sparkles', disabled: folderMissing || profileOf(binding).subscription }],
    [t('MCP-Server …'), () => projectMcpDialog(binding), { iconName: 'plug', disabled: folderMissing }],
    [revealLabel(), () => call('reveal', { path: binding.path }), { iconName: 'folder', disabled: folderMissing }],
    [t('Erneut anwenden'), () => call('reapply', { bindingId: binding.id }), { iconName: 'rotate', disabled: folderMissing || view.disabled }],
    '-',
    [t('Zuordnung entfernen'), confirmUnbind, { iconName: 'trash', danger: true }],
  ];
}

function projectRow(binding, profile) {
  const status = binding.status;
  const keyPresent = credentialOk(profile);
  const state = projectStatus(binding);
  const folderMissing = status?.health.state === 'folderMissing';
  const healthOk = status?.health.state === 'active' && keyPresent;
  const tip = view.disabled ? t('Deaktiviert – Claude nutzt hier die normale Anmeldung.')
    : !healthOk ? describeHealth(status?.health, keyPresent, profile) : status.conflicts[0]?.message ?? t('Aktiv');
  // Hidden: three clicks on the status switch the Budget-Killer on or off.
  const pill = h('span', { class: `pill ${state.tone}`, title: tip,
    onclick: (e) => { if (e.detail === 3 && !profile.sso && !profile.subscription && !view.disabled) call('toggle_pool', { bindingId: binding.id }); } },
  icon(state.symbol), state.label);
  const select = h('select', {
    'aria-label': t('Profil'), title: t('Profil für dieses Projekt wechseln'), disabled: view.disabled,
    onchange: async (e) => {
      if (await ensureLogin(e.target.value)) call('bind', { folder: binding.path, profileId: e.target.value });
      else renderMain();
    },
  }, view.profiles.map((p) => h('option', { value: p.id, selected: p.id === binding.profileId }, profileLabel(p))));
  // SSO gateways with several tenants (model IDs `<tenant>/<model>` in the user's model list): the project picks the models
  // of one tenant. A key profile has no tenants; its `<x>/<model>` IDs are providers of one gateway.
  const tenants = [...(profile.tenants ?? []), ...(binding.tenant && !profile.tenants?.includes(binding.tenant) ? [binding.tenant] : [])];
  const tenantSelect = profile.sso && (tenants.length >= 2 || binding.tenant) && h('select', {
    'aria-label': t('Tenant'), title: t('Modelle eines Tenants für dieses Projekt wählen'), disabled: view.disabled,
    onchange: (e) => call('set_binding_tenant', { bindingId: binding.id, tenant: e.target.value || null }),
  }, h('option', { value: '', selected: !binding.tenant }, t('Wie im Profil')),
  tenants.map((name) => h('option', { value: name, selected: name === binding.tenant }, name)));
  const confirmUnbind = async () => {
    if (await confirmDialog(t('Zuordnung für „%@“ entfernen?', binding.name),
      t('Nur die von der App gesetzten Einträge werden aus .claude/settings.local.json entfernt. Andere Einstellungen bleiben erhalten.'),
      t('Entfernen'))) call('unbind', { bindingId: binding.id });
  };
  return h('div', { class: 'project', style: `--profile: ${colorOf(binding.profileId)}`, oncontextmenu: contextMenu(() => projectMenu(binding, folderMissing, confirmUnbind)) },
    h('div', { class: 'project-main' },
      h('div', { class: 'project-name' },
        h('div', { class: 'line' }, h('strong', null, binding.name), pill),
        h('span', { class: 'mono', title: binding.path }, `‎${binding.displayPath}‎`)),
      select,
      tenantSelect,
      h('div', { class: 'row-actions' },
        button(t('Für Agenten optimieren'), () => agentsDialog(binding, profile),
          { iconName: 'sparkles', iconOnly: true, disabled: folderMissing || profile.subscription,
            title: profile.subscription ? t('Die KI-Analyse läuft über ein Gateway-Profil.') : t('AGENTS.md für dieses Repository per KI-Analyse erzeugen oder verbessern') }),
        button(t('MCP-Server'), () => projectMcpDialog(binding),
          { iconName: 'plug', iconOnly: true, title: t('MCP-Server für dieses Projekt auswählen'), disabled: folderMissing }),
        button(revealLabel(), () => call('reveal', { path: binding.path }), { iconName: 'folder', iconOnly: true }),
        button(t('Erneut anwenden'), () => call('reapply', { bindingId: binding.id }),
          { iconName: 'rotate', iconOnly: true, title: t('Einstellungen erneut schreiben'), disabled: folderMissing || view.disabled }),
        button(t('Entfernen'), confirmUnbind, { iconName: 'trash', iconOnly: true, title: t('Zuordnung entfernen'), cls: 'danger' }))),
    binding.mcpNames.length > 0 && h('div', { class: `note ${binding.mcpActive ? 'muted' : 'orange'}` }, icon(binding.mcpActive ? 'plug' : 'warning'),
      binding.mcpActive ? binding.mcpNames.join(', ') : t('MCP: %@ – .mcp.json weicht ab, „Erneut anwenden“ wählen.', binding.mcpNames.join(', '))),
    binding.pooled && !view.disabled && h('div', { class: 'note red' }, icon('flame'),
      t('Budget-Killer: Nach dem Budget dieses Keys werden die Keys der anderen Profile am selben Gateway verbrannt.')),
    !view.disabled && !healthOk && h('div', { class: `note ${state.tone}` }, describeHealth(status?.health, keyPresent, profile)),
    (status?.conflicts ?? []).map((c) => h('div', { class: `note ${c.blocking ? 'red' : 'orange'}` },
      icon(c.blocking ? 'error' : 'warning'), h('span', { class: 'selectable' }, c.message))));
}

function hermesMutationBlock(profileId, deactivating = false) {
  const state = hermesController.state;
  if (state.busy) return t('Warte, bis der Hermes-Vorgang abgeschlossen ist, bevor du Profile änderst.');
  if (!state.loaded || state.problem) return t('Hermes-Besitzregister nicht verfügbar. Aktualisiere Hermes, bevor du Profile änderst.');
  const selection = state.selection;
  if (!selection) return null;
  const profile = view.profiles.find((item) => item.id === selection.profileId);
  if (profileId && selection.profileId !== profileId && profile) return null;
  const name = profile?.name ?? selection.profileId;
  return deactivating
    ? t('Hermes-Verbindung vor der Deaktivierung von KeyZapper trennen. Betroffenes Ziel: %@', selection.target.home)
    : t('Hermes-Verbindung vor dem Löschen von %@ trennen. Betroffenes Ziel: %@', name, selection.target.home);
}

async function deleteProfile(profile, bindings) {
  const hermesBlock = hermesMutationBlock(profile.id);
  if (hermesBlock) {
    await confirmDialog(t('Hermes zuerst trennen'), hermesBlock, t('Verstanden'));
    return;
  }
  const message = profile.subscription
    ? (bindings.length ? t('%@ Projektzuordnung(en) werden zurückgenommen. Die Claude-Anmeldung in Claude Code bleibt bestehen.', bindings.length)
      : t('Die Claude-Anmeldung in Claude Code bleibt bestehen.'))
    : profile.sso
    ? (bindings.length ? t('Die SSO-Anmeldung wird entfernt und %@ Projektzuordnung(en) werden zurückgenommen.', bindings.length) : t('Die SSO-Anmeldung wird entfernt.'))
    : bindings.length
      ? t('Der Key wird gelöscht und %@ Projektzuordnung(en) werden zurückgenommen.', bindings.length)
      : t('Der Key wird gelöscht.');
  if (await confirmDialog(t('Profil „%@“ löschen?', profile.name), message, t('Löschen'))) {
    checks.delete(profile.id);
    call('delete_profile', { profileId: profile.id });
  }
}

// MARK: Claude settings and backup pages

/** Default profile chosen on the settings page but not applied yet; null = take the stored one. */
let settingsSelection = null;

function settingsPage() {
  if (settingsSelection === null) settingsSelection = view.globalProfileId ?? '';
  const selected = settingsSelection;
  const findings = view.globalFindings;
  const needsRepair = findings.some((f) => f.severity === 'error' || f.id.startsWith('plaintext-'));
  const health = view.globalHealth;
  const unchanged = (selected || null) === (view.globalProfileId ?? null) && health?.state === 'active';
  const healthText = !health ? null : {
    active: t('Standardprofil ist aktiv.'),
    notApplied: t('Standardprofil fehlt in der Datei – „Übernehmen“ wählen.'),
    drifted: t('Abweichung in %@ – „Übernehmen“ wählen.', health.keys.join(', ')),
    folderMissing: t('Datei nicht gefunden.'),
  }[health.state];
  const severityIcon = { error: ['error', 'red'], warning: ['warning', 'orange'], info: ['info', 'muted'] };
  return [
    pageHeader(t('Claude-Einstellungen'), t('~/.claude/settings.json prüfen, reparieren und Standardprofil setzen')),
    h('section', { class: 'card' },
      h('div', { class: 'card-head' }, h('div', { class: 'card-title' }, h('h2', null, t('Prüfung')), h('div', { class: 'mono muted small' }, view.userSettingsPath)),
        button(revealLabel(), () => call('reveal', { path: view.userSettingsFullPath }), { iconName: 'folder', iconOnly: true, disabled: !view.userSettingsExists })),
      findings.every((f) => f.severity === 'info') && h('div', { class: 'finding green' }, icon('check'), t('Keine Probleme gefunden.')),
      findings.map((f) => h('div', { class: `finding ${severityIcon[f.severity][1]}` }, icon(severityIcon[f.severity][0]), h('span', { class: 'selectable' }, f.message))),
      h('div', { class: 'line' },
        button(t('Erneut prüfen'), () => call('refresh_all'), { iconName: 'refresh' }),
        button(view.userSettingsExists ? t('Reparieren') : t('Erzeugen'), () => call('repair_user_settings'),
          { cls: 'primary', disabled: view.userSettingsExists && !needsRepair })),
      h('div', { class: 'section-note' }, t('„Reparieren“ macht die Datei gültig, entfernt Klartext-Keys und wandelt Werte in „env“ in Text um. Vorher legt KeyZapper eine Sicherung settings.json.keyzapper-<Zeit>.bak an.'))),
    h('section', { class: 'card' },
      h('h2', null, t('Standardprofil für alle übrigen Ordner')),
      field(t('Standardprofil'), h('select', { onchange: (e) => { settingsSelection = e.target.value; renderMain(); } },
        h('option', { value: '', selected: !selected }, t('Keins')),
        view.profiles.map((p) => h('option', { value: p.id, selected: p.id === selected }, profileLabel(p)))), 'c-profile'),
      healthText && h('div', { class: `finding ${health.state === 'active' ? 'green' : 'orange'}` },
        icon(health.state === 'active' ? 'check' : 'warning'), healthText),
      h('div', { class: 'line' }, h('span', { class: 'spacer' }),
        button(t('Übernehmen'), async () => { if (!selected || await ensureLogin(selected)) call('set_global_profile', { profileId: selected || null }); },
          { cls: 'primary', disabled: unchanged || (!selected && !view.globalProfileId) || view.disabled })),
      h('div', { class: 'section-note' }, t('Trägt das Profil in ~/.claude/settings.json ein. Claude Code nutzt es in allen Ordnern ohne eigene Zuordnung, statt auf einen Klartext-Key oder die normale Anmeldung zurückzufallen. Zugeordnete Projekte behalten ihr eigenes Profil.'))),
  ];
}

function backupPage() {
  return [
    pageHeader(t('Backup'), t('Profile, Projektzuordnungen und MCP-Server sichern oder auf einen anderen Rechner übertragen.')),
    h('div', { class: 'tiles' },
      h('section', { class: 'card tile' },
        h('span', { class: 'tile-icon' }, icon('upload')),
        h('h2', null, t('Backup exportieren')),
        h('p', { class: 'muted' }, view.allowKeyExport
          ? t('Enthält Profile, Projektzuordnungen und Keys – verschlüsselt mit AES-256-GCM.')
          : t('Enthält Profile und Projektzuordnungen. Keys sind laut Firmenrichtlinie vom Export ausgenommen.')),
        button(t('Backup exportieren …'), exportDialog, { cls: 'primary', disabled: !view.profiles.length })),
      h('section', { class: 'card tile' },
        h('span', { class: 'tile-icon' }, icon('download')),
        h('h2', null, t('Backup importieren')),
        h('p', { class: 'muted' }, t('Profile und Keys aus dem Backup überschreiben vorhandene mit gleicher ID. Projekte werden zugeordnet, sofern ihr Ordner auf diesem Rechner existiert.')),
        button(t('Backup importieren …'), importDialog))),
  ];
}

// MARK: Dialogs

/** Opens a modal dialog; `build(close)` returns { body, actions }. Re-rendered on view changes if `live`. */
function openDialog(title, build, { live = false, wide = false } = {}) {
  const dialog = h('dialog', { 'aria-label': title, style: wide ? 'width: min(620px, calc(100vw - 32px))' : null });
  const close = () => dialog.close();
  const render = () => {
    const focused = document.activeElement?.id;
    const { body, actions } = build(close);
    dialog.replaceChildren(h('form', { method: 'dialog', onsubmit: (e) => e.preventDefault() },
      h('div', { class: 'dialog-body' }, body), h('div', { class: 'dialog-actions' }, actions)));
    if (focused) document.getElementById(focused)?.focus();
  };
  render();
  if (live) liveDialogs.add(render);
  dialog.addEventListener('close', () => { liveDialogs.delete(render); dialog.remove(); });
  document.body.append(dialog);
  dialog.showModal();
  return { dialog, render, close };
}

function showAlert(message, onClose) {
  alertOpen = true;
  const { dialog } = openDialog(t('Fehler'), (close) => ({
    body: [h('h2', null, t('Fehler')), h('div', { class: 'alert-message selectable' }, message)],
    actions: [button('OK', close, { cls: 'primary' })],
  }));
  dialog.setAttribute('role', 'alertdialog');
  dialog.addEventListener('close', () => { alertOpen = false; onClose?.(); });
}

function confirmDialog(title, message, confirmLabel) {
  return new Promise((resolve) => {
    let confirmed = false;
    const { dialog } = openDialog(title, (close) => ({
      body: [h('h2', null, title), message instanceof Node ? message : h('p', { class: 'muted', style: 'margin:0' }, message)],
      actions: [button(t('Abbrechen'), close), button(confirmLabel, () => { confirmed = true; close(); }, { cls: 'primary danger' })],
    }));
    dialog.addEventListener('close', () => resolve(confirmed));
  });
}

function field(label, control, id) {
  control.id = id;
  return h('div', { class: 'field' }, h('label', { for: id }, label), control);
}

function input(props, onInput) {
  return h('input', { type: 'text', autocomplete: 'off', spellcheck: 'false', ...props, oninput: (e) => onInput(e.target.value) });
}

const RESERVED = ['ANTHROPIC_API_KEY', 'ANTHROPIC_AUTH_TOKEN', 'ANTHROPIC_BASE_URL', 'ANTHROPIC_MODEL', 'ANTHROPIC_DEFAULT_OPUS_MODEL',
  'ANTHROPIC_DEFAULT_SONNET_MODEL', 'ANTHROPIC_DEFAULT_HAIKU_MODEL', 'CLAUDE_CODE_USE_BEDROCK', 'CLAUDE_CODE_USE_VERTEX', 'CLAUDE_CODE_USE_FOUNDRY'];

/** Lines of the environment text that are invalid or set reserved names. */
function invalidEnvironmentLines(text) {
  return text.split(/\r?\n/).map((l) => l.trim()).filter((line) => {
    if (!line || line.startsWith('#')) return false;
    const i = line.indexOf('=');
    const name = i < 0 ? '' : line.slice(0, i).trim();
    return i < 0 || !/^[A-Za-z_][A-Za-z0-9_]*$/.test(name) || RESERVED.includes(name);
  });
}

function endpointHost(text) {
  try {
    const url = new URL(text.trim());
    const loopback = ['127.0.0.1', 'localhost', '[::1]'].includes(url.hostname);
    return (url.protocol === 'https:' || (url.protocol === 'http:' && loopback)) && url.hostname ? url.hostname.toLowerCase() : null;
  } catch { return null; }
}

function isHostAllowed(host) {
  if (!view.allowedHosts.length) return true;
  return view.allowedHosts.some((p) => (p.startsWith('*.') ? host.endsWith(p.slice(1)) : host === p));
}

const MARKETPLACE_ADDIN_ID = '4e2fd29d-dad0-40b3-af3c-a16e347a5ddc';
const GUID = /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/;

/** Functions of the add-in to switch off: separated by commas, spaces or line breaks. */
const officeFeatures = (text) => text.split(/[\s,;]+/).filter(Boolean);

/** What keeps the Office settings from being used, as [field, message]; the same checks as the app's Rust side. */
function officeProblems(o) {
  const problems = [];
  const addin = o.addinId.trim();
  if (!GUID.test(addin) || addin !== addin.toLowerCase()) problems.push(['addinId', t('Die Add-in-ID muss eine GUID in Kleinbuchstaben sein.')]);
  else if (addin === MARKETPLACE_ADDIN_ID) problems.push(['addinId', t('Die Add-in-ID darf nicht die des Marketplace-Add-ins sein.')]);
  let url = null;
  try { url = new URL(o.gatewayUrl.trim()); } catch { /* reported below */ }
  if (!url || url.protocol !== 'https:' || !url.hostname) problems.push(['gatewayUrl', t('Die Gateway-URL muss eine https-URL mit Host sein.')]);
  else if (/^(localhost|.*\.localhost|127(\.\d+){3}|\[::1\])$/.test(url.hostname.replace(/\.$/, '').toLowerCase())) problems.push(['gatewayUrl', t('Die Gateway-URL darf nicht auf diesen Rechner zeigen.')]);
  else if (url.username || url.password) problems.push(['gatewayUrl', t('Die Gateway-URL darf keine Zugangsdaten enthalten.')]);
  else if (!isHostAllowed(url.hostname.toLowerCase())) problems.push(['gatewayUrl', t('Host nicht freigegeben. Erlaubt: %@', view.allowedHosts.join(', '))]);
  if (!GUID.test(o.entraClientId.trim())) problems.push(['entraClientId', t('Die Entra-Client-ID muss eine GUID sein.')]);
  const scope = o.entraScope.trim();
  if (!scope || /\s/.test(scope)) problems.push(['entraScope', t('Der Entra-Scope darf nicht leer sein und keine Leerzeichen enthalten.')]);
  for (const feature of officeFeatures(o.disabledFeatures)) {
    if (!/^[a-z][a-z_.]*$/.test(feature)) problems.push(['disabledFeatures', t('Ungültige Funktion in der Liste der deaktivierten Funktionen: %@', feature)]);
  }
  return problems;
}

/** What keeps Claude Desktop's own sign-in from being used, as messages; the same checks as the app's Rust side. */
function desktopSsoProblems(d) {
  const problems = [];
  const https = (text) => { try { const u = new URL(text); return u.protocol === 'https:' && !!u.hostname && !u.username && !u.password; } catch { return false; } };
  const gateway = d.gatewayUrl.trim();
  if (gateway && !https(gateway)) problems.push(t('Die Gateway-URL für Claude Desktop muss eine https-URL mit Host sein.'));
  else if (gateway && !isHostAllowed(new URL(gateway).hostname.toLowerCase())) problems.push(t('Host nicht freigegeben. Erlaubt: %@', view.allowedHosts.join(', ')));
  if (!https(d.oidcIssuer.trim()) || d.oidcIssuer.includes('/.well-known/')) problems.push(t('Der OIDC-Issuer für Claude Desktop muss eine https-URL ohne /.well-known/… sein.'));
  if (!d.oidcClientId.trim() || /\s/.test(d.oidcClientId.trim())) problems.push(t('Die OIDC-Client-ID für Claude Desktop darf nicht leer sein und keine Leerzeichen enthalten.'));
  if (d.bearerTokenType === 'access_token' && !d.scopes.trim()) problems.push(t('Mit Access-Token braucht Claude Desktop die Scopes des Gateways.'));
  return problems;
}

function profileEditor(existing) {
  const s = {
    name: existing?.name ?? '', endpoint: existing?.endpoint ?? view.defaultEndpoint ?? '',
    modelAlias: existing?.modelAlias ?? view.defaultModelAlias ?? '', opusModel: existing?.opusModel ?? '',
    sonnetModel: existing?.sonnetModel ?? '', haikuModel: existing?.haikuModel ?? '',
    environmentText: existing?.environmentText ?? '', key: '', authType: existing?.subscription ? 'subscription' : existing?.sso ? 'oidc' : 'apiKey',
    oidcIssuer: existing?.oidcIssuer ?? '', oidcClientId: existing?.oidcClientId ?? '', oidcScope: existing?.oidcScope ?? '',
    oidcTokenType: existing?.oidcTokenType ?? 'access', models: [], loading: false, saving: false,
    office: {
      enabled: existing?.office?.enabled ?? false, addinId: existing?.office?.addinId ?? '', gatewayUrl: existing?.office?.gatewayUrl ?? '',
      entraClientId: existing?.office?.entraClientId ?? '', entraScope: existing?.office?.entraScope ?? '',
      disabledFeatures: (existing?.office?.disabledFeatures ?? []).join(', '),
    },
    claudeDesktop: {
      enabled: !!existing?.claudeDesktop, gatewayUrl: existing?.claudeDesktop?.gatewayUrl ?? '', oidcIssuer: existing?.claudeDesktop?.oidcIssuer ?? '',
      oidcClientId: existing?.claudeDesktop?.oidcClientId ?? '', scopes: existing?.claudeDesktop?.scopes ?? '',
      bearerTokenType: existing?.claudeDesktop?.bearerTokenType ?? 'id_token',
      credentialKind: existing?.claudeDesktop?.credentialKind === 'interactive' ? 'interactive' : 'external-idp',
    },
  };
  const managed = existing?.managed ?? false;
  const dlg = openDialog(existing ? t('Bearbeiten') : t('Neues Profil'), (close) => {
    const host = endpointHost(s.endpoint);
    const allowed = !host || isHostAllowed(host);
    const invalid = invalidEnvironmentLines(s.environmentText);
    const sso = s.authType === 'oidc';
    const subscription = s.authType === 'subscription';
    const issuerOk = !sso || /^https:\/\//i.test(s.oidcIssuer.trim()) || /^http:\/\/(localhost|127\.0\.0\.1|\[::1\])(:|\/|$)/i.test(s.oidcIssuer.trim());
    // Of a managed profile, Claude in Office and Claude Desktop (SSO) stay the user's own unless IT sets them.
    const officeLocked = managed && !!existing?.officeManaged;
    const officeShown = !subscription;
    const officeOn = officeShown && !officeLocked && s.office.enabled;
    const officeIssues = officeOn ? officeProblems(s.office) : [];
    const desktopLocked = managed && !!existing?.claudeDesktopManaged;
    const desktopShown = !subscription;
    const desktopIssues = desktopShown && !desktopLocked && s.claudeDesktop.enabled ? desktopSsoProblems(s.claudeDesktop) : [];
    const valid = s.name.trim() && (subscription || (host && allowed))
      && (sso ? issuerOk && s.oidcClientId.trim() : subscription || existing || s.key) && !invalid.length && !officeIssues.length && !desktopIssues.length;
    const update = (key) => (value) => { s[key] = value; dlg.render(); };
    const modelField = (label, key, id) => field(label, input({ value: s[key], placeholder: t('Claude-Standard'), list: 'gateway-models', disabled: managed }, update(key)), id);
    const loadModels = async () => {
      s.loading = true; dlg.render();
      const models = await call('available_models', { endpoint: s.endpoint, typedKey: s.key, profileId: existing?.id ?? null });
      s.loading = false;
      if (models?.length) {
        s.models = models;
        // Fill empty tiers with the best match, keep what the user already chose.
        const suggestion = await call('suggest_models', { models });
        for (const tier of ['opus', 'sonnet', 'haiku']) if (!s[`${tier}Model`]) s[`${tier}Model`] = suggestion?.[tier] ?? '';
      }
      dlg.render();
    };
    const save = async () => {
      s.saving = true; dlg.render();
      const result = await call('save_profile', { input: { id: existing?.id ?? null, ...s, office: subscription ? null : s.office, claudeDesktop: subscription ? null : s.claudeDesktop } });
      s.saving = false;
      if (result?.[0]) close(); else dlg.render();
    };
    return {
      body: [
        h('h2', null, existing ? existing.name : t('Neues Profil')),
        h('fieldset', { disabled: managed },
          field(t('Name'), input({ value: s.name, placeholder: subscription ? t('z. B. Claude Max') : t('z. B. Projekt Alpha') }, update('name')), 'p-name'),
          !subscription && [
            field(t('LiteLLM-Endpunkt'), input({ value: s.endpoint, placeholder: 'https://litellm.firma.intern' }, update('endpoint')), 'p-endpoint'),
            s.endpoint && !host && h('div', { class: 'field-note error-text' }, t('Bitte eine vollständige https-URL angeben (http nur für localhost).')),
            host && !allowed && h('div', { class: 'field-note error-text' }, t('Host nicht freigegeben. Erlaubt: %@', view.allowedHosts.join(', ')))]),
        managed && h('div', { class: 'section-note' }, sso ? t('Von der IT vorgegeben – möglich sind nur die Anmeldung sowie Claude in Office und Claude Desktop (SSO), soweit die IT sie nicht setzt.') : t('Von der IT vorgegeben – änderbar sind nur der Key sowie Claude in Office und Claude Desktop (SSO), soweit die IT sie nicht setzt.')),
        h('fieldset', { disabled: !!existing },
          field(t('Anmeldung'), h('select', { onchange: (e) => { s.authType = e.target.value; dlg.render(); } },
            h('option', { value: 'apiKey', selected: s.authType === 'apiKey' }, t('Key')),
            h('option', { value: 'oidc', selected: sso }, 'SSO'),
            h('option', { value: 'subscription', selected: subscription }, t('Claude-Abo (Pro, Max, Team)'))), 'p-auth')),
        subscription ? h('fieldset', null,
          h('div', { class: 'section-note' }, t('Projekte mit diesem Profil nutzen dein Claude-Abo über die Anmeldung von Claude Code – ohne Gateway und ohne Key. Einstellungen anderer Profile (Helper, Endpunkt, Modellnamen) gelten dort nicht.')),
          h('div', { class: 'section-note' }, t('Noch nicht angemeldet? In Claude Code „/login“ ausführen und das Claude-Konto wählen. KeyZapper liest die Anmeldung nur, um Tarif und Verbrauch anzuzeigen.')))
        : sso ? h('fieldset', { disabled: managed },
          field(t('Issuer'), input({ value: s.oidcIssuer, placeholder: 'https://login.microsoftonline.com/<tenant>/v2.0' }, update('oidcIssuer')), 'p-issuer'),
          s.oidcIssuer.trim() && !issuerOk && h('div', { class: 'field-note error-text' }, t('Die SSO-Adresse muss https verwenden: %@', s.oidcIssuer.trim())),
          field(t('Client-ID'), input({ value: s.oidcClientId }, update('oidcClientId')), 'p-client'),
          field(t('Scope'), input({ value: s.oidcScope, placeholder: 'openid profile offline_access' }, update('oidcScope')), 'p-scope'),
          field(t('Token-Art'), h('select', { onchange: (e) => { s.oidcTokenType = e.target.value; } },
            h('option', { value: 'access', selected: s.oidcTokenType !== 'id' }, t('Access-Token')),
            h('option', { value: 'id', selected: s.oidcTokenType === 'id' }, t('ID-Token'))), 'p-token-type'),
          h('div', { class: 'section-note' }, t('Nach dem Anlegen im Profil „Anmelden“ wählen. Tokens liegen nur lokal in einer Datei, die nur dein Benutzerkonto lesen kann. Wird Issuer oder Client-ID geändert, wirst du abgemeldet.')),
          h('div', { class: 'section-note' }, t('SSO-Profile haben keinen Key und nehmen nicht am Budget-Killer-Pool teil.')))
        : h('fieldset', null,
          field(existing ? t('Neuer Key') : t('Key'), input({ type: 'password', value: s.key,
            placeholder: existing ? t('leer lassen, um den Key zu behalten') : 'sk-…' }, update('key')), 'p-key'),
          h('div', { class: 'section-note' }, t('Der Key wird nur lokal in einer Datei gespeichert, die nur dein Benutzerkonto lesen kann – nie im Projekt.'))),
        subscription ? h('fieldset', null, h('legend', null, t('Modelle')),
          field(t('Standardmodell'), input({ value: s.modelAlias, placeholder: t('Claude-Standard'), list: 'claude-aliases' }, update('modelAlias')), 'p-alias'),
          h('datalist', { id: 'claude-aliases' }, ['opus', 'sonnet', 'haiku', 'opusplan'].map((m) => h('option', { value: m }))),
          h('div', { class: 'section-note' }, t('Optional, z. B. opus oder sonnet. Leer lassen, um den Standard deines Tarifs zu verwenden.')))
        : h('fieldset', null, h('legend', null, t('Modelle')),
          modelField('Opus', 'opusModel', 'p-opus'), modelField('Sonnet', 'sonnetModel', 'p-sonnet'),
          modelField('Haiku', 'haikuModel', 'p-haiku'), modelField(t('Standardmodell'), 'modelAlias', 'p-alias'),
          h('datalist', { id: 'gateway-models' }, s.models.map((m) => h('option', { value: m }))),
          h('div', { class: 'line' },
            button(t('Modelle vom Gateway laden'), loadModels,
              { disabled: managed || !host || !allowed || s.loading || (sso ? !existing : !existing && !s.key) }),
            s.loading && h('span', { class: 'muted' }, '…'),
            s.models.length > 0 && h('span', { class: 'muted small' }, t('%@ Modelle verfügbar', s.models.length))),
          h('div', { class: 'section-note' }, t('Eigene Modellnamen des Gateways für die Modellstufen von Claude Code. Leer lassen, um den Claude-Standard zu verwenden.'))),
        h('fieldset', { disabled: managed }, h('legend', null, t('Weitere Umgebungsvariablen')),
          h('textarea', { id: 'p-env', value: s.environmentText, spellcheck: 'false', 'aria-label': t('Weitere Umgebungsvariablen'),
            oninput: (e) => { s.environmentText = e.target.value; dlg.render(); } }),
          invalid.map((line) => h('div', { class: 'error-text' }, t('Ungültig oder nicht erlaubt: %@', line))),
          h('div', { class: 'section-note' }, t('Eine pro Zeile im Format NAME=Wert, z. B. CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS=1 für Bedrock über LiteLLM. Key, Endpunkt und Modelle werden oben gesetzt.'))),
        officeShown && h('fieldset', { disabled: officeLocked }, h('legend', null, t('Claude in Office')),
          officeLocked && h('div', { class: 'section-note' }, t('Von der IT vorgegeben')),
          h('label', { class: 'switch' },
            h('input', { type: 'checkbox', role: 'switch', checked: s.office.enabled, onchange: (e) => { s.office.enabled = e.target.checked; dlg.render(); } }),
            t('Claude in Excel, Word und PowerPoint einrichten')),
          s.office.enabled && [
            field(t('Gateway-URL'), input({ value: s.office.gatewayUrl, placeholder: 'https://ai.firma.example/api/x/desktop-gateway/office' }, (v) => { s.office.gatewayUrl = v; dlg.render(); }), 'p-office-gateway'),
            field(t('Entra-Client-ID'), input({ value: s.office.entraClientId, placeholder: '00000000-0000-0000-0000-000000000000' }, (v) => { s.office.entraClientId = v; dlg.render(); }), 'p-office-client'),
            field(t('Entra-Scope'), input({ value: s.office.entraScope, placeholder: 'api://<client-id>/access_as_user' }, (v) => { s.office.entraScope = v; dlg.render(); }), 'p-office-scope'),
            field(t('Add-in-ID'), input({ value: s.office.addinId, placeholder: '00000000-0000-0000-0000-000000000000' }, (v) => { s.office.addinId = v; dlg.render(); }), 'p-office-addin'),
            field(t('Abgeschaltete Funktionen'), input({ value: s.office.disabledFeatures, placeholder: 'web_search, thumbs' }, (v) => { s.office.disabledFeatures = v; dlg.render(); }), 'p-office-features'),
            officeIssues.filter(([key]) => key === 'disabledFeatures' || s.office[key].trim()).map(([, message]) => h('div', { class: 'error-text' }, message))],
          h('div', { class: 'section-note' }, t('Richtet Claude für Microsoft 365 als Add-in ein (Sideload) und meldet es per Entra-SSO am Gateway an; es wird kein Geheimnis gespeichert. Die Add-in-ID ist eine eigene GUID in Kleinbuchstaben, nicht die des Marketplace-Add-ins. Nach dem Einrichten Office einmal neu starten.'))),
        desktopShown && h('fieldset', { disabled: desktopLocked }, h('legend', null, t('Claude Desktop (SSO)')),
          desktopLocked && h('div', { class: 'section-note' }, t('Von der IT vorgegeben')),
          h('label', { class: 'switch' },
            h('input', { type: 'checkbox', role: 'switch', checked: s.claudeDesktop.enabled, onchange: (e) => { s.claudeDesktop.enabled = e.target.checked; dlg.render(); } }),
            t('Claude Desktop meldet sich selbst per SSO am Gateway an')),
          s.claudeDesktop.enabled && [
            field(t('Gateway-URL'), input({ value: s.claudeDesktop.gatewayUrl, placeholder: s.endpoint || t('Wie der Endpunkt des Profils') }, (v) => { s.claudeDesktop.gatewayUrl = v; dlg.render(); }), 'p-desktop-gateway'),
            field(t('OIDC-Issuer'), input({ value: s.claudeDesktop.oidcIssuer, placeholder: 'https://login.microsoftonline.com/<tenant-id>/v2.0' }, (v) => { s.claudeDesktop.oidcIssuer = v; dlg.render(); }), 'p-desktop-issuer'),
            field(t('OIDC-Client-ID'), input({ value: s.claudeDesktop.oidcClientId, placeholder: '00000000-0000-0000-0000-000000000000' }, (v) => { s.claudeDesktop.oidcClientId = v; dlg.render(); }), 'p-desktop-client'),
            field(t('Token-Art'), h('select', { onchange: (e) => { s.claudeDesktop.bearerTokenType = e.target.value; dlg.render(); } },
              h('option', { value: 'id_token', selected: s.claudeDesktop.bearerTokenType === 'id_token' }, t('ID-Token')),
              h('option', { value: 'access_token', selected: s.claudeDesktop.bearerTokenType === 'access_token' }, t('Access-Token'))), 'p-desktop-token'),
            field(t('Scopes'), input({ value: s.claudeDesktop.scopes, placeholder: t('Standard von Claude Desktop') }, (v) => { s.claudeDesktop.scopes = v; dlg.render(); }), 'p-desktop-scopes'),
            field(t('Claude-Desktop-Version'), h('select', { onchange: (e) => { s.claudeDesktop.credentialKind = e.target.value; dlg.render(); } },
              h('option', { value: 'external-idp', selected: s.claudeDesktop.credentialKind !== 'interactive' }, t('2.7032.0 oder neuer')),
              h('option', { value: 'interactive', selected: s.claudeDesktop.credentialKind === 'interactive' }, t('Älter als 2.7032.0'))), 'p-desktop-kind'),
            desktopIssues.map((message) => h('div', { class: 'error-text' }, message))],
          h('div', { class: 'section-note' }, t('Mit diesem Profil meldet sich Claude Desktop im Browser selbst beim Identity Provider an und schickt sein Token an das Gateway; KeyZapper speichert dafür weder Key noch Token. Die Modelle listet das Gateway. Ohne diesen Schalter holt Claude Desktop Key oder Token über den Helper.'))),
      ],
      actions: [button(t('Abbrechen'), close), button(existing ? t('Sichern') : t('Anlegen'), save, { cls: 'primary', disabled: !valid || s.saving })],
    };
  }, { wide: true });
  restoreCaret(dlg);
}

/** Keeps the caret at the end of the field being typed in when a dialog re-renders. */
function restoreCaret(dlg) {
  const render = dlg.render;
  dlg.render = () => {
    const active = document.activeElement;
    const position = active?.selectionStart;
    render();
    const now = document.activeElement;
    if (now && now.id === active?.id && position != null && 'setSelectionRange' in now) {
      try { now.setSelectionRange(position, position); } catch { /* not a text field */ }
    }
  };
}

function replaceKeyDialog(profile) {
  let key = '';
  const dlg = openDialog(t('Neuer Key'), (close) => ({
    body: [
      h('h2', null, profile.name),
      field(t('Neuer Key'), input({ type: 'password', value: key, placeholder: 'sk-…' }, (v) => { key = v; dlg.render(); }), 'k-key'),
      h('div', { class: 'section-note' }, t('Der bisherige Key für „%@“ wird ersetzt. Andere Profile bleiben unverändert.', profile.name)),
    ],
    actions: [button(t('Abbrechen'), close),
      button(t('Ersetzen'), async () => { close(); await call('store_key', { profileId: profile.id, key }); }, { cls: 'primary', disabled: !key.trim() })],
  }));
  restoreCaret(dlg);
  document.getElementById('k-key')?.focus();
}

function addProjectDialog(preselected, picked = null) {
  const s = { picked, profileId: preselected ?? (view.profiles.length === 1 ? view.profiles[0].id : '') };
  const dlg = openDialog(t('Projekt zuordnen'), (close) => {
    const existing = s.picked?.existingProfileId;
    return {
      body: [
        h('h2', null, t('Projekt zuordnen')),
        h('div', { class: 'field' }, h('label', null, t('Projektordner')),
          h('div', { class: 'line' },
            h('span', { class: `spacer mono ${s.picked ? '' : 'muted'}` }, s.picked?.folder ?? t('Kein Ordner gewählt')),
            button(t('Auswählen …'), async () => { const picked = await call('pick_project_folder'); if (picked) { s.picked = picked; dlg.render(); } }))),
        s.picked && s.picked.folder !== s.picked.root && h('div', { class: 'note' }, icon('info', 'accent'),
          t('Der Ordner gehört zum Git-Repository %@. Claude Code liest Projekteinstellungen nur dort; die Zuordnung gilt für das ganze Repository inkl. Unterordnern und Worktrees.', s.picked.root)),
        field(t('Profil'), h('select', { onchange: (e) => { s.profileId = e.target.value; dlg.render(); } },
          h('option', { value: '', selected: !s.profileId }, t('Bitte wählen')),
          view.profiles.map((p) => h('option', { value: p.id, selected: p.id === s.profileId }, profileLabel(p)))), 'a-profile'),
        existing && h('div', { class: 'note' }, icon('rotate', 'accent'), t('Bereits Profil „%@“ zugeordnet – wird umgestellt.', profileName(existing))),
      ],
      actions: [button(t('Abbrechen'), close),
        button(t('Zuordnen'), async () => { close(); if (await ensureLogin(s.profileId)) call('bind', { folder: s.picked.folder, profileId: s.profileId }); },
          { cls: 'primary', disabled: !s.picked || !s.profileId })],
    };
  });
}

function exportDialog() {
  const s = { password: '', confirmation: '', working: false };
  const dlg = openDialog(t('Backup exportieren'), (close) => {
    const min = view.minimumPasswordLength;
    const valid = s.password.length >= min && s.password === s.confirmation;
    return {
      body: [
        h('h2', null, t('Backup exportieren')),
        field(t('Passwort'), input({ type: 'password', value: s.password }, (v) => { s.password = v; dlg.render(); }), 'e-password'),
        field(t('Passwort wiederholen'), input({ type: 'password', value: s.confirmation }, (v) => { s.confirmation = v; dlg.render(); }), 'e-confirm'),
        s.confirmation && s.password !== s.confirmation && h('div', { class: 'field-note error-text' }, t('Die Passwörter stimmen nicht überein.')),
        h('div', { class: 'section-note' }, t('Mindestens %@ Zeichen. Ohne dieses Passwort lässt sich das Backup nicht wiederherstellen – es wird nirgends gespeichert.', min)),
        h('div', { class: 'note' }, icon('fileLock', 'accent'), view.allowKeyExport
          ? t('Enthält Profile, Projektzuordnungen und Keys – verschlüsselt mit AES-256-GCM.')
          : t('Enthält Profile und Projektzuordnungen. Keys sind laut Firmenrichtlinie vom Export ausgenommen.')),
      ],
      actions: [button(t('Abbrechen'), close, { disabled: s.working }),
        button(s.working ? '…' : t('Exportieren …'), async () => {
          s.working = true; dlg.render();
          const ok = await call('export_backup', { password: s.password });
          s.working = false;
          if (ok) close(); else dlg.render();
        }, { cls: 'primary', disabled: !valid || s.working })],
    };
  });
  restoreCaret(dlg);
}

function importDialog() {
  const s = { path: null, password: '', working: false };
  const dlg = openDialog(t('Backup importieren'), (close) => ({
    body: [
      h('h2', null, t('Backup importieren')),
      h('div', { class: 'field' }, h('label', null, t('Backup-Datei')),
        h('div', { class: 'line' },
          h('span', { class: `spacer mono ${s.path ? '' : 'muted'}` }, s.path ?? t('Keine Datei gewählt')),
          button(t('Auswählen …'), async () => { const path = await call('pick_backup_file'); if (path) { s.path = path; dlg.render(); } }))),
      field(t('Passwort'), input({ type: 'password', value: s.password }, (v) => { s.password = v; dlg.render(); }), 'i-password'),
      h('div', { class: 'section-note' }, t('Profile und Keys aus dem Backup überschreiben vorhandene mit gleicher ID. Projekte werden zugeordnet, sofern ihr Ordner auf diesem Rechner existiert.')),
    ],
    actions: [button(t('Abbrechen'), close, { disabled: s.working }),
      button(s.working ? '…' : t('Importieren'), async () => {
        s.working = true; dlg.render();
        const ok = await call('import_backup', { path: s.path, password: s.password });
        s.working = false;
        if (ok) close(); else dlg.render();
      }, { cls: 'primary', disabled: !s.path || !s.password || s.working })],
  }));
  restoreCaret(dlg);
}

const agentGoals = () => [
  ['cost', t('Kosteneffizienz'), t('Wenig Tokens und Tool-Aufrufe, kleine Diffs, günstigstes geeignetes Modell')],
  ['quality', t('Qualität & Korrektheit'), t('Gründlich prüfen, Tests und Builds, Diff vor Abschluss reviewen')],
  ['speed', t('Tempo'), t('Kurze Antworten, wenig Rückfragen, unabhängige Schritte parallel')],
  ['security', t('Sicherheit'), t('Keine Secrets, keine destruktiven Befehle ohne Rückfrage, Abhängigkeiten prüfen')],
  ['conventions', t('Team-Konventionen'), t('Stil, Commits, Branches und Pull Requests wie im Repository')],
];

/** Scans the project, lets the gateway model assess it and propose an AGENTS.md, which is saved after review. */
function agentsDialog(binding, profile) {
  const tiers = [['Opus', profile.opusModel], ['Sonnet', profile.sonnetModel], ['Haiku', profile.haikuModel], [t('Standardmodell'), profile.modelAlias]]
    .filter(([, model], i, all) => model && all.findIndex(([, m]) => m === model) === i);
  const s = { scan: null, scanError: null, goals: new Set(['cost']), language: view.language, model: tiers[0]?.[1] ?? '', notes: '',
    working: false, proposal: null, content: '' };
  const generate = async () => {
    if (!(await ensureLogin(profile.id))) return;
    s.working = true; dlg.render();
    const proposal = await call('generate_agents_file', { bindingId: binding.id,
      options: { goals: [...s.goals], language: s.language, model: s.model, notes: s.notes } });
    s.working = false;
    if (proposal) { s.proposal = proposal; s.content = proposal.content; }
    dlg.render();
  };
  const save = async (close) => {
    if (s.scan?.hasAgentsFile && !(await confirmDialog(t('AGENTS.md ersetzen?'),
      t('Die vorhandene AGENTS.md in „%@“ wird überschrieben.', binding.name), t('Ersetzen')))) return;
    const result = await call('write_agents_file', { bindingId: binding.id, content: s.content });
    if (result?.[0]) close();
  };
  const scanSummary = () => {
    if (s.scanError) return h('div', { class: 'error-text' }, s.scanError);
    if (!s.scan) return h('div', { class: 'line muted' }, h('span', { class: 'spinner' }), t('Repository wird gelesen …'));
    const languages = s.scan.languages.slice(0, 6).map(([name, count]) => `${name} ${count}`).join(' · ');
    return [
      h('div', null, h('strong', null, t('%@ Dateien', s.scan.fileCount)), languages && h('span', { class: 'muted' }, ` · ${languages}`)),
      s.scan.manifests.length > 0 && h('div', { class: 'mono muted' }, s.scan.manifests.join(', ')),
      h('div', { class: 'finding' }, icon(s.scan.agentFiles.length ? 'check' : 'info', s.scan.agentFiles.length ? 'green' : 'muted'),
        s.scan.agentFiles.length ? t('Vorhandene Agenten-Dateien: %@', s.scan.agentFiles.join(', ')) : t('Noch keine Agenten-Steuerdatei vorhanden.')),
    ];
  };
  const options = () => [
    h('fieldset', null, h('legend', null, t('Repository')), scanSummary()),
    h('fieldset', { disabled: s.working }, h('legend', null, t('Optimieren für')),
      agentGoals().map(([id, title, detail]) => h('label', { class: 'goal' },
        h('input', { type: 'checkbox', checked: s.goals.has(id), onchange: (e) => { if (e.target.checked) s.goals.add(id); else s.goals.delete(id); dlg.render(); } }),
        h('span', null, h('strong', null, title), h('span', { class: 'muted' }, ` – ${detail}`))))),
    h('fieldset', { disabled: s.working },
      field(t('Sprache der Datei'), h('select', { onchange: (e) => { s.language = e.target.value; } },
        h('option', { value: 'de', selected: s.language === 'de' }, 'Deutsch'),
        h('option', { value: 'en', selected: s.language !== 'de' }, 'English')), 'g-language'),
      field(t('Analysemodell'), h('select', { onchange: (e) => { s.model = e.target.value; } },
        tiers.map(([tier, model]) => h('option', { value: model, selected: s.model === model }, `${tier} · ${model}`)),
        h('option', { value: '', selected: !s.model }, t('Automatisch (vom Gateway)'))), 'g-model'),
      h('div', { class: 'field' }, h('label', { for: 'g-notes' }, t('Hinweise')),
        h('textarea', { id: 'g-notes', value: s.notes, placeholder: t('optional, z. B. „Nur pnpm verwenden“ oder „Migrationen nie selbst ausführen“'),
          oninput: (e) => { s.notes = e.target.value; } }))),
    !profile.sso && !profile.hasKey && h('div', { class: 'note red' }, icon('keyOff'), t('Kein Key hinterlegt – Anfragen schlagen fehl')),
    s.working ? h('div', { class: 'line muted', role: 'status' }, h('span', { class: 'spinner' }), t('Das Modell analysiert das Repository – das kann ein bis zwei Minuten dauern …'))
      : h('div', { class: 'section-note' }, t('Gesendet wird eine Zusammenfassung des Repositorys (Ordnerstruktur, README, Build- und CI-Dateien, vorhandene Agenten-Dateien) über das Gateway von „%@“. Die Kosten werden auf diesen Key gebucht; geschrieben wird erst nach deiner Prüfung.', profile.name)),
  ];
  const review = () => {
    const p = s.proposal;
    return [
      h('fieldset', null, h('legend', null, t('Bewertung')),
        h('div', { class: 'selectable' }, p.assessment || t('Keine Bewertung geliefert.')),
        h('div', { class: 'small muted' }, t('Analysiert mit %@ · %@ Eingabe- und %@ Ausgabe-Tokens', p.model,
          p.inputTokens.toLocaleString(locale()), p.outputTokens.toLocaleString(locale())))),
      p.recommendations.length > 0 && h('fieldset', null, h('legend', null, t('Modellempfehlung')),
        h('div', { class: 'recommendations' }, p.recommendations.map((r) => [
          h('strong', null, r.task),
          h('span', { class: 'mono' }, r.model, ['opus', 'sonnet', 'haiku'].includes(r.tier) && !r.model.toLowerCase().includes(r.tier) ? ` (${r.tier})` : ''),
          h('span', { class: 'muted selectable' }, r.reason)]))),
      h('fieldset', null, h('legend', null, 'AGENTS.md'),
        h('textarea', { id: 'g-content', class: 'document', value: s.content, spellcheck: 'false', 'aria-label': 'AGENTS.md',
          oninput: (e) => { s.content = e.target.value; } }),
        s.scan?.hasAgentsFile && h('div', { class: 'note orange' }, icon('warning'), t('Die vorhandene AGENTS.md wird ersetzt; ihr Inhalt ist in den Vorschlag eingeflossen.')),
        s.scan?.agentFiles.some((f) => f !== 'AGENTS.md') && h('div', { class: 'section-note' },
          t('Andere Agenten-Dateien (%@) bleiben unverändert. Doppelte Anweisungen dort entfernen, sonst lädt der Agent sie zweimal.', s.scan.agentFiles.filter((f) => f !== 'AGENTS.md').join(', ')))),
    ];
  };
  const dlg = openDialog(t('Für Agenten optimieren'), (close) => ({
    body: [h('h2', null, t('Für Agenten optimieren'), h('span', { class: 'muted' }, ` · ${binding.name}`)), s.proposal ? review() : options()],
    actions: s.proposal
      ? [button(t('Zurück'), () => { s.proposal = null; dlg.render(); }, { cls: 'left' }), button(t('Abbrechen'), close),
        button(s.scan?.hasAgentsFile ? t('AGENTS.md ersetzen') : t('AGENTS.md speichern'), () => save(close), { cls: 'primary', disabled: !s.content.trim() })]
      : [button(t('Abbrechen'), close, { disabled: s.working }),
        button(t('Analysieren & erzeugen'), generate, { cls: 'primary', disabled: !s.scan || s.working || (!profile.sso && !profile.hasKey) })],
  }), { wide: true });
  invoke('scan_repository', { bindingId: binding.id })
    .then((scan) => { s.scan = scan; }, (error) => { s.scanError = String(error); })
    .finally(() => dlg.render());
}

// MARK: MCP servers

const MCP_TRANSPORTS = () => [['stdio', t('Lokaler Prozess (stdio)')], ['http', 'HTTP'], ['sse', t('SSE (veraltet)')]];
const transportLabel = (transport) => ({ stdio: t('Lokal'), http: 'HTTP', sse: 'SSE' }[transport] ?? transport);
/** One-line summary of how a server is started or reached. */
const mcpSummary = (s) => (s.transport === 'stdio' ? [s.command, ...s.argsText.split('\n')].filter(Boolean).join(' ') : s.url);

/** Ready-made entries from the Claude Code documentation; secrets stay `${NAME}` references. */
const MCP_TEMPLATES = () => {
  const browserLogin = t('Anmeldung beim ersten Aufruf von „/mcp“ in Claude Code im Browser.');
  return [
    { name: 'github', title: 'GitHub', color: '#24292f', description: t('Issues, Pull Requests und Code'), transport: 'http',
      url: 'https://api.githubcopilot.com/mcp/', headersText: 'Authorization: Bearer ${GITHUB_PAT}',
      note: t('Einen GitHub Personal Access Token als Umgebungsvariable GITHUB_PAT setzen.') },
    { name: 'notion', title: 'Notion', color: '#191919', description: t('Seiten und Datenbanken'), transport: 'http', url: 'https://mcp.notion.com/mcp', note: browserLogin },
    { name: 'sentry', title: 'Sentry', color: '#362d59', description: t('Fehler und Performance'), transport: 'http', url: 'https://mcp.sentry.dev/mcp', note: browserLogin },
    { name: 'asana', title: 'Asana', color: '#f06a6a', description: t('Aufgaben und Projekte'), transport: 'sse', url: 'https://mcp.asana.com/sse', note: browserLogin },
    { name: 'stripe', title: 'Stripe', color: '#635bff', description: t('Zahlungen und Kunden'), transport: 'http', url: 'https://mcp.stripe.com', note: browserLogin },
    { name: 'playwright', title: 'Playwright', color: '#2ead33', description: t('Browser steuern und testen'), transport: 'stdio',
      command: 'npx', argsText: '@playwright/mcp@latest', note: t('Startet einen lokalen Browser. Benötigt Node.js.') },
    { name: 'postgres', title: 'PostgreSQL', color: '#336791', description: t('Datenbank abfragen'), transport: 'stdio',
      command: 'npx', argsText: '-y\n@bytebase/dbhub\n--dsn\n${DATABASE_URL}',
      note: t('Die Verbindungs-URL als Umgebungsvariable DATABASE_URL setzen, am besten mit einem Nur-Lese-Benutzer. Benötigt Node.js.') },
    { name: 'airtable', title: 'Airtable', color: '#2d7ff9', description: t('Tabellen lesen und schreiben'), transport: 'stdio',
      command: 'npx', argsText: '-y\nairtable-mcp-server', envText: 'AIRTABLE_API_KEY=${AIRTABLE_API_KEY}',
      note: t('Einen Airtable-API-Key als Umgebungsvariable AIRTABLE_API_KEY setzen. Benötigt Node.js.') },
  ];
};

/** Switches one server on or off for one project; the view brings back the old state if writing fails. */
function setProjectServer(binding, serverId, on) {
  const ids = binding.mcpServerIds.filter((id) => id !== serverId);
  if (on) ids.push(serverId);
  return call('set_project_mcp_servers', { bindingId: binding.id, serverIds: ids });
}

function toggleChip(label, checked, onChange, { disabled, color } = {}) {
  return h('label', { class: `toggle-chip${checked ? ' on' : ''}${disabled ? ' disabled' : ''}`, style: color ? `--profile: ${color}` : null },
    h('input', { type: 'checkbox', role: 'switch', checked, disabled, onchange: (e) => onChange(e.target.checked) }),
    h('span', { class: 'dot', 'aria-hidden': 'true' }), label);
}

function mcpServerCard(server) {
  const remove = async () => {
    const message = server.usedBy.length ? t('Der Server wird aus %@ Projekt(en) entfernt.', server.usedBy.length) : t('Der Server wird gelöscht.');
    if (await confirmDialog(t('MCP-Server „%@“ löschen?', server.name), message, t('Löschen'))) call('delete_mcp_server', { serverId: server.id });
  };
  const template = MCP_TEMPLATES().find((tpl) => tpl.name === server.name);
  const menu = () => [[t('Bearbeiten'), () => mcpServerEditor(server), { iconName: 'pencil' }], '-', [t('Löschen'), remove, { iconName: 'trash', danger: true }]];
  return h('section', { class: 'card', 'aria-label': server.name, oncontextmenu: server.managed ? null : contextMenu(menu) },
    h('div', { class: 'card-head' },
      h('span', { class: 'avatar', style: `--profile: ${template?.color ?? 'var(--accent-fill)'}`, 'aria-hidden': 'true' }, (template?.title ?? server.name).slice(0, 1).toUpperCase()),
      h('div', { class: 'card-title' },
        h('h2', null, server.name, h('span', { class: 'badge' }, transportLabel(server.transport)), server.oauthClientId && h('span', { class: 'badge' }, 'OAuth'),
          server.managed && h('span', { class: 'badge', title: t('Von der IT über die zentrale Liste bereitgestellt; hier nicht änderbar.') }, t('Von IT verwaltet'))),
        h('div', { class: 'mono muted selectable', title: mcpSummary(server) }, mcpSummary(server))),
      !server.managed && button(t('Bearbeiten'), () => mcpServerEditor(server), { iconName: 'pencil', iconOnly: true }),
      !server.managed && menuButton(t('Weitere Aktionen'), menu)),
    view.bindings.length
      ? h('div', { class: 'toggle-row' }, h('span', { class: 'muted small' }, t('Aktiv in')),
        view.bindings.map((b) => toggleChip(b.name, b.mcpServerIds.includes(server.id), (on) => setProjectServer(b, server.id, on),
          { disabled: b.status?.health.state === 'folderMissing', color: colorOf(b.profileId) })))
      : h('div', { class: 'muted small' }, t('Noch keine Projekte zugeordnet.')));
}

function mcpPage() {
  const installed = new Set(view.mcpServers.map((s) => s.name));
  const templates = MCP_TEMPLATES().filter((tpl) => !installed.has(tpl.name));
  return [
    pageHeader(t('MCP-Server'), t('Zusätzliche Werkzeuge für Claude Code, z. B. GitHub, Datenbanken oder ein Browser. Je Projekt einschalten – KeyZapper schreibt sie in die .mcp.json des Repositorys und schaltet sie frei.'),
      button(t('Neuer Server'), () => mcpServerEditor(null), { iconName: 'plus', cls: 'primary' })),
    mcpCatalogCard(),
    view.mcpServers.map(mcpServerCard),
    templates.length > 0 && h('section', { class: 'templates' },
      h('h3', null, view.mcpServers.length ? t('Weitere Vorlagen') : t('Mit einer Vorlage starten')),
      h('div', { class: 'template-grid' }, templates.map((tpl) => h('button', { type: 'button', class: 'template', onclick: () => mcpServerEditor(null, tpl) },
        h('span', { class: 'avatar', style: `--profile: ${tpl.color}`, 'aria-hidden': 'true' }, tpl.title.slice(0, 1)),
        h('span', { class: 'template-text' }, h('strong', null, tpl.title), h('span', { class: 'muted small' }, tpl.description)),
        h('span', { class: 'badge' }, transportLabel(tpl.transport)))))),
  ];
}

/** State of the central MCP server list IT sets with `ManagedMcpServersURL`. */
function mcpCatalogCard() {
  const c = view.mcpCatalog;
  if (!c) return null;
  const fetched = c.fetchedAt ? dateTimeText(new Date(c.fetchedAt * 1000)) : null;
  const status = c.problem ? h('div', { class: 'finding orange' }, icon('warning'), h('span', { class: 'selectable' }, c.problem))
    : fetched ? h('div', { class: 'finding green' }, icon('check'), t('%@ Server, Stand %@.', c.count, fetched))
      : h('div', { class: 'finding muted' }, icon('info'), c.loading ? t('Wird geladen …') : t('Noch nicht geladen.'));
  return h('section', { class: 'card' },
    h('div', { class: 'card-head' },
      h('span', { class: 'tile-icon', 'aria-hidden': 'true' }, icon('server')),
      h('div', { class: 'card-title' }, h('h2', null, t('Zentrale MCP-Liste der IT')), h('div', { class: 'mono muted small selectable', title: c.url }, c.url)),
      button(t('Neu laden'), () => call('reload_mcp_catalog'), { iconName: 'refresh', iconOnly: true, disabled: c.loading })),
    status,
    fetched && c.problem && h('div', { class: 'muted small' }, t('Es gilt weiter die Liste vom %@.', fetched)),
    c.skipped.length > 0 && h('div', { class: 'finding orange' }, icon('warning'), t('Unvollständig oder ungültig, daher ausgelassen: %@', c.skipped.join(', '))),
    c.shadowed.length > 0 && h('div', { class: 'finding orange' }, icon('warning'), t('Ausgelassen, weil ein eigener Server so heißt: %@', c.shadowed.join(', '))),
    h('div', { class: 'section-note' }, t('Diese Server stellt die IT bereit und pflegt sie zentral; KeyZapper lädt die Liste beim Start und alle 10 Minuten. Je Projekt einschalten wie eigene Server.')));
}

/** Lines that are not `NAME=value` (environment) or `Name: value` (headers). */
function invalidPairLines(text, separator) {
  const name = separator === '=' ? /^[A-Za-z_][A-Za-z0-9_]*$/ : /^[A-Za-z0-9-]+$/;
  return text.split(/\r?\n/).map((l) => l.trim()).filter((line) => {
    if (!line || line.startsWith('#')) return false;
    const i = line.indexOf(separator);
    return i < 0 || !name.test(line.slice(0, i).trim());
  });
}

/** Editor for a new server (optionally prefilled from a template) or an existing one. */
function mcpServerEditor(existing, template = null) {
  const base = existing ?? template ?? {};
  const s = { name: base.name ?? '', transport: base.transport ?? 'http', command: base.command ?? '', argsText: base.argsText ?? '',
    envText: base.envText ?? '', url: base.url ?? '', headersText: base.headersText ?? '', oauthClientId: base.oauthClientId ?? '',
    oauthCallbackPort: base.oauthCallbackPort ?? '', oauthScopes: base.oauthScopes ?? '', saving: false };
  const dlg = openDialog(existing ? t('Bearbeiten') : template?.title ?? t('Neuer Server'), (close) => {
    const update = (key) => (value) => { s[key] = value; dlg.render(); };
    const nameOk = /^[A-Za-z0-9_.-]+$/.test(s.name.trim());
    const duplicate = view.mcpServers.some((o) => o.name === s.name.trim() && o.id !== existing?.id);
    const stdio = s.transport === 'stdio';
    const invalid = stdio ? invalidPairLines(s.envText, '=') : invalidPairLines(s.headersText, ':');
    const portOk = !s.oauthCallbackPort.trim() || /^[1-9][0-9]{0,4}$/.test(s.oauthCallbackPort.trim()) && Number(s.oauthCallbackPort) <= 65535;
    const valid = nameOk && !duplicate && (stdio ? s.command.trim() : endpointHost(s.url) && portOk) && !invalid.length;
    const save = async () => {
      s.saving = true; dlg.render();
      const result = await call('save_mcp_server', { input: { id: existing?.id ?? null, ...s } });
      s.saving = false;
      if (result?.[0]) close(); else dlg.render();
    };
    return {
      body: [
        h('h2', null, existing ? existing.name : template ? t('%@ einrichten', template.title) : t('Neuer Server')),
        template?.note && h('div', { class: 'note' }, icon('info', 'accent'), template.note),
        h('fieldset', null,
          field(t('Name'), input({ value: s.name, placeholder: 'github' }, update('name')), 'm-name'),
          s.name.trim() && !nameOk && h('div', { class: 'field-note error-text' }, t('Nur Buchstaben, Ziffern, „-“, „_“ und „.“')),
          duplicate && h('div', { class: 'field-note error-text' }, t('Es gibt bereits einen MCP-Server namens „%@“.', s.name.trim())),
          field(t('Art'), h('select', { onchange: (e) => { s.transport = e.target.value; dlg.render(); } },
            MCP_TRANSPORTS().map(([value, label]) => h('option', { value, selected: value === s.transport }, label))), 'm-transport')),
        stdio ? h('fieldset', null,
          field(t('Befehl'), input({ value: s.command, placeholder: 'npx' }, update('command')), 'm-command'),
          h('div', { class: 'field' }, h('label', { for: 'm-args' }, t('Argumente')),
            h('textarea', { id: 'm-args', value: s.argsText, spellcheck: 'false', placeholder: '-y\n@modelcontextprotocol/server-github', oninput: (e) => { s.argsText = e.target.value; dlg.render(); } })),
          h('div', { class: 'field-note section-note' }, t('Eins pro Zeile.')),
          h('div', { class: 'field' }, h('label', { for: 'm-env' }, t('Umgebungsvariablen')),
            h('textarea', { id: 'm-env', value: s.envText, spellcheck: 'false', placeholder: 'GITHUB_TOKEN=${GITHUB_TOKEN}', oninput: (e) => { s.envText = e.target.value; dlg.render(); } })),
          invalid.map((line) => h('div', { class: 'field-note error-text' }, t('Ungültig oder nicht erlaubt: %@', line))),
          h('div', { class: 'field-note section-note' }, t('Eine pro Zeile im Format NAME=Wert.')))
        : h('fieldset', null,
          field('URL', input({ value: s.url, placeholder: 'https://mcp.example.com/mcp' }, update('url')), 'm-url'),
          s.url.trim() && !endpointHost(s.url) && h('div', { class: 'field-note error-text' }, t('Bitte eine vollständige https-URL angeben (http nur für localhost).')),
          h('div', { class: 'field' }, h('label', { for: 'm-headers' }, t('Header')),
            h('textarea', { id: 'm-headers', value: s.headersText, spellcheck: 'false', placeholder: 'Authorization: Bearer ${API_TOKEN}', oninput: (e) => { s.headersText = e.target.value; dlg.render(); } })),
          invalid.map((line) => h('div', { class: 'field-note error-text' }, t('Ungültig oder nicht erlaubt: %@', line))),
          h('div', { class: 'field-note section-note' }, t('Eine pro Zeile im Format Name: Wert.'))),
        !stdio && h('fieldset', null, h('legend', null, t('Anmeldung per OAuth / SSO')),
          field(t('Client-ID'), input({ value: s.oauthClientId, placeholder: t('öffentlicher OAuth-Client, kein Secret') }, update('oauthClientId')), 'm-client'),
          field(t('Callback-Port'), input({ value: s.oauthCallbackPort, placeholder: '8976', inputmode: 'numeric' }, update('oauthCallbackPort')), 'm-port'),
          s.oauthCallbackPort.trim() && !portOk && h('div', { class: 'field-note error-text' }, t('Der Callback-Port muss eine Zahl zwischen 1 und 65535 sein.')),
          s.oauthCallbackPort.trim() && portOk && h('div', { class: 'field-note section-note mono' }, `http://localhost:${s.oauthCallbackPort.trim()}/callback`),
          field(t('Scopes'), input({ value: s.oauthScopes, placeholder: 'mcp.read' }, update('oauthScopes')), 'm-scopes'),
          h('div', { class: 'section-note' }, t('Für Server, die OAuth nach MCP-Spezifikation anbieten (z. B. Weave-MCP über Entra ID oder Keycloak). Claude Code findet den Identity Provider über den Server, öffnet beim ersten Aufruf von „/mcp“ den Browser-Login (PKCE) und bewahrt die Tokens selbst auf. Beim Provider muss die Redirect-URI http://localhost:<Port>/callback für den Client erlaubt sein; für Entra die Scopes als api://<API-Client-ID>/mcp.read angeben.'))),
        h('div', { class: 'section-note' }, t('Secrets als ${NAME} eintragen – Claude Code setzt sie beim Start aus der Umgebung ein. Die .mcp.json wird nicht eingecheckt, liegt aber im Projektordner.')),
      ],
      actions: [button(t('Abbrechen'), close), button(existing ? t('Sichern') : t('Anlegen'), save, { cls: 'primary', disabled: !valid || s.saving })],
    };
  }, { wide: true });
  restoreCaret(dlg);
  document.getElementById('m-name')?.focus();
}

/** Switches the configured servers on or off for one project; each switch is written right away. */
function projectMcpDialog(binding) {
  openDialog(t('MCP-Server für „%@“', binding.name), (close) => {
    const current = view.bindings.find((b) => b.id === binding.id) ?? binding;
    return {
      body: [
        h('h2', null, t('MCP-Server für „%@“', binding.name)),
        !view.mcpServers.length ? h('div', { class: 'muted' }, t('Noch keine MCP-Server eingerichtet.'))
          : h('div', { class: 'switch-list' }, view.mcpServers.map((s) => h('label', { class: 'switch-row' },
            h('span', { class: 'switch-text' }, h('strong', null, s.name, h('span', { class: 'badge' }, transportLabel(s.transport)), s.managed && h('span', { class: 'badge' }, t('Von IT verwaltet'))),
              h('span', { class: 'mono muted small' }, mcpSummary(s))),
            h('input', { type: 'checkbox', role: 'switch', checked: current.mcpServerIds.includes(s.id),
              onchange: (e) => setProjectServer(current, s.id, e.target.checked) })))),
        h('div', { class: 'section-note' },
          t('KeyZapper schreibt die gewählten Server in die .mcp.json des Repositorys (nicht eingecheckt) und schaltet sie in .claude/settings.local.json frei. Claude Code lädt sie in diesem Repository dann automatisch.')),
      ],
      actions: [button(t('MCP-Server verwalten …'), () => { close(); navigate('mcp'); }, { cls: 'left' }), button(t('Fertig'), close, { cls: 'primary' })],
    };
  }, { live: true });
}

// MARK: Claude tools

const TOOLS = () => [
  { tool: 'cli', title: 'Claude Code', subtitle: t('Für das Terminal, auch von IntelliJ IDEA genutzt'), symbol: 'terminal', color: '#d97757' },
  { tool: 'vsCode', title: t('Claude Code für VS Code'), subtitle: t('Erweiterung aus dem VS Code Marketplace'), symbol: 'plug', color: '#0066b8',
    missing: t('Visual Studio Code wurde nicht gefunden.') },
  { tool: 'intellij', title: t('Claude Code für IntelliJ IDEA'), subtitle: t('Plugin aus dem JetBrains Marketplace'), symbol: 'plug', color: '#fe2857',
    missing: t('IntelliJ IDEA wurde nicht gefunden.') },
  { tool: 'desktop', title: 'Claude Desktop', subtitle: t('App für Chat, Cowork und Code'), symbol: 'bolt', color: '#c96442' },
];

let desktopSelection = null;

function toolCard(info, status, ...extra) {
  const installing = view.installingTool === info.tool;
  const installed = status?.version != null;
  const text = !status ? t('Wird geprüft …')
    : installed ? (status.version ? t('Installiert · Version %@', status.version) : t('Installiert'))
      : !status.hostFound ? info.missing : t('Nicht installiert');
  return h('section', { class: 'card', 'aria-label': info.title },
    h('div', { class: 'card-head' },
      h('span', { class: 'avatar', style: `--profile: ${info.color}`, 'aria-hidden': 'true' }, icon(info.symbol)),
      h('div', { class: 'card-title' }, h('h2', null, info.title), h('div', { class: 'muted small' }, info.subtitle)),
      status && !installed && status.hostFound && button(installing ? t('Wird installiert …') : t('Installieren'),
        () => call('install_tool', { tool: info.tool }), { iconName: 'download', cls: 'primary', disabled: !!view.installingTool })),
    h('div', { class: `finding ${installed ? 'green' : 'muted'}` }, icon(installed ? 'check' : 'info'), text,
      installing && h('span', { class: 'spinner', role: 'status', 'aria-label': t('Wird installiert …') })),
    extra);
}

/** Claude Desktop's third-party mode with a KeyZapper profile, or its normal sign-in. */
function claudeDesktopSettings() {
  const current = view.claudeDesktop.profileId ?? '';
  if (desktopSelection === null) desktopSelection = current;
  const selected = desktopSelection;
  const configured = view.profiles.find((p) => p.id === view.claudeDesktop.profileId);
  return h('fieldset', null, h('legend', null, t('Anmeldung')),
    field(t('Profil'), h('select', { onchange: (e) => { desktopSelection = e.target.value; renderMain(); } },
      h('option', { value: '', selected: !selected }, t('Normale Anmeldung (claude.ai)')),
      view.profiles.filter((p) => !p.subscription).map((p) => h('option', { value: p.id, selected: p.id === selected }, profileLabel(p)))), 't-desktop-profile'),
    configured && view.claudeDesktop.applied && h('div', { class: 'finding green' }, icon('check'), t('Claude Desktop nutzt „%@“ über das Gateway.', configured.name)),
    configured && !view.claudeDesktop.applied && h('div', { class: 'finding orange' }, icon('warning'),
      t('Für „%@“ eingerichtet, in Claude Desktop ist aber eine andere Konfiguration ausgewählt.', configured.name)),
    view.claudeDesktop.managed && h('div', { class: 'finding orange' }, icon('warning'),
      t('Claude Desktop wird auf diesem Rechner von der IT konfiguriert; KeyZapper richtet es deshalb nicht ein.')),
    view.claudeDesktop.auto && !view.claudeDesktop.managed && h('div', { class: 'finding' }, icon('info', 'muted'),
      t('Von der IT eingerichtet: KeyZapper schreibt diese Konfiguration beim Start und beim Aktualisieren wieder, falls sie fehlt oder geändert wurde.')),
    view.claudeDesktop.problem && h('div', { class: 'finding orange' }, icon('warning'), view.claudeDesktop.problem),
    view.claudeDesktop.installError && h('div', { class: 'finding orange' }, icon('warning'), view.claudeDesktop.installError),
    h('div', { class: 'line' }, h('span', { class: 'spacer' }),
      button(t('Übernehmen'), async () => {
        const signsInItself = view.profiles.find((p) => p.id === selected)?.claudeDesktop;
        if (!selected || signsInItself || await ensureLogin(selected)) call('set_claude_desktop_profile', { profileId: selected || null });
      }, { cls: 'primary', disabled: (selected === current && (!selected || view.claudeDesktop.applied)) || (!!selected && view.claudeDesktop.managed) })),
    h('div', { class: 'section-note' }, t('KeyZapper legt dafür eine Konfigurationsdatei für den Drittanbieter-Modus von Claude Desktop an: Anfragen gehen an das Gateway des Profils, Key oder SSO-Token holt Claude Desktop über den Helper – wie Claude Code –, oder es meldet sich selbst an, wenn das Profil „Claude Desktop (SSO)“ eingerichtet hat. Danach Claude Desktop beenden, neu öffnen und auf dem Anmeldebildschirm den Start mit dieser Konfiguration wählen.')));
}

function toolsPage() {
  const status = (tool) => view.tools?.find((s) => s.tool === tool);
  const [cli, vscode, intellij, desktop] = TOOLS();
  const cliPath = status('cli')?.path;
  const globalProfile = view.profiles.find((p) => p.id === view.globalProfileId);
  return [
    pageHeader(t('Claude-Tools'), t('Claude Code und Claude Desktop aus den offiziellen Quellen von Anthropic installieren – ohne Administratorrechte – und mit deinen Profilen verbinden.'),
      button(t('Erneut prüfen'), () => call('load_tools'), { iconName: 'refresh', iconOnly: true })),
    toolCard(cli, status('cli'),
      cliPath && h('div', { class: 'mono muted small selectable' }, cliPath),
      h('div', { class: 'section-note' }, t('Offizieller Installer von Anthropic: installiert in dein Benutzerverzeichnis und aktualisiert sich selbst.'))),
    toolCard(vscode, status('vsCode'),
      status('vsCode')?.version != null && h('div', { class: 'line' },
        h('span', { class: 'section-note spacer' }, t('Für Key- und SSO-Profile in VS Code „Disable Login Prompt“ einschalten, damit die Erweiterung keine Claude-Anmeldung verlangt. Mit einem Claude-Abo ausgeschaltet lassen.')),
        button(t('Einstellung öffnen'), () => call('open_vscode_setting')))),
    toolCard(intellij, status('intellij'),
      h('div', { class: 'section-note' }, t('Das Plugin startet claude im Terminal von IntelliJ – dafür zuerst Claude Code installieren. IntelliJ IDEA vor der Installation beenden.')),
      cliPath && status('intellij')?.version != null && h('div', { class: 'section-note' },
        t('Findet IntelliJ claude nicht, unter Settings › Tools › Claude Code [Beta] als „Claude command“ eintragen: %@', cliPath))),
    toolCard(desktop, status('desktop'), claudeDesktopSettings()),
    h('section', { class: 'card' },
      h('h2', null, t('Anmeldung in Claude Code')),
      h('p', { class: 'muted', style: 'margin:0' }, t('Claude Code im Terminal, in VS Code und in IntelliJ nutzt in zugeordneten Projekten deren Profil, in allen übrigen Ordnern das Standardprofil – mit Key, SSO oder Claude-Abo, ohne eigene Anmeldung.')),
      h('div', { class: 'line' },
        h('span', { class: `finding ${globalProfile ? 'green' : 'muted'} spacer` }, icon(globalProfile ? 'check' : 'info'),
          globalProfile ? t('Standardprofil: %@', globalProfile.name) : t('Kein Standardprofil gesetzt.')),
        button(t('Standardprofil festlegen …'), () => navigate('settings')))),
  ];
}

/** Once after an update: the highlights of the releases since the version last seen. */
function whatsNewDialog() {
  whatsNewShown = true;
  const releases = view.whatsNew;
  const { dialog } = openDialog(t('Neu in KeyZapper %@', view.appVersion), (close) => ({
    body: [
      h('h2', null, t('Neu in KeyZapper %@', view.appVersion)),
      releases.map((r) => h('fieldset', null, releases.length > 1 && h('legend', null, `KeyZapper ${r.version}`),
        h('ul', { class: 'whats-new' }, r.items.map((item) => h('li', null, item))))),
    ],
    actions: [
      button(t('Versionshinweise'), () => call('open_url', { url: `${view.projectUrl}/releases/tag/${view.appVersion}` }), { cls: 'left' }),
      button(t('Weiter'), close, { cls: 'primary' }),
    ],
  }), { wide: true });
  dialog.addEventListener('close', () => call('dismiss_whats_new'));
}

function howItWorksDialog() {
  const node = (symbol, title, subtitle) => h('div', { class: 'flow-node' }, icon(symbol), h('strong', null, title), h('span', { class: 'mono' }, subtitle));
  const arrow = () => icon('arrow', 'muted');
  const step = (n, title, text) => h('div', { class: 'step' }, h('span', { class: 'step-number' }, n),
    h('div', null, h('strong', null, title), h('div', { class: 'muted' }, text)));
  const hint = (symbol, text) => h('div', { class: 'finding' }, icon(symbol, 'muted'), text);
  openDialog(t('So funktioniert KeyZapper'), (close) => ({
    body: [
      h('h2', null, t('So funktioniert KeyZapper')),
      h('strong', { class: 'green' }, t('Einmal zuordnen – danach musst du nie wieder Keys wechseln.')),
      h('div', { class: 'flow' },
        node('terminal', 'Claude Code', t('im Projekt')), arrow(),
        node('bolt', 'keyzapper-helper', '--profile'), arrow(),
        node('fileLock', t('Key-Datei'), t('Projekt-Key')), arrow(),
        node('server', 'LiteLLM', t('→ Bedrock'))),
      step(1, t('Einmal einrichten'), t('Profil mit Key anlegen und Projektordner zuordnen. Der Key liegt nur lokal in deinem Benutzerkonto, nie im Projekt.')),
      step(2, t('Verweis statt Key'), t('KeyZapper schreibt in den Projektordner .claude/settings.local.json – darin steht nur, welcher Helper und welches Profil gilt, nie der Key selbst.')),
      step(3, t('Claude holt den Key selbst'), t('Startet Claude Code in VS Code, IntelliJ oder im Terminal, ruft es den Helper auf. Der liefert genau den Key dieses Projekts – auch wenn KeyZapper geschlossen ist.')),
      step(4, t('Mehrere Projekte parallel'), t('Jedes Projekt nutzt seinen eigenen Key, gleichzeitig und unabhängig voneinander.')),
      h('fieldset', null, h('legend', null, t('Gut zu wissen')),
        hint('rotate', t('Nach dem Zuordnen oder einem Profilwechsel die Claude-Sitzung im Projekt einmal neu starten.')),
        hint('key', t('Neuer Key? Einmal „Ändern“ – alle zugeordneten Projekte nutzen ihn automatisch. Neue Sitzungen sofort, laufende nach spätestens 5 Minuten.')),
        hint('folder', t('Die Zuordnung gilt für das ganze Repository samt Unterordnern und Worktrees. Außerhalb zugeordneter Ordner nutzt Claude die normale Anmeldung.')),
        hint('shield', t('Fehlt ein Key, schlagen Anfragen fehl. Es wird nie ein anderer Key verwendet.')),
        hint('plug', t('MCP-Server einmal unter „MCP-Server“ einrichten und je Projekt einschalten – Claude Code lädt sie dort automatisch.')),
        hint('bolt', isMac() ? t('Schließt du das Fenster, läuft KeyZapper in der Menüleiste weiter: Klick aufs Symbol zeigt Abo-Verbrauch und Budgets, Rechtsklick wechselt Profile je Projekt.')
          : t('Schließt du das Fenster, läuft KeyZapper im Infobereich der Taskleiste weiter: Klick aufs Symbol zeigt Abo-Verbrauch und Budgets, Rechtsklick wechselt Profile je Projekt.')),
        hint('sparkles', t('Mit einem Profil vom Typ „Claude-Abo“ nutzen Projekte dein Claude-Abo statt eines Gateway-Keys.'))),
    ],
    actions: [button(t('Verstanden'), close, { cls: 'primary' })],
  }), { wide: true });
}

// MARK: Tray popover

const isPopover = () => document.body.classList.contains('popover-window');
let popoverHeight = 0;
let popoverRefreshing = false;

async function refreshPopover() {
  popoverRefreshing = true;
  renderPopover();
  await call('refresh_all');
  setTimeout(() => { popoverRefreshing = false; renderPopover(); }, 800);
}

/** A gateway profile in the popover: what is left of its budget (one row per tenant), or why that is unknown. */
function popoverProfile(profile) {
  const name = (label) => h('span', { class: 'popover-name' }, h('span', { class: 'dot', style: `background: ${colorOf(profile.id)}` }), profile.name,
    label && h('span', { class: 'muted small' }, `· ${label}`));
  const status = (cls, text, label) => h('div', { class: 'budget-text' }, name(label), h('span', { class: 'spacer' }), h('span', { class: `small ${cls}` }, text));
  if (!credentialOk(profile)) return status('red', profile.sso ? ssoStateText(profile) : t('Kein Key'));
  if (profile.budgets.length === 0) return profile.sso ? status('green', t('Angemeldet')) : status('muted', t('Budget nicht verfügbar'));
  return profile.budgets.map((budget) => {
    const label = budgetLabel(budget, profile.budgets.length);
    if (budget.maxBudget == null) return status('muted', t('%@ verbraucht', money(budget.spend)), label);
    const used = budget.maxBudget > 0 ? Math.min(1, Math.max(0, budget.spend / budget.maxBudget)) : 1;
    const reset = budget.resetAt && parseResetDate(budget.resetAt);
    const resetText = reset && relativeTime(reset);
    return h('div', { class: 'popover-row' },
      h('div', { class: 'budget-text' }, name(label), h('span', { class: 'spacer' }), h('strong', { class: usageTone(used) }, t('%@ übrig', money(budget.remaining ?? 0)))),
      meterBar(used, t('Budget verbraucht'), 1 - used),
      h('div', { class: 'budget-text muted small' }, t('%@ % von %@ verbraucht', Math.round(used * 100), money(budget.maxBudget)),
        h('span', { class: 'spacer' }), resetText && h('span', { title: dateTimeText(reset) }, t('Reset %@', resetText))));
  });
}

/** Usage at a glance from the tray icon: the Claude subscription's limits and every key's budget. */
function renderPopover() {
  const subscriptionProfiles = view.profiles.filter((p) => p.subscription);
  const gatewayProfiles = view.profiles.filter((p) => !p.subscription);
  const attention = view.disabled ? 0 : view.bindings.filter((b) => projectStatus(b).attention).length;
  const open = (page = null) => invoke('open_main_window', { page });
  const sub = view.subscription;
  document.getElementById('popover').replaceChildren(
    h('header', { class: 'popover-head' },
      h('span', { class: 'logo' }, icon('bolt')),
      h('strong', null, t('Verbrauch')),
      h('span', { class: 'spacer' }),
      button(t('Aktualisieren'), refreshPopover, { iconName: 'refresh', iconOnly: true, cls: popoverRefreshing ? 'spinning' : null, disabled: popoverRefreshing })),
    h('div', { class: 'popover-body' },
      !view.profiles.length && h('div', { class: 'muted' }, t('Noch keine Profile')),
      subscriptionProfiles.length > 0 && h('section', { class: 'popover-section' },
        h('div', { class: 'popover-title' }, t('Claude-Abo'), sub?.plan && h('span', { class: 'badge' }, sub.plan)),
        sub && !sub.loggedIn
          ? h('div', { class: 'line red small' }, icon('keyOff'), t('Nicht bei Claude angemeldet – in Claude Code „/login“ ausführen.'))
          : subscriptionUsage(true)),
      gatewayProfiles.length > 0 && h('section', { class: 'popover-section' },
        h('div', { class: 'popover-title' }, t('Budgets')),
        gatewayProfiles.map(popoverProfile)),
      view.disabled && h('div', { class: 'line orange small' }, icon('pause'), t('KeyZapper ist deaktiviert')),
      attention > 0 && h('button', { type: 'button', class: 'popover-attention', onclick: () => open('projects') },
        icon('warning', 'orange'),
        h('span', { class: 'spacer' }, attention === 1 ? t('1 Projekt braucht Aufmerksamkeit') : t('%@ Projekte brauchen Aufmerksamkeit', attention)),
        icon('arrow', 'muted'))),
    h('footer', { class: 'popover-foot' },
      button(t('KeyZapper öffnen'), () => open(), { cls: 'primary' }),
      h('span', { class: 'spacer' }),
      button(t('KeyZapper beenden'), () => invoke('quit_app'), { cls: 'link' })));
  fitPopover();
}

/** Asks for a window as high as the content (the body scrolls beyond the maximum). */
function fitPopover() {
  const [head, body, foot] = document.getElementById('popover').children;
  const height = Math.ceil(head.offsetHeight + body.scrollHeight + foot.offsetHeight);
  if (height === popoverHeight) return;
  popoverHeight = height;
  invoke('fit_usage_popover', { height });
}

async function startPopover() {
  document.body.append(tooltip);
  english = await invoke('translations');
  setView(await invoke('get_view'));
  await listen('changed', async () => setView(await invoke('get_view')));
  await listen('popover-shown', async () => setView(await invoke('get_view')));
  document.addEventListener('keydown', (e) => { if (e.key === 'Escape') invoke('hide_usage_popover'); });
}

// MARK: Tooltips

/** Shows `title` texts as a quick tooltip; the WebViews show native ones late or not at all. A popover, so it is above open dialogs. */
const tooltip = h('div', { class: 'tooltip', role: 'tooltip', popover: 'manual' });
let tipTarget = null;
let tipTimer = null;

function hideTip() {
  clearTimeout(tipTimer);
  tipTarget = null;
  if (tooltip.matches(':popover-open')) tooltip.hidePopover();
}

function showTip(target) {
  if (!target.isConnected) return;
  tooltip.textContent = target.dataset.tip;
  if (!tooltip.matches(':popover-open')) tooltip.showPopover();
  const rect = target.getBoundingClientRect();
  const { offsetWidth: width, offsetHeight: height } = tooltip;
  const below = rect.bottom + 6 + height <= window.innerHeight;
  tooltip.style.top = `${below ? rect.bottom + 6 : rect.top - 6 - height}px`;
  tooltip.style.left = `${Math.max(8, Math.min(rect.left + rect.width / 2 - width / 2, window.innerWidth - width - 8))}px`;
}

document.addEventListener('mouseover', (event) => {
  const target = event.target.closest?.('[title], [data-tip]');
  if (target === tipTarget) return;
  hideTip();
  if (!target) return;
  // Moved to data-tip so the native tooltip does not show as well.
  if (target.hasAttribute('title')) {
    target.dataset.tip = target.title;
    if (target.getAttribute('aria-label') !== target.title) target.setAttribute('aria-description', target.title);
    target.removeAttribute('title');
  }
  if (!target.dataset.tip) return;
  tipTarget = target;
  tipTimer = setTimeout(() => showTip(target), 350);
});
for (const type of ['mousedown', 'keydown', 'scroll']) document.addEventListener(type, hideTip, true);

// MARK: Drag & drop

/** Folders dragged from Finder or Explorer: onto a profile card they are bound to it, anywhere else "Projekt zuordnen" opens. */
let dropTarget = null;

function profileCardAt(position) {
  const scale = window.devicePixelRatio || 1;
  return document.elementFromPoint(position.x / scale, position.y / scale)?.closest('[data-profile-id]') ?? null;
}

function markDrop(position) {
  const card = position && !document.querySelector('dialog[open]') ? profileCardAt(position) : null;
  document.body.classList.toggle('dragging', !!position);
  if (card === dropTarget) return;
  dropTarget?.classList.remove('drop-target');
  dropTarget = card;
  dropTarget?.classList.add('drop-target');
}

async function dropped({ paths, position }) {
  const card = profileCardAt(position);
  markDrop(null);
  if (!paths?.length || !view.profiles.length || view.disabled || document.querySelector('dialog[open]')) return;
  const profileId = card?.dataset.profileId;
  if (!profileId) {
    const picked = await call('inspect_folder', { path: paths[0] });
    if (picked) addProjectDialog(null, picked);
    return;
  }
  if (!(await ensureLogin(profileId))) return;
  for (const path of paths) {
    const picked = await call('inspect_folder', { path });
    if (!picked) break;
    await call('bind', { folder: picked.folder, profileId });
  }
}

// MARK: Start

async function start() {
  if (isPopover()) return startPopover();
  document.body.append(tooltip);
  english = await invoke('translations');
  setView(await invoke('get_view'));
  if (page === 'tools') call('load_tools');
  await listen('changed', async () => setView(await invoke('get_view')));
  await listen('navigate', (e) => navigate(e.payload));
  await listen('tauri://drag-enter', (e) => markDrop(e.payload.position));
  await listen('tauri://drag-over', (e) => markDrop(e.payload.position));
  await listen('tauri://drag-leave', () => markDrop(null));
  await listen('tauri://drag-drop', (e) => dropped(e.payload));
}

start();
