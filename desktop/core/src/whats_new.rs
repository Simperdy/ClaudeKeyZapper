//! "What's new" after an update: highlights per release and the last version whose highlights were shown.

use crate::l;
use crate::update::compare_versions;
use serde::Serialize;
use std::cmp::Ordering;

const SEEN_FILE: &str = "seen-version";

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Release {
    pub version: String,
    pub items: Vec<String>,
}

/// Highlights per release, newest first (German source strings, translated by `l!`).
fn notes() -> Vec<(&'static str, Vec<String>)> {
    vec![
        (
            "1.5.5",
            vec![
                l!("Projekte in WSL: Ordner unter \\\\wsl.localhost lassen sich zuordnen; Claude Code in WSL holt den Key über den Windows-Helper per WSL-Interop, Git-Root und Status kommen aus der Distribution."),
            ],
        ),
        (
            "1.5.4",
            vec![
                l!("Claude in Office und Claude Desktop (SSO) lassen sich jetzt auch bei Profilen aus Intune im Profil einschalten, solange die IT sie dort nicht selbst setzt."),
                l!("Update-Prüfung und Downloads laufen jetzt über den Proxy der Firma: aus „ProxyURL“ per Intune, den Proxy-Umgebungsvariablen oder den Systemeinstellungen."),
                l!("Hinter einem Proxy mit TLS-Inspektion: „IgnoreDownloadCertificateErrors“ per Intune lässt Update-Prüfung und Downloads auch mit nicht vertrauenswürdigem Zertifikat zu; die Pakete bleiben durch ihre Signatur geschützt."),
            ],
        ),
        (
            "1.5.3",
            vec![
                l!("KeyZapper braucht im Tray praktisch keine CPU mehr: Versteckte Fenster bekommen keine Ereignisse mehr und holen ihren Stand erst beim Einblenden."),
            ],
        ),
        (
            "1.5.1",
            vec![
                l!("Claude Desktop meldet sich per SSO jetzt mit „external-idp“ an (ab Claude Desktop 2.7032.0); für ältere Versionen gibt es im Profil die Einstellung „Älter als 2.7032.0“."),
                l!("Claude Desktop von der IT: KeyZapper installiert es auf Wunsch im Hintergrund und richtet die Konfiguration der Firma ohne Klick ein, auch mit weiteren Claude-Desktop-Einstellungen."),
                l!("Claude Desktop wird jetzt auch eingerichtet, wenn die IT nur Update- oder Proxy-Einstellungen verteilt, und beim Entfernen wird die zuvor aktive Konfiguration wieder aktiv."),
            ],
        ),
        (
            "1.5.0",
            vec![
                l!("MCP-Server zentral verwalten: Die IT verteilt per Intune nur die Adresse einer Liste auf ihrem Server; KeyZapper lädt sie mit deiner SSO-Anmeldung und bietet die Server für jedes Projekt an."),
                l!("Neue Seite Claude-Tools: Claude Code, die Erweiterung für VS Code, das Plugin für IntelliJ IDEA und Claude Desktop mit einem Klick installieren – ohne Administratorrechte."),
                l!("Claude Desktop mit einem Key- oder SSO-Profil nutzen: KeyZapper richtet das Gateway ein; Key oder SSO-Token holt Claude Desktop über den Helper, oder es meldet sich mit dem OIDC-Client des Gateways selbst per SSO an."),
                l!("Claude in Excel, Word und PowerPoint: Ein Profil richtet Claude für Microsoft 365 als Add-in ein, das sich per Entra-SSO am Firmen-Gateway anmeldet – ohne gespeichertes Geheimnis."),
                l!("Tenant pro Projekt: Bei SSO-Gateways mit mehreren Tenants wählt jedes Projekt die Modelle seines Tenants, und das Budget wird pro Tenant angezeigt."),
                l!("Neuer Schalter „Beta-Versionen“ in der Seitenleiste: Ist er an, bietet die Update-Suche auch Beta-Versionen an."),
            ],
        ),
        (
            "1.4.3",
            vec![
                l!("Budgetbalken zeigen jetzt, was übrig ist, und werden mit dem Verbrauch kürzer."),
                l!("SSO-Profile zeigen jetzt ihr verbleibendes Budget, z. B. an LiteLLM – ohne eigenen Key das Budget des angemeldeten Nutzers."),
            ],
        ),
        (
            "1.4.1",
            vec![
                l!("Claude-Abo als Profiltyp: Projekte nutzen dein Claude Pro, Max oder Team über die Anmeldung von Claude Code – ohne Gateway und Key."),
                l!("Ein Klick aufs Symbol in der Menüleiste bzw. im Infobereich zeigt den Verbrauch auf einen Blick: Limits des Claude-Abos und Budgets aller Keys. Rechtsklick öffnet das bisherige Menü."),
            ],
        ),
        (
            "1.4.0",
            vec![
                l!("Neues Design: Seitenleiste, Budgetbalken mit Ampel, Projektstatus auf einen Blick mit „Alle reparieren“ – und Projektordner einfach auf ein Profil ziehen."),
                l!("Nach dem Schließen läuft KeyZapper in der Menüleiste bzw. im Infobereich weiter: Budgets im Blick und das Profil je Projekt mit einem Klick wechseln."),
                l!("MCP-Server aus Vorlagen wie GitHub, Notion oder Sentry anlegen und je Projekt per Schalter einschalten. Unter Windows starten npx-Server automatisch über cmd /c."),
                l!("MCP-Server einrichten und je Projekt auswählen: KeyZapper schreibt sie in die .mcp.json des Repositorys und schaltet sie frei, Claude Code lädt sie dort automatisch."),
                l!("Updates installieren sich nach Bestätigung mit Administratorrechten selbst, danach startet KeyZapper automatisch neu."),
                l!("Nach einem Update zeigt KeyZapper einmal, was neu ist."),
            ],
        ),
        (
            "1.3.0",
            vec![
                l!("Für Agenten optimieren: Das ✨-Symbol am Projekt analysiert das Repository per KI und erzeugt eine AGENTS.md nach deinen Zielen, z. B. Kosteneffizienz – samt Empfehlung, welches Modell sich wofür eignet."),
                l!("Symbole zeigen beim Überfahren ihren Namen oder einen kurzen Hilfetext."),
                l!("Updates werden vor der Installation anhand ihrer Signatur geprüft."),
                l!("Gateways, an die Keys gehen, müssen https verwenden (http nur für localhost)."),
            ],
        ),
    ]
}

fn without_suffix(version: &str) -> &str {
    version.split('-').next().unwrap_or(version)
}

/// Highlights after updating from `previous` to `current`, newest first: the releases after the previous one up to the
/// current one, compared without beta suffix (the betas of a release share its notes). Without a previous version
/// (updated from a release before this dialog existed) the newest up to the current one. At most three releases.
pub fn since(previous: Option<&str>, current: &str) -> Vec<Release> {
    let current = without_suffix(current);
    notes()
        .into_iter()
        .filter(|(version, _)| compare_versions(version, current) != Ordering::Greater)
        .filter(|(version, _)| previous.is_none_or(|p| compare_versions(version, without_suffix(p)) == Ordering::Greater))
        .take(3)
        .map(|(version, items)| Release { version: version.to_string(), items })
        .collect()
}

pub fn seen_version() -> Option<String> {
    let text = std::fs::read_to_string(crate::paths::data_dir().join(SEEN_FILE)).ok()?;
    Some(text.trim().to_string()).filter(|v| !v.is_empty())
}

pub fn mark_seen(version: &str) -> std::io::Result<()> {
    crate::paths::write_atomic(&crate::paths::data_dir().join(SEEN_FILE), version.as_bytes(), false)
}
