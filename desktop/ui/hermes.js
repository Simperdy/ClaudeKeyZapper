const ACTIONS = Object.freeze({ connect: 'connect', disconnect: 'disconnect', recover: 'recover' });
const targetKey = (target) => JSON.stringify([target?.executable, target?.home]);
// An explicitly chosen tenant must be the exact `<tenant>/` prefix of the model ID. Never rewrite either value.
const modelMatchesTenant = (modelId, tenant) => !tenant
  || (modelId.startsWith(`${tenant}/`) && modelId.length > tenant.length + 1);

export function createHermesController({ call, confirm, changed = () => {}, readView = null }) {
  let snapshot = { busy: false, loaded: false, error: null, status: 'notTested', testStatus: 'notTested', targets: [], diagnostics: [], selection: null, recoveryTarget: null, health: 'unavailable', problem: null };
  const publish = (next) => { snapshot = { ...snapshot, ...next }; changed(snapshot); };
  const fail = (error) => publish({ busy: false, error: String(error?.message ?? error), status: 'failed' });
  // Non-secret backend state only: adopt what the controller already published instead of
  // leaving the page on a stale snapshot until the user refreshes manually. No writes.
  const adopt = (hermes) => {
    if (!hermes || !Array.isArray(hermes.targets)) return false;
    publish({ loaded: true, targets: hermes.targets, diagnostics: hermes.diagnostics ?? [], selection: hermes.selected ?? null,
      recoveryTarget: hermes.recoveryTarget ?? null, health: hermes.health ?? snapshot.health,
      problem: hermes.problem ?? null, testStatus: hermes.testStatus ?? snapshot.testStatus, status: hermes.health ?? snapshot.status });
    return true;
  };
  const run = async (work) => {
    if (snapshot.busy) return false;
    publish({ busy: true, error: null });
    try { return await work(); }
    catch (error) {
      if (readView) { try { adopt((await readView())?.hermes); } catch { /* keep the previous snapshot */ } }
      fail(error);
      return false;
    }
    finally { if (snapshot.busy) publish({ busy: false }); }
  };
  const findTarget = (target) => snapshot.targets.find((entry) => targetKey(entry.target) === targetKey(target));
  const doAction = async (action, target, selection) => run(async () => {
    const entry = findTarget(target);
    if (!entry || !entry.supported) throw new Error('Selected Hermes target is unavailable or unsupported.');
    const args = { action, target: entry.target, selection: selection ?? null };
    const proposal = await call('hermes_preview', args);
    if (!proposal?.confirmationId) throw new Error('Hermes preview unavailable.');
    const approved = await confirm({ action, target: entry.target, selection, preview: proposal });
    if (!approved) return false;
    const applied = action === ACTIONS.connect
      ? await call('hermes_connect', { selection, confirmationId: proposal.confirmationId })
      : action === ACTIONS.disconnect
        ? await call('hermes_disconnect', { target: entry.target, confirmationId: proposal.confirmationId })
        : await call('hermes_recover', { target: entry.target, confirmationId: proposal.confirmationId });
    if (!applied) throw new Error('Hermes operation failed.');
    const updatedHermes = applied.hermes;
    publish({ busy: false, error: null, status: updatedHermes?.health ?? 'configured', health: updatedHermes?.health ?? 'configured',
      selection: updatedHermes ? updatedHermes.selected : snapshot.selection,
      recoveryTarget: updatedHermes ? updatedHermes.recoveryTarget ?? null : snapshot.recoveryTarget,
      testStatus: updatedHermes?.testStatus ?? snapshot.testStatus,
      targets: updatedHermes?.targets ?? snapshot.targets, diagnostics: updatedHermes?.diagnostics ?? snapshot.diagnostics });
    return true;
  });
  const controller = {
    get state() {
      const targets = snapshot.targets.map((entry) => Object.freeze({ ...entry, target: Object.freeze({ ...entry.target }) }));
      const selection = snapshot.selection && Object.freeze({ ...snapshot.selection, target: Object.freeze({ ...snapshot.selection.target }) });
      const recoveryTarget = snapshot.recoveryTarget && Object.freeze({ ...snapshot.recoveryTarget });
      return Object.freeze({ ...snapshot, targets: Object.freeze(targets), selection, recoveryTarget });
    },
    // Shared non-secret snapshot path: any backend view (event, get_view, failure) updates the page state.
    sync(hermes) { adopt(hermes); },
    async refresh() {
      return run(async () => {
        const result = await call('hermes_refresh');
        const hermes = result?.hermes;
        if (!hermes || !Array.isArray(hermes.targets)) throw new Error('Hermes registry refresh unavailable.');
        const selection = hermes.selected ?? null;
        const recoveryTarget = hermes.recoveryTarget ?? null;
        const selectedProfileExists = !selection || result.profiles?.some((p) => p.id === selection.profileId) === true;
        const health = selectedProfileExists || hermes.health === 'recoveryRequired' ? hermes.health : 'unavailable';
        publish({ loaded: true, targets: hermes.targets, diagnostics: hermes.diagnostics ?? [], selection, recoveryTarget, health, problem: hermes.problem,
          testStatus: hermes.testStatus ?? 'notTested', status: health,
          error: hermes.problem ?? (!selectedProfileExists ? 'Selected Hermes profile is unavailable.' : null) });
        return true;
      });
    },
    preview(selection) {
      if (!selection?.target || !selection.profileId || !selection.modelId) return Promise.resolve(false);
      return run(async () => {
        const entry = findTarget(selection.target);
        if (!entry || !entry.supported) throw new Error('Selected Hermes target is unavailable or unsupported.');
        const proposal = await call('hermes_preview', { action: ACTIONS.connect, target: entry.target, selection });
        if (!proposal?.confirmationId) throw new Error('Hermes preview unavailable.');
        return proposal;
      });
    },
    connect(selection) {
      if (!selection?.target || !selection.profileId || !selection.modelId) return Promise.resolve(false);
      return doAction(ACTIONS.connect, selection.target, selection);
    },
    disconnect() {
      const selected = snapshot.selection;
      if (!selected?.target) return Promise.resolve(false);
      return doAction(ACTIONS.disconnect, selected.target, null);
    },
    recover() {
      const target = snapshot.recoveryTarget ?? snapshot.selection?.target;
      if (!target) return Promise.resolve(false);
      return doAction(ACTIONS.recover, target, null);
    },
    async testConnection(paidConsent) {
      if (!paidConsent || snapshot.busy) return false;
      // Task 6 exposes no paid model-test command. Never synthesize success or issue an invented RPC.
      publish({ testStatus: 'notTested' });
      return false;
    },
  };
  return controller;
}

