// No console window next to the app on Windows (release builds).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod model;
mod tray;

use keyzapper_core::errors::Error;
use keyzapper_core::gateway::CheckResult;
use keyzapper_core::models::budget_targets;
use keyzapper_core::tools::{self, Tool};
use keyzapper_core::{agents, backup, claude_desktop, claude_office, clipboard, gateway, l, legacy_keychain, mcp_catalog, oidc, subscription, update};
use model::{McpServerInput, Model, ProfileInput, TenantBudget, View};
use keyzapper_core::hermes::controller::{Action as HermesAction, Inputs as HermesInputs, Operation as HermesOperation, Preview as HermesPreview};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use window_vibrancy::NSVisualEffectMaterial;

struct Shared(Mutex<Model>);

fn with_model<T>(app: &AppHandle, f: impl FnOnce(&mut Model) -> T) -> T {
    let shared = app.state::<Shared>();
    let mut model = shared.0.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut model)
}

fn view(app: &AppHandle) -> View {
    let view = with_model(app, |m| m.view());
    tray::update(app);
    view
}

/// Tells the open windows to fetch a new view. A window waiting hidden in the tray is left alone: after an event,
/// WebKit keeps a hidden window on the display link for good (constant CPU load). It fetches a new view when shown.
fn changed(app: &AppHandle) {
    for (label, window) in app.webview_windows() {
        if window.is_visible().unwrap_or(false) {
            let _ = app.emit_to(label.as_str(), "changed", ());
        }
    }
    tray::update(app);
}

/// Brings the main window back, e.g. from the tray or the Dock.
fn show_window(app: &AppHandle) {
    tray::hide_popover(app);
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        // Hidden windows get no `changed` events; the window catches up now.
        let _ = app.emit_to("main", "changed", ());
    }
}

/// Puts the system's translucent material behind the window: vibrancy on macOS, Mica on Windows 11.
/// False where it is not available (Windows 10, Linux); the page then paints its own background.
fn apply_window_material(window: &tauri::WebviewWindow, material: window_vibrancy::NSVisualEffectMaterial, radius: Option<f64>) -> bool {
    use window_vibrancy::NSVisualEffectState;
    if cfg!(windows) {
        window_vibrancy::apply_mica(window, None).is_ok()
    } else {
        window_vibrancy::apply_vibrancy(window, material, Some(NSVisualEffectState::FollowsWindowActiveState), radius).is_ok()
    }
}

/// Runs blocking work (files, git, network, dialogs) off the main thread.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tauri::async_runtime::spawn_blocking(f).await.expect("background task panicked")
}

// MARK: Background refresh

/// Recomputes project status and the settings check off the lock; the newest run wins.
fn refresh(app: &AppHandle) {
    let Some((generation, state, binder)) = with_model(app, |m| {
        m.refresh_generation += 1;
        Some((m.refresh_generation, m.state.clone(), m.binder.clone()?))
    }) else {
        return;
    };
    let app = app.clone();
    std::thread::spawn(move || {
        let (statuses, mcp_active, global_health, findings) = model::status_snapshot(&state, &binder);
        with_model(&app, |m| {
            if m.refresh_generation == generation {
                m.statuses = statuses;
                m.mcp_active = mcp_active;
                m.global_health = global_health;
                m.global_findings = findings;
            }
        });
        changed(&app);
    });
}

/// Loads the budgets of every stored key and signed-in SSO profile from the gateway (LiteLLM `/key/info`, `/user/info`):
/// one per tenant of an SSO profile's projects (`?model=`), else one. The budgets are shown first; the model list follows,
/// so a slow or unreachable `/v1/models` never delays them.
fn refresh_budgets(app: &AppHandle, only: Option<&str>) {
    let profiles = with_model(app, |m| m.budget_profiles());
    for (profile, key) in profiles.into_iter().filter(|p| only.is_none_or(|o| o == p.0.id)) {
        let app = app.clone();
        std::thread::spawn(move || {
            let timeout = Duration::from_secs(10);
            let (secret, sso) = match key {
                Some(key) => (Some(key), false),
                None => (with_model(&app, |m| m.sso_token(&profile)).ok(), true),
            };
            let targets = with_model(&app, |m| budget_targets(&profile, &m.state.bindings));
            // The tenants are asked in parallel; one that fails (no access there) does not hide the others.
            let budgets: Vec<TenantBudget> = match secret.as_deref() {
                Some(secret) => std::thread::scope(|scope| {
                    let requests: Vec<_> = targets
                        .iter()
                        .map(|target| {
                            let profile = &profile;
                            scope.spawn(move || {
                                let model = target.model.as_deref();
                                if sso { gateway::sso_budget(&profile.endpoint, secret, model, timeout) } else { gateway::budget(&profile.endpoint, secret, model, timeout) }
                                    .ok()
                                    .map(|budget| TenantBudget { tenant: target.tenant.clone(), budget })
                            })
                        })
                        .collect();
                    requests.into_iter().filter_map(|request| request.join().ok().flatten()).collect()
                }),
                None => vec![],
            };
            with_model(&app, |m| {
                if budgets.is_empty() {
                    m.budgets.remove(&profile.id);
                } else {
                    m.budgets.insert(profile.id.clone(), budgets);
                }
            });
            changed(&app);
            // The model list tells which tenants a project can choose; a failed reload keeps the last list. Only an SSO
            // profile has tenants (its list follows the access its token has now); a key profile never asks for the list.
            let Some(secret) = secret.filter(|_| profile.is_sso()) else { return };
            if let Ok(models) = gateway::models(&profile.endpoint, &secret) {
                with_model(&app, |m| {
                    m.gateway_models.insert(profile.id.clone(), models);
                    m.follow_tenants(&profile.id);
                });
                changed(&app);
            }
        });
    }
}

