import test from 'node:test';
import assert from 'node:assert/strict';
import {createHermesController, hermesPage} from '../hermes.js';
import {readFileSync} from 'node:fs';
const en = JSON.parse(readFileSync(new URL('../../locales/en.json', import.meta.url), 'utf8'));

const target = { executable: '/opt/hermes/bin/hermes', home: '/home/user/.hermes', displayName: 'Hermes' };
const selection = { target, profileId: 'p-1', modelId: 'tenant-a/team/sonnet', tenant: 'tenant-a' };
const preview = { confirmationId: 'c-1', summary: 'backend summary', expiresInSeconds: 60 };
const refreshed = (supported = true) => ({ hermes: { targets: [{ target, supported }], health: 'notConnected', selected: null, busy: false, problem: null, testStatus: 'notTested' } });

function make(overrides = {}) {
  const calls = [];
  const confirmations = [];
  const c = createHermesController({
    call: async (name, args) => { calls.push([name, args]); return name === 'hermes_refresh' ? refreshed() : preview; },
    confirm: async (details) => { confirmations.push(details); return true; },
    changed: () => {}, ...overrides,
  });
  return { c, calls, confirmations };
}

test('cancelled confirmation never applies Hermes config', async () => {
  const { c, calls } = make({ confirm: async () => false });
  await c.refresh();
  await c.connect(selection);
  assert.deepEqual(calls.map(([name]) => name), ['hermes_refresh', 'hermes_preview']);
});

test('connect applies the exact selection with the matching preview confirmation ID', async () => {
  const { c, calls, confirmations } = make();
  await c.refresh();
  await c.connect(selection);
  assert.deepEqual(calls, [
    ['hermes_refresh', undefined],
    ['hermes_preview', { action: 'connect', target, selection }],
    ['hermes_connect', { selection, confirmationId: 'c-1' }],
  ]);
  assert.equal(confirmations[0].selection, selection);
});

test('preview identity is passed unchanged and stale preview is refused by the backend', async () => {
  const { c, calls } = make({ call: async (name, args) => {
    calls.push([name, args]);
    if (name === 'hermes_refresh') return refreshed();
    if (name === 'hermes_preview') return preview;
    if (name === 'hermes_connect') throw new Error('Confirmation expired or no longer matches this selection.');
  } });
  await c.refresh();
  await c.connect(selection);
  assert.match(c.state.error, /Confirmation expired/);
  assert.equal(calls.at(-1)[1].confirmationId, 'c-1');
});

test('CLI failure is visible as controller error and failed status', async () => {
  const { c, calls } = make({ call: async (name) => {
    calls.push([name]);
    if (name === 'hermes_refresh') return refreshed();
    throw new Error('CLI failed');
  } });
  await c.refresh();
  await c.connect(selection);
  assert.equal(c.state.error, 'CLI failed');
  assert.equal(c.state.status, 'failed');
});

test('repeat mutation clicks are suppressed while preview is pending', async () => {
  let release;
  const { c, calls } = make({ call: (name) => {
    calls.push(name);
    if (name === 'hermes_refresh') return Promise.resolve(refreshed());
    if (name === 'hermes_preview') return new Promise((resolve) => { release = () => resolve(preview); });
    return Promise.resolve(preview);
  } });
  await c.refresh();
  const first = c.connect(selection);
  const second = await c.connect(selection);
  assert.equal(second, false);
  release();
  await first;
  assert.equal(calls.filter((name) => name === 'hermes_preview').length, 1);
});

test('unavailable or unsupported Hermes target cannot connect', async () => {
  const { c, calls } = make({ call: async (name, args) => {
    calls.push([name, args]);
    return name === 'hermes_refresh' ? refreshed(false) : preview;
  } });
  await c.refresh();
  await c.connect(selection);
  assert.equal(c.state.error, 'Selected Hermes target is unavailable or unsupported.');
  assert.deepEqual(calls.map(([name]) => name), ['hermes_refresh']);
});

test('paid test requires separate explicit consent and stays notTested when unsupported', async () => {
  const { c, calls } = make();
  await c.refresh();
  const result = await c.testConnection(false);
  assert.equal(result, false);
  assert.equal(await c.testConnection(true), false);
  assert.equal(c.state.testStatus, 'notTested');
  assert.deepEqual(calls.map(([name]) => name), ['hermes_refresh']);
});

test('controller state is an immutable snapshot, including nested target identifiers', async () => {
  const { c } = make();
  await c.refresh();
  assert.equal(Object.isFrozen(c.state), true);
  assert.equal(Object.isFrozen(c.state.targets), true);
  assert.equal(Object.isFrozen(c.state.targets[0]), true);
  assert.equal(Object.isFrozen(c.state.targets[0].target), true);
});