let draft = { target: '', profileId: '', tenant: '', modelId: '' };

function selectControl(h, props, options) {
  const { value, ...attributes } = props;
  const element = h('select', attributes, options);
  element.value = value ?? '';
  return element;
}

export function hermesPage({ view, t, h, field, pageHeader, controller }) {
  const state = controller.state;
  const targetId = (target) => targetKey(target);
  const targets = state.targets ?? [];
  const selected = state.selection;
  if (!draft.target && selected?.target && targets.some((entry) => targetId(entry.target) === targetId(selected.target))) draft.target = targetId(selected.target);
  const profiles = view.profiles.filter((profile) => !profile.subscription);
  const draftProfile = profiles.find((profile) => profile.id === draft.profileId);
  const target = targets.find((entry) => targetId(entry.target) === draft.target)?.target;
  const usableTargets = targets.filter((entry) => entry.supported);
  const diagnostics = [...(state.diagnostics ?? [])];
  for (const entry of targets.filter((entry) => !entry.supported)) {
    if (!diagnostics.some((item) => item.executable === entry.target.executable)) diagnostics.push({ executable: entry.target.executable, reason: 'notValidated' });
  }
  const targetOptions = [h('option', { value: '' }, t('Hermes-Installation auswählen')),
    draft.target && !targets.some((entry) => targetId(entry.target) === draft.target) && h('option', { value: draft.target }, t('Hermes-Installation nicht verfügbar')),
    ...targets.map(({ target: item, supported }) => h('option', { value: targetId(item) }, `${item.displayName} · ${item.home} · ${t(supported ? 'Validiert' : 'Nicht validiert')}`))];
  const profileOptions = [h('option', { value: '' }, t('KeyZapper-Profil auswählen')),
    draft.profileId && !draftProfile && h('option', { value: draft.profileId }, t('KeyZapper-Profil nicht verfügbar')),
    ...profiles.map((profile) => h('option', { value: profile.id }, profile.name))];
  const profileTenants = draftProfile?.tenants ?? [];
  const tenantOptions = [h('option', { value: '' }, t(draftProfile?.sso && profileTenants.length ? 'Mandant auswählen' : 'Kein Tenant')),
    draft.tenant && !profileTenants.includes(draft.tenant) && h('option', { value: draft.tenant }, t('Mandant nicht verfügbar')),
    ...profileTenants.map((tenant) => h('option', { value: tenant }, tenant))];
  const mutatingDisabled = state.busy;
  const onDraft = (key, value, event) => {
    draft = { ...draft, [key]: value };
    const focusKey = event?.target?.getAttribute?.('data-hermes-control');
    document.getElementById('main').replaceChildren(...hermesPage({ view, t, h, field, pageHeader, controller }).flat(Infinity));
    if (focusKey) Array.from(document.getElementById('main').querySelectorAll('select, input')).find((element) => element.getAttribute('data-hermes-control') === focusKey)?.focus();
  };
  const selectedTargetAvailable = !selected || targets.some((entry) => targetId(entry.target) === targetId(selected.target));
  const existingProfile = selected && view.profiles.find((profile) => profile.id === selected.profileId);
  const selectionFromDraft = () => {
    const selectedTarget = targets.find((entry) => targetId(entry.target) === draft.target);
    return selectedTarget && draftProfile && (!draftProfile.sso || (profileTenants.length > 0 && !!draft.tenant)) && (!draft.tenant || profileTenants.includes(draft.tenant)) && draft.modelId.trim() && modelMatchesTenant(draft.modelId.trim(), draft.tenant)
      ? { target: selectedTarget.target, profileId: draft.profileId, modelId: draft.modelId.trim(), tenant: draft.tenant || null }
      : null;
  };
  const errorText = state.error ? t('Hermes-Aktion fehlgeschlagen. Installationen aktualisieren und erneut versuchen.') : null;
  const displayStatus = ({ configured: t('Eingerichtet'), notConnected: t('Nicht verbunden'), drifted: t('Konfiguration außerhalb von KeyZapper geändert'), recoveryRequired: t('Wiederherstellung erforderlich'), unavailable: t('Nicht verfügbar'), problem: t('Aufmerksamkeit erforderlich') })[state.health] ?? t('Status unbekannt');
  const displayTestStatus = ({ notTested: t('Nicht getestet'), succeeded: t('Test erfolgreich'), failed: t('Test fehlgeschlagen') })[state.testStatus] ?? t('Nicht getestet');
  const recoveryTarget = state.recoveryTarget;
  const selectedTargetSupported = !!selected && targets.some((entry) => entry.supported && targetId(entry.target) === targetId(selected.target));
  const recoveryTargetSupported = !!recoveryTarget && targets.some((entry) => entry.supported && targetId(entry.target) === targetId(recoveryTarget));
  const recoveryPending = state.health === 'recoveryRequired' && !!recoveryTarget;
  const actions = [
    h('button', { type: 'button', disabled: mutatingDisabled, onclick: () => controller.refresh() }, t('Installationen aktualisieren')),
    target && usableTargets.some((entry) => targetId(entry.target) === targetId(target)) && h('button', { type: 'button', disabled: mutatingDisabled, onclick: () => controller.connect(selectionFromDraft()) }, t(selected ? 'Hermes-Verbindung aktualisieren' : 'Hermes verbinden')),
    recoveryPending && h('button', { type: 'button', disabled: mutatingDisabled || !recoveryTargetSupported, onclick: () => controller.recover() }, t('Reparieren')),
    selectedTargetSupported && state.health !== 'recoveryRequired' && state.health !== 'notConnected' && h('button', { type: 'button', disabled: mutatingDisabled, onclick: () => controller.disconnect() }, t('Entfernen')),
  ];
  const output = [pageHeader(t('Hermes'), t('Wähle ausdrücklich eine Installation, ein KeyZapper-Profil und ein Modell. Schreibzugriffe sind nur für die exakt validierte Installation verfügbar.'), ...actions),
    h('section', { class: 'card' },
      selected && h('div', { class: 'section-note' },
        h('strong', null, t('Gespeicherte Hermes-Auswahl')),
        h('p', null, t('Installation: %@', selected.target.displayName)),
        h('p', { class: 'mono selectable' }, t('Hermes-Home: %@', selected.target.home)),
        h('p', { class: 'mono selectable' }, t('Programm: %@', selected.target.executable)),
        h('p', null, t('KeyZapper-Profil: %@', existingProfile?.name ?? t('Unbekanntes Profil'))),
        h('p', null, t('Mandant: %@', selected.tenant || t('Kein Tenant'))),
        h('p', null, t('Modell: %@', selected.modelId))),
      recoveryTarget && !selected && h('div', { class: 'section-note' }, h('strong', null, t('Unterbrochene Hermes-Wiederherstellung')), h('p', null, t('Hermes-Ziel: %@', recoveryTarget.displayName)), h('p', { class: 'mono selectable' }, t('Hermes-Home: %@', recoveryTarget.home))),
      selected && !selectedTargetAvailable && h('p', { class: 'note red' }, t('Das gespeicherte Hermes-Ziel ist nicht verfügbar. Aktualisiere Installationen; es wurde kein anderes Ziel ausgewählt.')),
      recoveryPending && !recoveryTargetSupported && h('p', { class: 'note' }, t('Diese Hermes-Installation ist nicht validiert. Wiederherstellung bleibt sichtbar, aber gesperrt.')),
      draft.target && !target && selectedTargetAvailable && h('p', { class: 'note red' }, t('Die ausgewählte Hermes-Installation ist nicht mehr verfügbar. Aktualisiere Installationen; es wurde kein Ersatz gewählt.')),
      field(t('Hermes-Installation'), selectControl(h, { value: draft.target, 'data-hermes-control': 'target', disabled: mutatingDisabled, onchange: (event) => onDraft('target', event.target.value, event) }, targetOptions), 'hermes-target'),
      field(t('KeyZapper-Profil'), selectControl(h, { value: draft.profileId, 'data-hermes-control': 'profile', disabled: mutatingDisabled, onchange: (event) => { draft = { ...draft, profileId: event.target.value, tenant: '' }; onDraft('profileId', event.target.value, event); } }, profileOptions), 'hermes-profile'),
      selected && !view.profiles.some((profile) => profile.id === selected.profileId) && h('p', { class: 'note red' }, t('Das gespeicherte Hermes-Profil fehlt. Wähle ausdrücklich ein Profil; es wurde kein Ersatz gewählt.')),
      draft.profileId && !draftProfile && h('p', { class: 'note red' }, t('Das ausgewählte KeyZapper-Profil ist nicht mehr verfügbar. Aktualisiere Profile; es wurde kein Ersatz gewählt.')),
      field(t('Mandant'), selectControl(h, { value: draft.tenant, 'data-hermes-control': 'tenant', disabled: mutatingDisabled, onchange: (event) => onDraft('tenant', event.target.value, event) }, tenantOptions), 'hermes-tenant'),
      draft.tenant && !(draftProfile?.tenants ?? []).includes(draft.tenant) && h('p', { class: 'note red' }, t('Der ausgewählte Mandant ist in der aktuellen Modellregistrierung nicht verfügbar. Es wurde kein Ersatz gewählt.')),
      draftProfile?.sso && !draftProfile.tenants?.length && h('p', { class: 'note orange' }, t('Für dieses SSO-Profil sind keine Mandanten aus der Modellregistrierung geladen. Anmeldung und Modellliste prüfen; es wird kein Mandant automatisch gewählt.')),
      field(t('Modell-ID'), h('input', { type: 'text', value: draft.modelId, disabled: mutatingDisabled, autocomplete: 'off', oninput: (event) => { draft.modelId = event.target.value; } }), 'hermes-model'),
      h('p', { class: 'muted' }, t('Gib die exakte Modell-ID des ausgewählten Profils oder Tenants ein. Es wird kein Standard automatisch ausgewählt.')),
      draft.tenant && draft.modelId.trim() && !modelMatchesTenant(draft.modelId.trim(), draft.tenant) && h('p', { class: 'note red' }, t('Die Modell-ID gehört nicht zum ausgewählten Mandanten. Sie muss mit „%@/“ beginnen; es wird nichts automatisch ersetzt.', draft.tenant)),
      h('p', { class: 'status' }, t('Verbindungsstatus: %@', displayStatus)),
      h('p', { class: 'status' }, t('Teststatus: %@', displayTestStatus)),
      state.busy && h('p', { role: 'status' }, t('Wird ausgeführt …')),
      errorText && h('p', { class: 'note red', role: 'alert' }, errorText),
      state.problem && h('p', { class: 'note red', role: 'alert' }, t('Hermes-Registry oder -Konfiguration konnte nicht gelesen werden. Aktualisieren, um es erneut zu versuchen.')),
      targets.length === 0 && diagnostics.length === 0 && h('p', { class: 'note' }, t('Keine Hermes-Installation gefunden. Aktualisiere die Installationen, um erneut zu suchen.')),
      ...diagnostics.map((item) => h('div', { class: 'note' },
        h('p', { class: 'mono selectable' }, t('Programm: %@', item.executable)),
        h('p', null, t(item.reason === 'unknownBuild'
          ? 'Unbekannter oder abweichender Hermes-Build; nicht validiert. Für diese Installation bleiben Konfigurationszugriffe und Änderungen gesperrt.'
          : 'Die lokale Validierung dieser Hermes-Installation war nicht erfolgreich. Konfigurationszugriffe und Änderungen bleiben gesperrt.')))),
      diagnostics.length > 0 && h('div', { class: 'note' },
        h('p', null, t('Sichere lokale Diagnose im Quell-Checkout: Aufrufschema des vorhandenen Test-Harness (Platzhalter ersetzen):')),
        h('code', { class: 'mono selectable' }, 'python3 desktop/scripts/test_hermes_contract.py --hermes ABSOLUTE_EXECUTABLE --helper ABSOLUTE_EXECUTABLE --home FRESH_SCRATCH_HOME'),
        h('p', null, t('Den Opt-in-Test nur in einer genehmigten Offline-Runtime mit frischen Scratch-Homes und synthetischen Zugangsdaten ausführen; niemals das echte Benutzer-Home übergeben. Keine automatische Freigabe, Quellcodeänderung oder kostenpflichtige Modellanfrage.'))),
      h('p', { class: 'muted' }, t('Der kostenpflichtige Modelltest ist nicht verfügbar: Es gibt keinen unterstützten Testbefehl. Der Teststatus bleibt „nicht getestet“; es wird keine kostenpflichtige Anfrage gesendet.')),
    )];
  return output;
}