/// Plan and usage limits of the Claude subscription. Claude Code's login is only read while a subscription profile exists.
fn refresh_subscription(app: &AppHandle) {
    let Some(previous) = with_model(app, |m| {
        if !m.has_subscription_profile() {
            m.subscription = None;
            return None;
        }
        Some(m.subscription.clone())
    }) else {
        return;
    };
    let app = app.clone();
    std::thread::spawn(move || {
        let status = subscription::status(previous.as_ref(), Duration::from_secs(10));
        with_model(&app, |m| m.subscription = Some(status));
        changed(&app);
    });
}

/// One Claude in Office run at a time (a run decides under the model lock, but uses the network and the app folders outside it).
static OFFICE_RUN: Mutex<()> = Mutex::new(());

/// Brings Claude in Office in line with the profile that has it switched on, or removes it. Anthropic's template is only
/// loaded while a manifest is to be written. `retry` writes again after an earlier failure; the start passes false, so a
/// failure does not repeat on every launch.
fn sync_office(app: &AppHandle, retry: bool) {
    let _run = OFFICE_RUN.lock().unwrap_or_else(|e| e.into_inner());
    let fetched = with_model(app, |m| m.office_wanted()).then(|| claude_office::fetch_template(Duration::from_secs(10)));
    if let Some((job, sideload)) = with_model(app, |m| Some((m.plan_office(fetched, retry)?, m.office_sideload.clone()))) {
        // The record is saved before the manifests are written, so a crash in between cannot leave unknown files.
        let outcome = job.run(&sideload, &mut |interim| with_model(app, |m| m.record_office(interim)));
        with_model(app, |m| m.finish_office(&job, outcome));
    }
    changed(app);
}

/// `sync_office` off the caller's thread: it may wait for the network, or for macOS to ask about the app folders.
fn sync_office_later(app: &AppHandle, retry: bool) {
    let app = app.clone();
    std::thread::spawn(move || sync_office(&app, retry));
}

/// Everything the tray popover shows: key budgets and the subscription's limits.
fn refresh_usage(app: &AppHandle) {
    with_model(app, |m| m.usage_refreshed_at = Some(std::time::Instant::now()));
    refresh_budgets(app, None);
    refresh_subscription(app);
}

/// Loads the central MCP server list (`ManagedMcpServersURL`), if IT set one.
fn refresh_mcp_catalog(app: &AppHandle) {
    let Some((url, token)) = with_model(app, |m| m.mcp_catalog_request()) else { return };
    changed(app);
    let app = app.clone();
    std::thread::spawn(move || {
        let result = mcp_catalog::fetch(&url, token.as_deref(), Duration::from_secs(15));
        with_model(&app, |m| m.take_mcp_catalog(result));
        refresh(&app);
    });
}

/// Which Claude tools are installed and what Claude Desktop is set to.
fn refresh_tools(app: &AppHandle) {
    let status = tools::status();
    let (library, recorded) = with_model(app, |m| (m.desktop_library.clone(), m.state.claude_desktop_profile_id.clone()));
    let desktop = claude_desktop::status(&library, recorded.as_deref());
    with_model(app, |m| {
        m.tools = Some(status);
        m.claude_desktop = desktop;
    });
    changed(app);
}

/// Installs a missing Claude Desktop in the background when IT asks for it (`ClaudeDesktopAutoInstall`), independent of the
/// tools page and not while KeyZapper is deactivated. Called after the organisation's configuration was written, so the first
/// launch already offers its sign-in. A failure is tried again only by an explicit refresh (`retry`).
fn auto_install_claude_desktop(app: &AppHandle, retry: bool) {
    let wanted = with_model(app, |m| m.config.claude_desktop_auto_install && !m.is_disabled() && (retry || m.state.claude_desktop_install_error.is_none()));
    if !wanted {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        if tools::desktop_installed() {
            return with_model(&app, |m| m.record_desktop_install(None));
        }
        let start = with_model(&app, |m| {
            let free = m.installing_tool.is_none();
            if free {
                m.installing_tool = Some(Tool::Desktop);
            }
            free
        });
        if !start {
            return;
        }
        changed(&app);
        let result = tools::install(Tool::Desktop);
        with_model(&app, |m| {
            m.installing_tool = None;
            m.record_desktop_install(result.err());
        });
        refresh_tools(&app);
    });
}

/// Stable releases only, unless beta releases are switched on (by default while a beta is installed).
fn check_updates(app: &AppHandle, user_initiated: bool) {
    let (enabled, version, beta) = with_model(app, |m| (m.config.update_check_enabled, m.app_version.clone(), m.beta_updates()));
    let notice = |text: String| {
        if user_initiated {
            with_model(app, |m| m.notice = Some(text));
        }
    };
    if !enabled {
        return notice(l!("Updates werden von der IT verteilt; die Update-Prüfung ist abgeschaltet."));
    }
    let Some(version) = version else { return notice(l!("Entwicklungsversion – keine Update-Prüfung.")) };
    match update::latest_release(beta) {
        Ok(release) if update::is_newer(&release.version, &version) => with_model(app, |m| m.available_update = Some(release)),
        Ok(_) => notice(l!("KeyZapper %@ ist aktuell.", version)),
        Err(e) => {
            if user_initiated {
                with_model(app, |m| m.error = Some(l!("Update-Prüfung fehlgeschlagen: %@", e)));
            }
        }
    }
    changed(app);
}