test('configured Hermes status does not imply a successful paid test', async () => {
  const { c } = make({ call: async () => ({ profiles: [], hermes: { targets: [{ target, supported: false }], selected: null, health: 'configured', testStatus: 'notTested' } }) });
  await c.refresh();
  assert.equal(c.state.health, 'configured');
  assert.equal(c.state.testStatus, 'notTested');
});

test('journal-only recovery previews the exact pending target without an owner selection', async () => {
  const calls = [];
  const pending = { ...target, home: '/home/user/.hermes-pending' };
  const c = createHermesController({
    call: async (name, args) => {
      calls.push([name, args]);
      if (name === 'hermes_refresh') return { hermes: { targets: [{ target: pending, supported: true }], selected: null, recoveryTarget: pending, health: 'recoveryRequired' } };
      return preview;
    }, confirm: async () => false,
  });
  await c.refresh();
  assert.equal(await c.recover(), false);
  assert.deepEqual(calls.map(([name]) => name), ['hermes_refresh', 'hermes_preview']);
  assert.deepEqual(calls[1][1], { action: 'recover', target: pending, selection: null });
});

class TestElement {
  constructor(tag) { this.tagName = tag; this.children = []; this.listeners = {}; this.attributes = {}; this.value = ''; this.disabled = false; this._textContent = ''; }
  get textContent() { return this.children.length ? this.children.map((child) => child.textContent).join('') : this._textContent; }
  set textContent(value) { this._textContent = String(value); this.children = []; }
  getAttribute(name) { return this.attributes[name] ?? null; }
  setAttribute(name, value) { this.attributes[name] = String(value); if (name === 'id') this.id = String(value); }
  addEventListener(name, fn) { this.listeners[name] = fn; }
  append(child) {
    this.children.push(child);
    child.parent = this;
    if (this.tagName === 'select' && child.tagName === 'option' && child.selected) this.value = child.value;
    else if (this.tagName === 'select' && child.tagName === 'option' && this.value === '') this.value = child.value;
  }
  replaceChildren(...children) { this.children = []; this.value = ''; for (const child of children) this.append(child); }
  focus() { globalThis.document.activeElement = this; }
  get options() { return this.children.filter((child) => child.tagName === 'option'); }
  querySelectorAll(selector) { const tags = selector.split(',').map((tag) => tag.trim()); return this.children.flatMap((child) => [child, ...child.querySelectorAll(selector)]).filter((child) => tags.includes(child.tagName)); }
  querySelector(selector) { return this.querySelectorAll(selector)[0] ?? null; }
}
function makeDom() {
  const main = new TestElement('main');
  globalThis.document = { activeElement: null, createElement: (tag) => new TestElement(tag),
    createTextNode: (text) => Object.assign(new TestElement('#text'), { textContent: text }),
    getElementById: (id) => id === 'main' ? main : null };
  // Mirrors app.js h(): properties are assigned before children are appended.
  const h = (tag, props, ...children) => {
    const el = document.createElement(tag);
    for (const [key, value] of Object.entries(props ?? {})) {
      if (value == null || value === false) continue;
      if (key.startsWith('on')) el.addEventListener(key.slice(2), value);
      else if (key === 'class') el.className = value;
      else if (['value', 'checked', 'disabled', 'selected', 'hidden', 'required'].includes(key)) el[key] = value;
      else el.setAttribute(key, value === true ? '' : value);
    }
    for (const child of children.flat(Infinity)) {
      if (child == null || child === false) continue;
      el.append(child instanceof TestElement ? child : document.createTextNode(String(child)));
    }
    return el;
  };
  const field = (label, control) => h('label', null, label, control);
  const pageHeader = (...children) => h('header', null, ...children);
  return { main, h, field, pageHeader };
}
const pageView = { language: 'en', profiles: [
  { id: 'p-1', name: 'Profile One', sso: false, subscription: false, tenants: [] },
  { id: 'p-2', name: 'Profile Two', sso: true, subscription: false, tenants: ['tenant-a', 'tenant-b'] },
] };
const translate = (key, ...values) => {
  let text = en[key] ?? key;
  for (const value of values) text = text.replace('%@', String(value));
  return text;
};
function renderHermes(dom, controller) {
  dom.main.replaceChildren(...hermesPage({ view: pageView, t: translate, ...dom, controller }).flat(Infinity));
}