/// How often budgets and project status are reloaded while KeyZapper runs (e.g. only in the tray).
const BACKGROUND_REFRESH: Duration = Duration::from_secs(10 * 60);

/// Start-up work that may take a while: status, budgets, updates, CLI versions, keys left in the keychain.
fn start_background(app: &AppHandle) {
    refresh(app);
    refresh_usage(app);
    refresh_mcp_catalog(app);
    let handle = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(BACKGROUND_REFRESH);
        refresh(&handle);
        refresh_usage(&handle);
        refresh_mcp_catalog(&handle);
    });
    sync_office_later(app, false);
    with_model(app, |m| m.auto_apply_claude_desktop());
    auto_install_claude_desktop(app, false);
    let handle = app.clone();
    std::thread::spawn(move || check_updates(&handle, false));
    let handle = app.clone();
    std::thread::spawn(move || {
        let minimum = with_model(&handle, |m| m.minimum_cli_version());
        let outdated: Vec<String> = keyzapper_core::cli::outdated_installations(&minimum)
            .into_iter()
            .map(|(path, version)| format!("{} {version}", keyzapper_core::paths::abbreviate_home(&path)))
            .collect();
        let candidates = with_model(&handle, |m| {
            m.outdated_clis = outdated;
            m.profiles_without_key()
        });
        let in_keychain: Vec<String> = candidates.into_iter().filter(|id| legacy_keychain::has_key(id)).collect();
        with_model(&handle, |m| m.keychain_profiles = in_keychain);
        changed(&handle);
    });
}

// MARK: Commands

// Acquire the shared permit BEFORE cloning model inputs, under the very same UI
// lock as delete/deactivate/save/import. All Hermes I/O runs after releasing it.
fn hermes_inputs(app: &AppHandle) -> Result<(HermesOperation, HermesInputs), String> {
    let (operation, mut inputs) = with_model(app, |m| {
        let operation = m.hermes_controller.begin()?;
        let inputs = HermesInputs {
            profiles: m.state.profiles.clone(), policy: m.config.clone(), disabled: m.is_disabled(), helper: None,
            targets: m.hermes_controller.view().targets.into_iter().map(|entry| entry.target).collect(),
            diagnostics: m.hermes_controller.view().diagnostics,
        };
        Ok::<_, String>((operation, inputs))
    })?;
    inputs.helper = model::helper_path().map(|p| p.to_string_lossy().into_owned());
    Ok((operation, inputs))
}

#[tauri::command]
async fn hermes_refresh(app: AppHandle) -> View {
    blocking(move || {
        let Ok((operation, mut inputs)) = hermes_inputs(&app) else { return view(&app) };
        changed(&app); // Discovery can block; publish busy before any CLI work.
        let root = std::env::var_os("HERMES_HOME").map(std::path::PathBuf::from);
        let found = keyzapper_core::hermes::cli::discover(root.as_deref());
        inputs.diagnostics = found.diagnostics;
        inputs.targets = found.executables.iter().flat_map(|executable| found.homes.iter().map(move |home| {
            keyzapper_core::hermes::HermesTarget { executable: executable.clone(), home: home.clone(), display_name: home.file_name().unwrap_or_default().to_string_lossy().into_owned() }
        })).collect();
        let _ = operation.refresh(&keyzapper_core::hermes::cli::HermesCli::new(), &inputs, |_| changed(&app));
        view(&app)
    }).await
}

#[tauri::command]
async fn hermes_preview(
    app: AppHandle,
    action: String,
    target: keyzapper_core::hermes::HermesTarget,
    selection: Option<keyzapper_core::hermes::HermesSelection>,
) -> Result<HermesPreview, String> {
    blocking(move || {
        let action = HermesAction::parse(&action)?;
        let (operation, inputs) = hermes_inputs(&app)?;
        operation.preview(&keyzapper_core::hermes::cli::HermesCli::new(), &inputs, action, &target, selection, std::time::Instant::now(), |_| changed(&app))
    }).await
}

#[tauri::command]
async fn hermes_connect(app: AppHandle, selection: keyzapper_core::hermes::HermesSelection, confirmation_id: String) -> Result<View, String> {
    hermes_apply(app, HermesAction::Connect, selection.target.clone(), Some(selection), confirmation_id).await
}

async fn hermes_apply(app: AppHandle, action: HermesAction, target: keyzapper_core::hermes::HermesTarget, selection: Option<keyzapper_core::hermes::HermesSelection>, confirmation_id: String) -> Result<View, String> {
    blocking(move || {
        let (operation, inputs) = hermes_inputs(&app)?;
        operation.apply(&keyzapper_core::hermes::cli::HermesCli::new(), &inputs, action, &target, selection, &confirmation_id, std::time::Instant::now(), |_| changed(&app))?;
        Ok(view(&app))
    }).await
}

#[tauri::command]
async fn hermes_disconnect(app: AppHandle, target: keyzapper_core::hermes::HermesTarget, confirmation_id: String) -> Result<View, String> {
    hermes_apply(app, HermesAction::Disconnect, target, None, confirmation_id).await
}

#[tauri::command]
async fn hermes_recover(app: AppHandle, target: keyzapper_core::hermes::HermesTarget, confirmation_id: String) -> Result<View, String> {
    hermes_apply(app, HermesAction::Recover, target, None, confirmation_id).await
}

#[tauri::command]
fn translations() -> HashMap<String, String> {
    keyzapper_core::i18n::english().clone()
}

#[tauri::command]
fn get_view(app: AppHandle) -> View {
    view(&app)
}

#[tauri::command]
async fn refresh_all(app: AppHandle) -> View {
    blocking(move || {
        refresh(&app);
        refresh_usage(&app);
        // An explicit refresh also tries Claude in Office again if it failed before.
        if with_model(&app, |m| m.office_failed()) {
            sync_office_later(&app, true);
        }
        // And restores the Claude Desktop setup IT asks for, and its installation if that failed.
        with_model(&app, |m| m.auto_apply_claude_desktop());
        auto_install_claude_desktop(&app, true);
        view(&app)
    })
    .await
}

#[tauri::command]
fn dismiss_notice(app: AppHandle) -> View {
    with_model(&app, |m| m.notice = None);
    view(&app)
}

#[tauri::command]
fn dismiss_error(app: AppHandle) -> View {
    with_model(&app, |m| m.error = None);
    view(&app)
}

/// Runs a model action off the main thread, then refreshes the status.
async fn act(app: AppHandle, action: impl FnOnce(&mut Model) + Send + 'static) -> View {
    blocking(move || {
        with_model(&app, action);
        refresh(&app);
        view(&app)
    })
    .await
}

#[tauri::command]
async fn save_profile(app: AppHandle, input: ProfileInput) -> (bool, View) {
    blocking(move || {
        let ok = with_model(&app, |m| m.save_profile_input(input));
        refresh(&app);
        refresh_budgets(&app, None);
        refresh_subscription(&app);
        if ok {
            sync_office_later(&app, true);
        }
        (ok, view(&app))
    })
    .await
}

#[tauri::command]
async fn store_key(app: AppHandle, profile_id: String, key: String) -> View {
    blocking(move || {
        with_model(&app, |m| m.store_key(&profile_id, &key));
        refresh(&app);
        refresh_budgets(&app, Some(&profile_id));
        view(&app)
    })
    .await
}

#[tauri::command]
async fn delete_profile(app: AppHandle, profile_id: String) -> View {
    let handle = app.clone();
    let view = act(app, move |m| m.delete_profile(&profile_id)).await;
    sync_office_later(&handle, true);
    view
}

/// Copies the key marked as confidential and clears it after 60 s if unchanged.
#[tauri::command]
async fn copy_key(app: AppHandle, profile_id: String) -> View {
    blocking(move || {
        if let Some((key, name)) = with_model(&app, |m| m.key_for_copy(&profile_id)) {
            match clipboard::copy_concealed(&key) {
                Some(marker) => {
                    with_model(&app, |m| m.notice = Some(l!("Key von „%@“ kopiert. Er wird nach 60 Sekunden aus der Zwischenablage entfernt.", name)));
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_secs(60));
                        clipboard::clear_if_unchanged(marker);
                    });
                }
                None => with_model(&app, |m| m.error = Some(l!("Die Zwischenablage ist nicht verfügbar."))),
            }
        }
        view(&app)
    })
    .await
}

#[derive(serde::Serialize)]
struct CheckOutcome {
    success: bool,
    message: String,
}

#[tauri::command]
async fn test_connection(app: AppHandle, profile_id: String) -> Result<CheckOutcome, String> {
    blocking(move || {
        if let Some(previous) = with_model(&app, |m| m.state.profile(&profile_id).filter(|p| p.is_subscription()).map(|_| m.subscription.clone())) {
            let status = subscription::status(previous.as_ref(), Duration::from_secs(10));
            let outcome = match (&status.problem, &status.plan) {
                _ if !status.logged_in => CheckOutcome { success: false, message: l!("Nicht bei Claude angemeldet – in Claude Code „/login“ ausführen.") },
                (Some(problem), _) => CheckOutcome { success: false, message: problem.clone() },
                (None, Some(plan)) => CheckOutcome { success: true, message: l!("Bei Claude angemeldet (%@).", plan) },
                (None, None) => CheckOutcome { success: true, message: l!("Bei Claude angemeldet.") },
            };
            with_model(&app, |m| m.subscription = Some(status));
            changed(&app);
            return Ok(outcome);
        }
        let (endpoint, models, key, sso) = with_model(&app, |m| {
            let profile = m.state.profile(&profile_id).cloned().ok_or_else(|| l!("Unbekanntes Profil: %@", profile_id))?;
            let key = if profile.is_sso() { m.sso_token(&profile) } else { m.keys.read(&profile_id).map_err(|e| e.to_string()) };
            Ok::<_, String>((profile.endpoint.clone(), profile.configured_models(), key, profile.is_sso()))
        })?;
        let key = match key {
            Ok(key) => key,
            Err(message) if sso => return Ok(CheckOutcome { success: false, message }),
            Err(message) => return Err(message),
        };
        let result = gateway::run(&endpoint, &key, &models);
        if sso && result == CheckResult::Unauthorized {
            with_model(&app, |m| m.sso_expired.insert(profile_id.clone()));
            return Ok(CheckOutcome { success: false, message: l!("Kein aktiver Zugang – Sitzung abgelaufen, bitte neu anmelden.") });
        }
        refresh_budgets(&app, Some(&profile_id));
        Ok(CheckOutcome { success: result.is_success(), message: result.message() })
    })
    .await
}