test('real Hermes page preserves select draft values and keyboard focus on change and publication', async () => {
  const dom = makeDom();
  let c;
  let mounted = false;
  c = createHermesController({
    call: async () => ({ hermes: { targets: [{ target, supported: true }], selected: null, health: 'notConnected' } }),
    changed: () => { if (mounted) renderHermes(dom, c); },
  });
  renderHermes(dom, c);
  mounted = true;
  await c.refresh();
  let selects = dom.main.querySelectorAll('select');
  selects[0].value = JSON.stringify([target.executable, target.home]);
  selects[0].focus();
  selects[0].listeners.change({ target: selects[0] });
  selects = dom.main.querySelectorAll('select');
  assert.equal(selects[0].value, JSON.stringify([target.executable, target.home]));
  assert.equal(document.activeElement, selects[0]);
  selects[1].value = 'p-2';
  selects[1].focus();
  selects[1].listeners.change({ target: selects[1] });
  selects = dom.main.querySelectorAll('select');
  assert.equal(selects[0].value, JSON.stringify([target.executable, target.home]));
  assert.equal(selects[1].value, 'p-2');
  assert.equal(document.activeElement, selects[1]);
  selects[2].value = 'tenant-a';
  selects[2].focus();
  selects[2].listeners.change({ target: selects[2] });
  selects = dom.main.querySelectorAll('select');
  assert.equal(selects[2].value, 'tenant-a');
  assert.equal(document.activeElement, selects[2]);
  await c.refresh();
  selects = dom.main.querySelectorAll('select');
  assert.equal(selects[0].value, JSON.stringify([target.executable, target.home]));
  assert.equal(selects[1].value, 'p-2');
  assert.equal(selects[2].value, 'tenant-a');
});

test('Hermes Repair and Remove labels resolve from the locale dictionary', async () => {
  const dom = makeDom();
  const recoveryController = { state: { targets: [{ target, supported: true }], selection: null, recoveryTarget: target, health: 'recoveryRequired', testStatus: 'notTested', busy: false }, recover() {}, refresh() {} };
  const output = hermesPage({ view: pageView, t: translate, ...dom, controller: recoveryController });
  dom.main.replaceChildren(...output);
  assert.ok(dom.main.querySelectorAll('button').some((node) => node.textContent === 'Repair'));
  const readonlyController = { ...recoveryController, state: { ...recoveryController.state, targets: [{ target, supported: false }] } };
  dom.main.replaceChildren(...hermesPage({ view: pageView, t: translate, ...dom, controller: readonlyController }));
  const readonlyRepair = dom.main.querySelectorAll('button').find((node) => node.textContent === 'Repair');
  assert.ok(readonlyRepair);
  assert.equal(readonlyRepair.disabled, true);
  const removeController = { ...recoveryController, state: { ...recoveryController.state, selection, recoveryTarget: null, health: 'configured' }, disconnect() {} };
  const removeOutput = hermesPage({ view: pageView, t: translate, ...dom, controller: removeController });
  dom.main.replaceChildren(...removeOutput);
  assert.ok(dom.main.querySelectorAll('button').some((node) => node.textContent === 'Remove'));
});

test('unknown installation without HOME renders fixed localized safe diagnostic guidance, not no-installation', async () => {
  const dom = makeDom();
  const { c } = make({ call: async () => ({ hermes: { targets: [], diagnostics: [{ executable: '/synthetic/unknown-hermes', reason: 'unknownBuild', stderr: 'synthetic-secret-cli-output' }], health: 'unavailable', selected: null } }) });
  await c.refresh(); renderHermes(dom, c);
  const text = dom.main.textContent;
  assert.match(text, /Unknown or different Hermes build/);
  assert.match(text, /not validated/);
  assert.match(text, /python3 desktop\/scripts\/test_hermes_contract.py --hermes ABSOLUTE_EXECUTABLE --helper ABSOLUTE_EXECUTABLE --home FRESH_SCRATCH_HOME/);
  assert.match(text, /approved offline runtime/);
  assert.doesNotMatch(text, /No Hermes installation found|synthetic-secret-cli-output|Native Hermes configuration writes are not supported in this version/);
  assert.deepEqual(dom.main.querySelectorAll('button').map((node) => node.textContent), ['Refresh installations']);
  const other = { state: { ...c.state, diagnostics: [{ executable: '/synthetic/known', reason: 'notValidated' }] } };
  renderHermes(dom, other);
  assert.match(dom.main.textContent, /local validation did not pass/);
  assert.doesNotMatch(dom.main.textContent, /Unknown or different Hermes build/);
});

test('failed apply adopts the backend recovery target without a manual refresh', async () => {
  const pending = { ...target, home: '/synthetic/recovery' };
  const calls = [];
  const reads = [];
  const c = createHermesController({
    call: async (name, args) => {
      calls.push([name, args]);
      if (name === 'hermes_refresh') return refreshed();
      if (name === 'hermes_preview') return preview;
      return null; // app.js call() swallows the rejected command and returns null
    },
    readView: async () => {
      reads.push('get_view');
      return { profiles: [], hermes: { targets: [{ target, supported: true }], selected: null, recoveryTarget: pending,
        health: 'recoveryRequired', problem: null, testStatus: 'notTested' } };
    },
    confirm: async () => true,
  });
  await c.refresh();
  assert.equal(c.state.recoveryTarget, null);
  assert.equal(await c.connect(selection), false);
  assert.equal(reads.length, 1, 'failure reads the non-secret backend view once');
  assert.deepEqual(c.state.recoveryTarget, pending);
  assert.equal(c.state.health, 'recoveryRequired');
  assert.match(c.state.error, /Hermes operation failed/);
  assert.equal(calls.filter(([name]) => name === 'hermes_refresh').length, 1, 'no manual refresh RPC is required');
});

test('page refuses a model ID that does not belong to the explicitly chosen tenant', async () => {
  const dom = makeDom();
  const calls = [];
  let c;
  const view = () => ({ hermes: { targets: [{ target, supported: true }], selected: null, health: 'notConnected' } });
  c = createHermesController({
    call: async (name, args) => {
      calls.push([name, args]);
      if (name === 'hermes_refresh') return view();
      if (name === 'hermes_preview') return preview;
      return { hermes: { targets: [{ target, supported: true }], selected: selection, health: 'configured' } };
    },
    confirm: async () => true,
    changed: () => renderHermes(dom, c),
  });
  renderHermes(dom, c);
  await c.refresh();
  let selects = dom.main.querySelectorAll('select');
  selects[0].value = JSON.stringify([target.executable, target.home]);
  selects[0].listeners.change({ target: selects[0] });
  selects = dom.main.querySelectorAll('select');
  selects[1].value = 'p-2';
  selects[1].listeners.change({ target: selects[1] });
  selects = dom.main.querySelectorAll('select');
  selects[2].value = 'tenant-a';
  selects[2].listeners.change({ target: selects[2] });
  const model = dom.main.querySelectorAll('input')[0];
  model.value = 'tenant-b/sonnet';
  model.listeners.input({ target: model });
  await c.refresh();
  assert.match(dom.main.textContent, /does not belong to the selected tenant/);
  dom.main.querySelectorAll('button').find((node) => node.textContent === 'Connect Hermes').listeners.click();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(calls.filter(([name]) => name === 'hermes_preview'), [], 'mismatch never reaches preview');

  model.value = 'tenant-a/sonnet';
  model.listeners.input({ target: model });
  await c.refresh();
  assert.doesNotMatch(dom.main.textContent, /does not belong to the selected tenant/);
  dom.main.querySelectorAll('button').find((node) => node.textContent === 'Connect Hermes').listeners.click();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(calls.filter(([name]) => name === 'hermes_preview').length, 1, 'the qualified combination still connects');
});

test('app.js wires the shared backend Hermes snapshot into the controller', () => {
  const source = readFileSync(new URL('../app.js', import.meta.url), 'utf8');
  assert.match(source, /readView: \(\) => invoke\('get_view'\)/);
  assert.match(source, /hermesController\.sync\(view\.hermes\)/);
});

test('controller mixed inventory preserves recovery health and exact Repair route on the real page', async () => {
  const pending = { ...target, home: '/synthetic/pending' };
  const unknown = { ...target, executable: '/synthetic/unknown' };
  const hermes = process.env.KEYZAPPER_HERMES_CONTROLLER_VIEW
    ? JSON.parse(process.env.KEYZAPPER_HERMES_CONTROLLER_VIEW)
    : { targets: [{ target: pending, supported: true }, { target: unknown, supported: false }], selected: null, recoveryTarget: pending, health: 'recoveryRequired', problem: 'legacy compatibility warning' };
  const calls = [];
  const c = createHermesController({ call: async (name, args) => {
    calls.push([name, args]); return name === 'hermes_refresh' ? { profiles: [], hermes } : preview;
  }, confirm: async () => false });
  await c.refresh();
  assert.equal(c.state.health, 'recoveryRequired');
  assert.deepEqual(c.state.recoveryTarget, hermes.recoveryTarget);
  assert.equal(c.state.selection, null);
  const dom = makeDom(); renderHermes(dom, c);
  const repair = dom.main.querySelectorAll('button').find((node) => node.textContent === 'Repair');
  assert.ok(repair); assert.equal(repair.disabled, false);
  await repair.listeners.click();
  assert.deepEqual(calls.at(-1), ['hermes_preview', { action: 'recover', target: hermes.recoveryTarget, selection: null }]);
  assert.equal(await c.connect({ ...selection, target: hermes.targets[1].target }), false);
  assert.equal(calls.length, 2, 'unknown target cannot preview or silently fall back');
});