/// Lists the gateway's model names for a key: the one typed in the editor, else the stored key of the profile.
#[tauri::command]
async fn available_models(app: AppHandle, endpoint: String, typed_key: String, profile_id: Option<String>) -> Result<Vec<String>, String> {
    blocking(move || {
        let endpoint = model::valid_endpoint(&endpoint).ok_or_else(|| l!("Bitte eine vollständige https-URL angeben (http nur für localhost)."))?;
        let sso = with_model(&app, |m| profile_id.as_deref().and_then(|id| m.state.profile(id)).is_some_and(|p| p.is_sso()));
        let key = with_model(&app, |m| {
            if !m.config.is_endpoint_allowed(&endpoint) {
                return Err(l!(
                    "Der Endpunkt %@ ist laut Firmenrichtlinie nicht freigegeben. Erlaubt: %@",
                    keyzapper_core::managed::host_of(&endpoint).unwrap_or_default(),
                    m.config.allowed_gateway_hosts.join(", ")
                ));
            }
            match (typed_key.trim(), profile_id) {
                ("", Some(id)) if m.state.profile(&id).is_some_and(|p| p.is_sso()) => {
                    let profile = m.state.profile(&id).cloned().unwrap();
                    m.sso_token(&profile).map_err(|_| l!("Kein aktiver Zugang – Sitzung abgelaufen, bitte neu anmelden."))
                }
                ("", Some(id)) => m.keys.read(&id).map_err(|e| e.to_string()),
                ("", None) => Err(l!("Für den Modellabruf wird ein Key benötigt.")),
                (typed, _) => Ok(typed.to_string()),
            }
        })?;
        match gateway::models(&endpoint, &key) {
            Ok(models) if models.is_empty() => Err(l!("Das Gateway hat keine Modelle für diesen Key gemeldet.")),
            Ok(models) => Ok(models),
            Err(CheckResult::Unauthorized) if sso => Err(l!("Kein aktiver Zugang – Sitzung abgelaufen, bitte neu anmelden.")),
            Err(failure) => Err(failure.message()),
        }
    })
    .await
}

/// Browser sign-in (OIDC authorization code + PKCE); blocks up to `LOGIN_TIMEOUT` on a background thread.
#[tauri::command]
async fn sso_login(app: AppHandle, profile_id: String) -> View {
    blocking(move || {
        let Some((profile, tokens)) = with_model(&app, |m| {
            let profile = m.state.profile(&profile_id).filter(|p| p.is_sso()).cloned()?;
            m.sso_logging_in.insert(profile_id.clone()).then(|| (profile, m.tokens.clone()))
        }) else {
            return view(&app);
        };
        changed(&app);
        let opener = app.clone();
        let result = oidc::login(
            &profile,
            &tokens,
            |url| opener.opener().open_url(url, None::<&str>).map_err(|e| Error::Oidc(e.to_string())),
            oidc::LOGIN_TIMEOUT,
        );
        with_model(&app, |m| {
            m.sso_logging_in.remove(&profile_id);
            match result {
                Ok(()) => {
                    m.sso_expired.remove(&profile_id);
                    m.notice = Some(l!("Angemeldet bei „%@“.", profile.name));
                }
                Err(e) => m.error = Some(e.to_string()),
            }
        });
        refresh(&app);
        refresh_mcp_catalog(&app);
        view(&app)
    })
    .await
}

#[tauri::command]
async fn sso_logout(app: AppHandle, profile_id: String) -> View {
    act(app, move |m| {
        m.sso_logout(&profile_id);
        m.notice = Some(l!("Abgemeldet."));
    })
    .await
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SuggestedModels {
    opus: Option<String>,
    sonnet: Option<String>,
    haiku: Option<String>,
}

#[tauri::command]
fn suggest_models(models: Vec<String>) -> SuggestedModels {
    let (opus, sonnet, haiku) = keyzapper_core::models::suggest_models(&models);
    SuggestedModels { opus, sonnet, haiku }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PickedFolder {
    folder: String,
    root: String,
    existing_profile_id: Option<String>,
}

fn picked_folder(app: &AppHandle, folder: &str) -> PickedFolder {
    let root = keyzapper_core::settings::settings_root(folder);
    let existing_profile_id = with_model(app, |m| m.state.bindings.iter().find(|b| b.path == root).map(|b| b.profile_id.clone()));
    PickedFolder { folder: keyzapper_core::paths::canonical_path(folder), root, existing_profile_id }
}

#[tauri::command]
async fn pick_project_folder(app: AppHandle) -> Option<PickedFolder> {
    blocking(move || {
        let folder = app.dialog().file().set_title(l!("Projektordner")).blocking_pick_folder()?.into_path().ok()?;
        Some(picked_folder(&app, &folder.to_string_lossy()))
    })
    .await
}

/// A folder dragged onto the window.
#[tauri::command]
async fn inspect_folder(app: AppHandle, path: String) -> Result<PickedFolder, String> {
    blocking(move || {
        if !std::path::Path::new(&path).is_dir() {
            return Err(l!("Bitte einen Ordner hineinziehen, keine Datei."));
        }
        Ok(picked_folder(&app, &path))
    })
    .await
}

#[tauri::command]
async fn bind(app: AppHandle, folder: String, profile_id: String) -> View {
    act(app, move |m| m.bind(&folder, &profile_id)).await
}

#[tauri::command]
async fn reapply(app: AppHandle, binding_id: String) -> View {
    act(app, move |m| m.reapply(&binding_id)).await
}

#[tauri::command]
async fn reapply_many(app: AppHandle, binding_ids: Vec<String>) -> View {
    act(app, move |m| m.reapply_many(&binding_ids)).await
}

/// The project uses the models of one tenant of its profile's gateway; `None` = as in the profile.
#[tauri::command]
async fn set_binding_tenant(app: AppHandle, binding_id: String, tenant: Option<String>) -> View {
    blocking(move || {
        let profile_id = with_model(&app, |m| {
            m.set_binding_tenant(&binding_id, tenant);
            m.state.bindings.iter().find(|b| b.id == binding_id).map(|b| b.profile_id.clone())
        });
        refresh(&app);
        // The tenant of a project is one of the budgets shown for its profile.
        if let Some(profile_id) = profile_id {
            refresh_budgets(&app, Some(&profile_id));
        }
        view(&app)
    })
    .await
}

#[tauri::command]
async fn toggle_pool(app: AppHandle, binding_id: String) -> View {
    act(app, move |m| m.toggle_pool(&binding_id)).await
}

#[tauri::command]
async fn unbind(app: AppHandle, binding_id: String) -> View {
    act(app, move |m| {
        m.unbind(&binding_id);
    })
    .await
}

// MARK: Agent instruction files

#[tauri::command]
async fn scan_repository(app: AppHandle, binding_id: String) -> Result<agents::RepoScan, String> {
    blocking(move || {
        let path = with_model(&app, |m| m.state.bindings.iter().find(|b| b.id == binding_id).map(|b| b.path.clone()))
            .ok_or_else(|| l!("Unbekannte Zuordnung."))?;
        if !std::path::Path::new(&path).is_dir() {
            return Err(l!("Ordner nicht gefunden: %@", keyzapper_core::paths::abbreviate_home(&path)));
        }
        Ok(agents::scan(&path))
    })
    .await
}

/// Asks the profile's gateway model for an assessment, model advice and an `AGENTS.md`; nothing is written yet.
#[tauri::command]
async fn generate_agents_file(app: AppHandle, binding_id: String, options: agents::Options) -> Result<agents::Proposal, String> {
    blocking(move || {
        let (profile, path, key) = with_model(&app, |m| m.analysis_access(&binding_id))?;
        let available = match gateway::models(&profile.endpoint, &key) {
            Ok(models) => models,
            Err(CheckResult::Unauthorized) if profile.is_sso() => {
                with_model(&app, |m| m.sso_expired.insert(profile.id.clone()));
                return Err(l!("Kein aktiver Zugang – Sitzung abgelaufen, bitte neu anmelden."));
            }
            Err(failure @ (CheckResult::Unauthorized | CheckResult::BudgetExceeded | CheckResult::Unreachable(_))) => return Err(failure.message()),
            // Some gateways do not list models; the analysis call can still work.
            Err(_) => vec![],
        };
        let model = agents::analysis_model(&options.model, &profile, &available)
            .ok_or_else(|| l!("Kein Modell für die Analyse gefunden. Bitte im Profil Modelle eintragen."))?;
        let prompt = agents::prompt(&agents::context(&agents::scan(&path)), &options, &profile, &available);
        let reply = gateway::complete(&profile.endpoint, &key, &model, agents::SYSTEM_PROMPT, &prompt, 16_000, Duration::from_secs(300));
        refresh_budgets(&app, Some(&profile.id));
        let reply = reply?;
        let proposal = agents::parse_reply(&reply.text).ok_or_else(|| l!("Die Antwort des Modells enthielt keine AGENTS.md. Bitte erneut versuchen."))?;
        Ok(agents::Proposal { model, input_tokens: reply.input_tokens, output_tokens: reply.output_tokens, ..proposal })
    })
    .await
}

#[tauri::command]
async fn write_agents_file(app: AppHandle, binding_id: String, content: String) -> (bool, View) {
    blocking(move || {
        let ok = with_model(&app, |m| m.write_agents_file(&binding_id, &content));
        (ok, view(&app))
    })
    .await
}

// MARK: MCP servers

#[tauri::command]
async fn save_mcp_server(app: AppHandle, input: McpServerInput) -> (bool, View) {
    blocking(move || {
        let ok = with_model(&app, |m| m.save_mcp_server(input));
        refresh(&app);
        (ok, view(&app))
    })
    .await
}

#[tauri::command]
async fn delete_mcp_server(app: AppHandle, server_id: String) -> View {
    act(app, move |m| m.delete_mcp_server(&server_id)).await
}

#[tauri::command]
async fn set_project_mcp_servers(app: AppHandle, binding_id: String, server_ids: Vec<String>) -> View {
    act(app, move |m| {
        m.set_project_mcp_servers(&binding_id, server_ids);
    })
    .await
}

#[tauri::command]
async fn reload_mcp_catalog(app: AppHandle) -> View {
    blocking(move || {
        refresh_mcp_catalog(&app);
        view(&app)
    })
    .await
}

// MARK: Claude tools

#[tauri::command]
async fn load_tools(app: AppHandle) -> View {
    blocking(move || {
        refresh_tools(&app);
        view(&app)
    })
    .await
}

/// Installs one tool from Anthropic's official sources; one at a time.
#[tauri::command]
async fn install_tool(app: AppHandle, tool: Tool) -> View {
    blocking(move || {
        let start = with_model(&app, |m| {
            let free = m.config.claude_tools_enabled && m.installing_tool.is_none();
            if free {
                m.installing_tool = Some(tool);
            }
            free
        });
        if !start {
            return view(&app);
        }
        changed(&app);
        let result = tools::install(tool);
        with_model(&app, |m| {
            m.installing_tool = None;
            match result {
                Ok(()) => m.notice = Some(Model::tool_installed_message(tool)),
                Err(e) => m.error = Some(l!("Installation fehlgeschlagen: %@", e)),
            }
        });
        refresh_tools(&app);
        view(&app)
    })
    .await
}

#[tauri::command]
async fn set_claude_desktop_profile(app: AppHandle, profile_id: Option<String>) -> View {
    act(app, move |m| m.set_claude_desktop_profile(profile_id)).await
}

/// Opens VS Code's "Disable Login Prompt" setting of the Claude Code extension.
#[tauri::command]
fn open_vscode_setting(app: AppHandle) {
    let _ = app.opener().open_url("vscode://settings/claudeCode.disableLoginPrompt", None::<&str>);
}

#[tauri::command]
async fn set_global_profile(app: AppHandle, profile_id: Option<String>) -> View {
    act(app, move |m| m.set_global_profile(profile_id)).await
}

#[tauri::command]
async fn repair_user_settings(app: AppHandle) -> View {
    act(app, |m| m.repair_user_settings()).await
}

#[tauri::command]
async fn set_disabled(app: AppHandle, disabled: bool) -> View {
    let handle = app.clone();
    let view = act(app, move |m| m.set_disabled(disabled)).await;
    sync_office_later(&handle, true);
    view
}

#[tauri::command]
async fn restore_from_backup(app: AppHandle) -> View {
    let handle = app.clone();
    let view = act(app, |m| m.restore_from_backup()).await;
    sync_office_later(&handle, true);
    view
}

#[tauri::command]
async fn discard_backup(app: AppHandle) -> View {
    act(app, |m| m.discard_backup()).await
}

#[tauri::command]
async fn migrate_keychain(app: AppHandle) -> View {
    blocking(move || {
        with_model(&app, |m| m.migrate_keychain());
        refresh(&app);
        refresh_budgets(&app, None);
        view(&app)
    })
    .await
}

/// Asks where to save, then writes the password-encrypted `.kzbackup` file. False if cancelled or failed.
#[tauri::command]
async fn export_backup(app: AppHandle, password: String) -> bool {
    blocking(move || {
        let name = format!("KeyZapper-Backup-{}.{}", keyzapper_core::time::today(), backup::FILE_EXTENSION);
        let Some(target) = app
            .dialog()
            .file()
            .add_filter("KeyZapper Backup", &[backup::FILE_EXTENSION])
            .set_file_name(&name)
            .blocking_save_file()
            .and_then(|p| p.into_path().ok())
        else {
            return false;
        };
        let payload = with_model(&app, |m| m.backup_payload());
        let result = backup::seal(&payload, &password, backup::DEFAULT_ITERATIONS)
            .map_err(|e| e.to_string())
            .and_then(|data| keyzapper_core::paths::write_atomic(&target, &data, true).map_err(|e| e.to_string()));
        with_model(&app, |m| match result {
            Ok(()) => {
                m.notice = Some(l!(
                    "Verschlüsseltes Backup gespeichert: %@ Profil(e), %@ Key(s), %@ Projekt(e).",
                    payload.state.profiles.len(),
                    payload.keys.len(),
                    payload.state.bindings.len()
                ));
                true
            }
            Err(e) => {
                m.error = Some(e);
                false
            }
        })
    })
    .await
}

#[tauri::command]
async fn pick_backup_file(app: AppHandle) -> Option<String> {
    blocking(move || {
        app.dialog()
            .file()
            .add_filter("KeyZapper Backup", &[backup::FILE_EXTENSION])
            .blocking_pick_file()
            .and_then(|p| p.into_path().ok())
            .map(|p| p.to_string_lossy().into_owned())
    })
    .await
}

#[tauri::command]
async fn import_backup(app: AppHandle, path: String, password: String) -> bool {
    blocking(move || {
        let opened = std::fs::read(&path).map_err(|e| e.to_string()).and_then(|data| backup::open(&data, &password).map_err(|e| e.to_string()));
        let ok = with_model(&app, |m| match opened {
            Ok(payload) => m.import_payload(payload),
            Err(e) => {
                m.error = Some(e);
                false
            }
        });
        refresh(&app);
        refresh_budgets(&app, None);
        sync_office_later(&app, true);
        ok
    })
    .await
}

#[tauri::command]
async fn check_for_updates(app: AppHandle) -> View {
    blocking(move || {
        check_updates(&app, true);
        view(&app)
    })
    .await
}

/// Beta releases on or off for update checks; switched on, KeyZapper looks for one right away.
#[tauri::command]
async fn set_beta_updates(app: AppHandle, enabled: bool) -> View {
    blocking(move || {
        with_model(&app, |m| m.set_beta_updates(enabled));
        if enabled {
            check_updates(&app, true);
        }
        view(&app)
    })
    .await
}

/// Downloads and verifies the installer and installs it; KeyZapper then starts again as the new version.
/// If installing in place fails, the package is opened in the system installer instead.
#[tauri::command]
async fn install_update(app: AppHandle) -> View {
    blocking(move || {
        let Some(release) = with_model(&app, |m| {
            if m.installing_update {
                return None;
            }
            m.installing_update = true;
            m.available_update.clone()
        }) else {
            return view(&app);
        };
        changed(&app);
        let path = match update::download_package(&release) {
            Ok(path) => path,
            Err(e) => {
                with_model(&app, |m| {
                    m.installing_update = false;
                    m.error = Some(l!("Update konnte nicht geladen werden: %@", e));
                });
                return view(&app);
            }
        };
        match update::install(&path, &release.version) {
            // Windows: the installer runs on and starts the new version itself; the app must not hold its files.
            Ok(()) if cfg!(windows) => app.exit(0),
            Ok(()) => app.restart(),
            Err(update::UpdateError::Cancelled) => with_model(&app, |m| m.notice = Some(update::UpdateError::Cancelled.to_string())),
            Err(e) => {
                let opened = app.opener().open_path(path.to_string_lossy(), None::<&str>);
                with_model(&app, |m| match opened {
                    Ok(()) => {
                        m.notice = Some(l!("Installer für KeyZapper %@ geöffnet. Nach der Installation KeyZapper neu starten.", release.version));
                        m.available_update = None;
                    }
                    Err(_) => m.error = Some(e.to_string()),
                });
            }
        }
        with_model(&app, |m| m.installing_update = false);
        view(&app)
    })
    .await
}

#[tauri::command]
fn dismiss_whats_new(app: AppHandle) -> View {
    with_model(&app, |m| m.dismiss_whats_new());
    view(&app)
}

// MARK: Tray popover

/// The popover's content height in logical pixels, so the window fits it.
#[tauri::command]
fn fit_usage_popover(app: AppHandle, height: f64) {
    tray::fit_popover(&app, height);
}

#[tauri::command]
fn hide_usage_popover(app: AppHandle) {
    tray::hide_popover(&app);
}

/// Opens the main window from the popover, optionally on one page (`projects`, …).
#[tauri::command]
fn open_main_window(app: AppHandle, page: Option<String>) {
    show_window(&app);
    if let Some(page) = page {
        let _ = app.emit_to("main", "navigate", page);
    }
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
}

#[tauri::command]
fn reveal(app: AppHandle, path: String) {
    let _ = app.opener().reveal_item_in_dir(path);
}

#[tauri::command]
fn open_url(app: AppHandle, url: String) {
    if url.starts_with("https://") {
        let _ = app.opener().open_url(url, None::<&str>);
    }
}

fn main() {
    let builder = tauri::Builder::default();
    // Starting KeyZapper again while it runs in the tray shows the existing window (macOS does this by itself).
    #[cfg(windows)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_window(app)));
    let app = builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let version = (!cfg!(debug_assertions)).then(|| app.package_info().version.to_string());
            let mut model = Model::load(version);
            model.vibrancy = app.get_webview_window("main").is_some_and(|w| apply_window_material(&w, NSVisualEffectMaterial::Sidebar, None));
            if let Some(popover) = app.get_webview_window(tray::POPOVER) {
                apply_window_material(&popover, NSVisualEffectMaterial::Popover, Some(12.0));
            }
            app.manage(Shared(Mutex::new(model)));
            if let Err(e) = tray::create(app.handle()) {
                eprintln!("KeyZapper: tray icon unavailable: {e}");
            }
            start_background(app.handle());
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == tray::POPOVER {
                return tray::popover_event(window, event);
            }
            // Closing only hides the window while the tray icon can bring it back.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if tray::exists(window.app_handle()) {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            translations,
            hermes_refresh,
            hermes_preview,
            hermes_connect,
            hermes_disconnect,
            hermes_recover,
            get_view,
            refresh_all,
            dismiss_notice,
            dismiss_error,
            save_profile,
            store_key,
            delete_profile,
            copy_key,
            sso_login,
            sso_logout,
            test_connection,
            available_models,
            suggest_models,
            pick_project_folder,
            inspect_folder,
            bind,
            reapply,
            reapply_many,
            set_binding_tenant,
            toggle_pool,
            unbind,
            scan_repository,
            generate_agents_file,
            write_agents_file,
            save_mcp_server,
            delete_mcp_server,
            set_project_mcp_servers,
            reload_mcp_catalog,
            load_tools,
            install_tool,
            set_claude_desktop_profile,
            open_vscode_setting,
            set_global_profile,
            repair_user_settings,
            set_disabled,
            restore_from_backup,
            discard_backup,
            migrate_keychain,
            export_backup,
            pick_backup_file,
            import_backup,
            check_for_updates,
            set_beta_updates,
            install_update,
            dismiss_whats_new,
            fit_usage_popover,
            hide_usage_popover,
            open_main_window,
            quit_app,
            reveal,
            open_url,
        ])
        .build(tauri::generate_context!())
        .expect("error while running KeyZapper");
    app.run(|_app, _event| {
        // Clicking the Dock icon shows the hidden window again.
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen { .. } = _event {
            show_window(_app);
        }
    });
}
