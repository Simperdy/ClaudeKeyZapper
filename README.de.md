[English](README.md) · **Deutsch**

<img src="Resources/AppIcon.svg" width="96" alt="KeyZapper-Icon">

# KeyZapper

[![Build](https://github.com/bl0rb/ClaudeKeyZapper/actions/workflows/desktop.yml/badge.svg)](https://github.com/bl0rb/ClaudeKeyZapper/actions/workflows/desktop.yml)
[![Release](https://github.com/bl0rb/ClaudeKeyZapper/actions/workflows/release.yml/badge.svg)](https://github.com/bl0rb/ClaudeKeyZapper/actions/workflows/release.yml)
[![Security audit](https://github.com/bl0rb/ClaudeKeyZapper/actions/workflows/security.yml/badge.svg)](https://github.com/bl0rb/ClaudeKeyZapper/actions/workflows/security.yml)
[![Aktuelle Version](https://img.shields.io/github/v/release/bl0rb/ClaudeKeyZapper?display_name=tag&label=release)](https://github.com/bl0rb/ClaudeKeyZapper/releases/latest)
[![Lizenz: MIT](https://img.shields.io/badge/Lizenz-MIT-blue)](LICENSE)
[![Plattformen](https://img.shields.io/badge/Plattformen-macOS%2014%2B%20%7C%20Windows%2010%2B-lightgrey)](#voraussetzungen)
[![Signierte Updates](https://img.shields.io/badge/Updates-minisign--signiert-success)](docs/security-audit.md#updates-and-installers)
[![Key-Ablage](https://img.shields.io/badge/Keys-lokale%20Datei%20%C2%B7%20nur%20Besitzer%20%C2%B7%20nur%20https-success)](docs/security-audit.md#key-storage)
[![Sicherheitsreview](https://img.shields.io/badge/Sicherheitsreview-2026--11--03-informational)](docs/security-audit.md)

Desktop-App für macOS und Windows, die freigegebene, projektbezogene LiteLLM-Keys in einer lokalen Key-Datei verwaltet und lokalen Projektordnern zuordnet.
Claude Code (VS Code, IntelliJ, CLI) verwendet danach in jedem Projekt automatisch den passenden Key. Mehrere gleichzeitig
offene Projekte arbeiten unabhängig voneinander.

**Einmal zuordnen, danach nie wieder Keys wechseln.** Claude Code holt sich den passenden Key über `keyzapper-helper`
selbst, auch wenn KeyZapper geschlossen ist. Die App erklärt das unter „So funktioniert’s“.

![KeyZapper-Übersicht mit zwei Key-Profilen, einem SSO-Profil und zugeordneten Projekten](docs/screenshots/keyzapper-overview.de.png)

<sub>Screenshot mit ausgedachten Beispieldaten.</sub>

```
Claude Code ──apiKeyHelper──▶ keyzapper-helper ──▶ lokale Key-Datei
     │
     └──── Anfragen mit Projekt-Key ────▶ LiteLLM ──▶ Amazon Bedrock
```

## Warum KeyZapper

LLM-Kosten müssen projektbezogen erfasst und in Projektbudgets gehalten werden. Dafür stellt eine eigens entwickelte
Middleware vor LiteLLM API-Keys pro Projekt aus. Jeder Key trägt das Budget und die freigegebenen Modelle seines
Projekts, sodass jede Anfrage mit diesem Key auf das Projekt gebucht wird. KeyZapper sorgt dafür, dass Claude Code immer
den Key des Projekts verwendet, in dem gerade gearbeitet wird. So landen die Kosten ohne manuelles Key-Wechseln auf dem
richtigen Projekt.

## Funktionen

* Profile mit Name, LiteLLM-Endpunkt (https; einfaches http nur für ein Gateway auf localhost), optionalem Modellalias und Key. Der Key liegt ausschließlich in der lokalen Key-Datei `keys.json` im App-Datenordner (keine Schlüsselbund- oder Anmeldeinformationsverwaltungs-Abfragen).
* Projektordner einem Profil zuordnen. Die App ergänzt nur `.claude/settings.local.json` und schließt die Datei per `.git/info/exclude` von Git aus.
* Status je Projekt: aktiv, abweichend, Ordner fehlt, Key fehlt, Konflikte mit anderen Einstellungen. Eine Zusammenfassung zeigt, was Aufmerksamkeit braucht; *Alle reparieren* schreibt die Einstellungen aller betroffenen Projekte neu.
* Projektordner aus Finder oder Explorer auf ein Profil ziehen, um sie zuzuordnen.
* MCP-Server (stdio, HTTP oder SSE) einmal definieren – oder aus Vorlagen wie GitHub, Notion oder Sentry – und je Projekt einschalten. Die App schreibt sie in die `.mcp.json` des Repositorys (von Git ausgeschlossen) und schaltet sie in `.claude/settings.local.json` frei, damit Claude Code sie dort automatisch lädt. Remote-Server mit OAuth (z. B. Weave-MCP über Entra ID oder Keycloak) bekommen Client-ID, Callback-Port und Scopes; den Browser-Login übernimmt Claude Code.
* Zentrale MCP-Liste: Die IT veröffentlicht die MCP-Server als eine JSON-Datei auf ihrem eigenen Server (z. B. dem SSO-Server); KeyZapper lädt sie und bietet die Server schreibgeschützt für jedes Projekt an (siehe [Zentrale MCP-Liste](#zentrale-mcp-liste)).
* Seite *Claude-Tools*: installiert Claude Code (CLI), die VS Code-Erweiterung, das IntelliJ-Plugin und Claude Desktop aus den offiziellen Quellen von Anthropic und verbindet Claude Desktop mit einem Key- oder SSO-Profil (siehe [Claude-Tools](#claude-tools)).
* Verbindungstest gegen LiteLLM (ungültiger oder gesperrter Key, Rate-Limit, Budget, Gateway nicht erreichbar).
* Verbleibendes Budget je Key laut LiteLLM (`/key/info`): ein Balken, der ab 70 % Verbrauch orange und ab 90 % rot wird, Rest vom Limit und nächster Reset, bei Keys ohne Limit die bisherigen Ausgaben.
* Claude-Abo-Profile: Projekte nutzen dein Claude Pro, Max, Team oder Enterprise über die Anmeldung von Claude Code statt eines Gateway-Keys (siehe unten).
* Claude in Microsoft 365: Ein Profil kann das Claude-Add-in für Excel, Word und PowerPoint hinter dem Firmen-Gateway mit Entra-SSO einrichten (siehe unten).
* Symbol in der Menüleiste (macOS) bzw. im Infobereich (Windows): Ein Klick zeigt den Verbrauch auf einen Blick – Limits des Claude-Abos (Sitzung, Woche, je Modell) und das Budget jedes Keys; per Rechtsklick wechselst du das Profil eines Projekts, ohne das Fenster zu öffnen. Nach dem Schließen des Fensters läuft KeyZapper dort weiter.
* Rücknahme entfernt nur die Einträge, die die App selbst gesetzt hat.
* Seitenleiste mit Profilen, Projekten, MCP-Servern, Claude-Einstellungen und Backup. Alles lässt sich direkt ändern – über das ⋯-Menü oder per Rechtsklick; Keys lassen sich kopieren und ersetzen.
* Update-Prüfung gegen die GitHub-Releases mit Installation per Klick; die Version steht in der App und unter „Über KeyZapper“.
* Oberfläche auf Deutsch und Englisch (folgt der Systemsprache).
* Verschlüsseltes Backup und Wiederherstellen von Profilen, Zuordnungen und Keys (`.kzbackup`, passwortgeschützt).
* Steuerbar per Intune: vorgegebene Profile (auch mit Claude in Microsoft 365), Gateway-Allowlist, Standardwerte, CLI-Mindestversion, OneDrive-Backup, Update-Prüfung, zentrale MCP-Liste, Seite Claude-Tools.

## Voraussetzungen

* macOS 14 oder neuer oder Windows 10 oder neuer (mit WebView2-Runtime)
* Claude Code ≥ 2.1.288 (VS-Code-Erweiterung bzw. `claude`-CLI für IntelliJ). Ältere Versionen lesen die Projektzuordnung
  nur beim Start direkt im Projektordner; die App warnt in dem Fall.

## Nutzung

1. **Profil anlegen:** Name, LiteLLM-Endpunkt, Key und optional die Modellnamen des Gateways für Opus, Sonnet und Haiku. *Modelle vom Gateway laden* listet die Modelle, die der Key nutzen darf (LiteLLM `/v1/models`), und belegt die Stufen vor. Von der IT vorgegebene Profile sind bereits da; dort nur den Key eintragen.
2. **Projekt zuordnen:** Ordner wählen und Profil auswählen. Bei Git-Repositories gilt die Zuordnung für das ganze Repository
   samt Unterordnern und Worktrees. Claude Code liest die Projekteinstellungen nur am Root des Haupt-Checkouts.
3. **Claude-Sitzung im Projekt neu starten**, in VS Code eine neue Unterhaltung bzw. das Fenster neu laden.

Die Übersicht zeigt jedes Profil mit Endpunkt, Modell und dem zugewiesenen Key (maskiert, z. B. `••••7f3a`) samt allen
zugeordneten Projekten. Pro Projekt lassen sich das Profil wechseln, die Einstellungen neu schreiben und die Zuordnung entfernen.

**Key kopieren:** Der Key landet als „vertraulich“ markiert in der Zwischenablage, damit Clipboard-Manager ihn nicht speichern.
Nach 60 Sekunden wird er wieder entfernt, sofern inzwischen nichts anderes kopiert wurde.

**Keywechsel:** „Ändern“ ersetzt den Key; das wirkt sofort für neue Sitzungen. Laufende Sitzungen übernehmen den Key nach Ablauf des
Helper-Caches (Standard 5 min), nach einem HTTP 401 oder nach einem Neustart.

**Fehlt ein Key** oder ist er gesperrt, schlagen die Anfragen fehl. Claude Code weicht nie auf andere Zugangsdaten aus.

## SSO-Profile

KeyZapper bietet außerdem eine **experimentelle Hermes-Integration nur für exakt validierte Builds**; die macOS-/Firmenabnahme steht noch aus. Vor der Nutzung den [Hermes-Leitfaden zu Einrichtung, Wiederherstellung und Abnahme](docs/hermes-integration.md) lesen.

Profile können sich über einen statischen LiteLLM-Key (Typ Key) oder per OIDC-Anmeldung (Typ SSO) authentifizieren.
SSO erfordert einen OIDC-validierenden Proxy vor LiteLLM, der das Token (standardmäßig das Access-Token) prüft und den virtuellen Key in die Anfragen injiziert.

**SSO-Felder:** Issuer (https-URL), Client-ID (öffentlicher Client, kein Secret), Scope (Standard `openid profile offline_access`),
Token-Art (Standard *Access-Token*, oder *ID-Token* für Gateways, die das OIDC-ID-Token als Bearer-Token erwarten).
Für Microsoft Entra ID mit Access-Token Scope `api://<app-id>/.default offline_access` verwenden; ein Refresh-Token ist erforderlich.
Mit ID-Token muss der Scope `openid` (bei Entra ID auch `profile`, sonst fehlt der Claim `oid`) und `offline_access` enthalten;
der Ablauf kommt aus dem `exp`-Claim des Tokens.

**Anmeldung:** *Anmelden* öffnet den System-Browser für Authorization-Code-Flow mit PKCE. Die Redirect-URI ist eine Loopback-Adresse
`http://127.0.0.1:<zufälliger Port>/callback`. Die Redirect-URI registrieren:

* **Microsoft Entra ID:** App-Registrierung › Mobile und Desktop-Anwendungen › Redirect-URI: `http://127.0.0.1`
  (Entra ignoriert den Port bei Loopback-Umleitungen).
* **Keycloak:** Öffentlicher Client › Standard flow › PKCE S256 › Valid redirect URI: `http://127.0.0.1/*`.

**Tokens:** Refresh- und Access-Tokens werden lokal in `sso-tokens.json` im App-Datensatz-Ordner gespeichert (nur Besitzer, `0600`),
nie in Backups oder per „Key kopieren". Der Helper gibt ein gecachtes Access-Token zurück oder aktualisiert es einmal,
serialisiert sicher über parallele Sitzungen. Exit-Code 67 bedeutet abgelaufene Sitzung; in KeyZapper neu anmelden.

**Einstellungen:** `CLAUDE_CODE_API_KEY_HELPER_TTL_MS` im Umgebungsbereich des Profils auf einen Wert unter der Token-Lebensdauer
setzen (z. B. 900000 für 60–90 Min. Tokens). Für lange Streams über ein Gateway auch `API_TIMEOUT_MS`,
`CLAUDE_STREAM_IDLE_TIMEOUT_MS`, `CLAUDE_STREAM_FIRST_BYTE_TIMEOUT_MS` setzen.

**Einschränkungen:** SSO-Profile unterstützen keinen Budget-Killer-Pool und kein Key-Kopieren. Modell-Laden, Verbindungstest und
Budget-Anzeige (ein Budget je Tenant, siehe unten) verwenden das Access-Token.

## Mehrere Tenants unter einem SSO-Profil

Ein Gateway, das mehrere Tenants (Teams, Kostenstellen) bedient, führt seine Modelle als `<tenant>/<modell>`, zum Beispiel
`acme/claude-sonnet-5-5` und `globex/claude-sonnet-5-5`. Wer in zwei Tenants Zugang hat, behält trotzdem **ein** Profil und
**einen** Login: derselbe SSO-Token, derselbe Helper, dieselbe Base-URL. KeyZapper liest die Modellliste zusammen mit den Budgets
vom Gateway (`/v1/models`); enthält sie zwei oder mehr Tenants, zeigt jede Projektzeile eine Auswahl **Tenant**.

Die Wahl eines Tenants ersetzt nur die vier Modelleinstellungen dieses Projekts (`ANTHROPIC_MODEL` und
`ANTHROPIC_DEFAULT_{OPUS,SONNET,HAIKU}_MODEL`) durch die Modelle dieses Tenants: Jedes Modell des Profils wird zu
`<tenant>/<name>`, wenn der Tenant es hat, sonst gilt das neueste Modell des Tenants für diese Stufe. Beim Standardmodell bleiben
Stufen-Aliase wie `sonnet` oder `opusplan` erhalten; eine Modell-ID zieht mit oder fällt auf das Sonnet-Modell des Tenants zurück.
Helper (`--profile <ID>`) und `ANTHROPIC_BASE_URL` bleiben gleich, Claude Code ruft im Projekt also nach einer Anmeldung den
gewählten Tenant auf. *Wie im Profil* kehrt zu den eigenen Modellen des Profils zurück.

Die Wahl wird mit der Projektzuordnung gespeichert und überlebt *Erneut anwenden*, Profiländerungen und Backups; wird das Projekt
einem anderen Profil zugeordnet, entfällt sie. Claude-Abo-Profile haben keine Tenants.

## Claude-Abo-Profile

Ein Profil vom Typ *Claude-Abo* hat weder Endpunkt noch Key: Zugeordnete Projekte nutzen das Claude-Konto, mit dem Claude
Code angemeldet ist (`/login`). KeyZapper schreibt `apiKeyHelper`, `ANTHROPIC_BASE_URL`, `ANTHROPIC_API_KEY`,
`ANTHROPIC_AUTH_TOKEN` und die Gateway-Modellvariablen als leere Werte in `.claude/settings.local.json`, damit ein
Standardprofil oder Shell-Variablen dort nicht greifen; ein optionales Standardmodell (z. B. `opus`) landet in `ANTHROPIC_MODEL`.

Für Tarif und Verbrauch liest KeyZapper die Anmeldung von Claude Code (macOS-Schlüsselbundeintrag „Claude Code-credentials“,
sonst `~/.claude/.credentials.json`) und fragt `api.anthropic.com/api/oauth/usage` ab – wie `/usage` in Claude Code. Das Token
wird nie erneuert, gespeichert oder kopiert; ist es abgelaufen (Claude Code erneuert es, sobald es läuft), zeigt KeyZapper den
letzten Stand. Der Endpunkt ist keine öffentliche API und kann sich ändern. Die Gateway-Allowlist gilt für diese Profile nicht,
und die Repository-Analyse benötigt ein Gateway-Profil.

## Claude in Microsoft 365 über das Profil

Ein Profil kann auch **Claude für Microsoft 365** einrichten, das Claude-Add-in für Excel, Word und PowerPoint, hinter dem
Firmen-Gateway. KeyZapper tut das für das Profil, bei dem *Claude in Office* eingeschaltet ist: im Profileditor oder über das
Dict `Office` eines verwalteten Profils (siehe [Verteilung über Microsoft Intune](#verteilung-über-microsoft-intune)). Ist es bei
mehreren Profilen eingeschaltet, haben die von der IT vorgegebenen Vorrang, danach gilt das erste eigene.

**Was passiert:** KeyZapper lädt Anthropics Manifest-Vorlage (`https://pivot.claude.ai/manifest.xml`, nur solange Claude in Office
eingeschaltet ist), setzt die Add-in-ID der Firma und hängt die Gateway- und Entra-Parameter an die beiden Seitenadressen des Add-ins: `gateway_url`,
`gateway_api_format=anthropic`, `entra_sso=1`, `graph_client_id`, `entra_scope`, `gateway_auth_source=entra` und, falls gesetzt,
`disabled_features`. Das Ergebnis wird per Sideload eingebunden:

* **macOS:** `<Add-in-ID>.manifest.xml` im Ordner `wef` jeder installierten App unter `/Applications` (Excel, Word, PowerPoint),
  also `~/Library/Containers/com.microsoft.<App>/Data/Documents/wef`.
* **Windows:** die Datei in `%LOCALAPPDATA%\KeyZapper\office`, eingetragen als Wert mit dem Namen der Add-in-ID unter
  `HKCU\SOFTWARE\Microsoft\Office\16.0\Wef\Developer`.

**Nur Entra, kein Geheimnis:** Das Add-in meldet sich per Microsoft-Entra-SSO an. Das Manifest enthält kein Geheimnis, und
KeyZapper liest oder schreibt nie den eigenen Speicher des Add-ins. App-Registrierung in Entra und Gateway-Route, die deren
Tokens annimmt, gehören zur Gateway-Seite; KeyZapper reicht nur die IDs durch.

**Felder:** Gateway-URL (https; der Host muss von `AllowedGatewayHosts` erlaubt sein), Entra-Client-ID, Entra-Scope
(`api://<Client-ID>/access_as_user`), Add-in-ID und optional abzuschaltende Funktionen des Add-ins (kommagetrennt). Die Add-in-ID
ist eine eigene GUID in Kleinbuchstaben; die ID des Marketplace-Add-ins wird abgelehnt, damit das Add-in per Sideload nie ihren
Platz einnimmt.

**Office einmal neu starten,** dann Claude über *Einfügen → Meine Add-ins* öffnen. Die Profilkarte zeigt, für welche Apps es
eingerichtet ist, oder warum es nicht geklappt hat. Unter macOS fragt das System beim ersten Schreiben, ob KeyZapper auf Daten
anderer Apps (die Office-Container) zugreifen darf; bitte erlauben. Wurde es abgelehnt, zeigt die Profilkarte den Fehler, und
eine Änderung am Profil oder *Status aktualisieren* versucht es erneut.

**Wann geschrieben wird:** nur bei einer Änderung: an den Office-Einstellungen des Profils, an Anthropics Vorlage oder an den
installierten Office-Apps. KeyZapper führt dazu eine eigene Aufzeichnung in `state.json` und liest die Office-Ordner nie, um das
herauszufinden. Ohne Verbindung gilt die zuletzt geladene Vorlage. Gibt es keine, oder sieht die Vorlage nicht nach Anthropics aus,
wird nichts geschrieben, und ein vorhandenes Manifest bleibt.

**Entfernen:** Ausschalten von *Claude in Office*, Löschen des Profils oder Deaktivieren von KeyZapper entfernt KeyZappers eigenes
Manifest (unter Windows auch den Registrywert) aus Office. Dateien anderer Add-ins bleiben.

**Beenden oder Deinstallieren von KeyZapper lässt das Manifest liegen,** denn nur KeyZapper entfernt es, und nur solange es läuft.
Vor dem Deinstallieren *Claude in Office* im Profil ausschalten oder KeyZapper deaktivieren; dann wird das Manifest (unter
Windows auch der Registrywert) vorher entfernt.

**Office puffert seitlich geladene Manifeste möglicherweise im Cache.** Zeigt sich eine Änderung (anderes Gateway, andere
Add-in-ID, ausgeschaltet) nach dem Neustart nicht, Office noch einmal neu starten oder, wenn das nicht hilft, den Add-in-Cache
von Office leeren. Das ist ein Hinweis: KeyZapper prüft diesen Cache weder noch fasst es ihn an.

**Sideload ist Microsofts Weg zum Entwickeln und Testen von Add-ins,** nicht für die Verteilung im Betrieb. Dafür unterstützt
Microsoft die zentrale Bereitstellung des Manifests über das Microsoft 365 Admin Center. Dort dieselbe Add-in-ID verwenden, damit
beide Wege dasselbe Add-in meinen.

## So funktioniert es

**Einrichtung (einmalig):** Der Key geht per stdin an den Helper und landet nur in der Key-Datei `keys.json`. Ins Projekt schreibt die App nur Verweise.

![Einrichtung: KeyZapper speichert den Key über keyzapper-helper und schreibt settings.local.json in Repo A und Repo B](docs/diagrams/keyzapper-flow-setup.de.svg)

**Laufzeit (jede Claude-Anfrage):** Claude Code holt den Key pro Projekt selbst über den Helper, auch wenn die App geschlossen ist.

![Laufzeit: Claude Code in Repo A und Repo B ruft keyzapper-helper mit der eigenen Profil-ID auf und sendet Key A bzw. Key B an LiteLLM](docs/diagrams/keyzapper-flow-runtime.de.svg)

Die Gesamtansicht mit Erläuterungen liegt unter [docs/diagrams/keyzapper-flow.de.html](docs/diagrams/keyzapper-flow.de.html).

Die App schreibt in `<Projekt>/.claude/settings.local.json`:

| Schlüssel | Wert |
|---|---|
| `apiKeyHelper` | `'/Applications/KeyZapper.app/Contents/MacOS/keyzapper-helper' credential --profile <UUID>` unter macOS, unter Windows der gequotete Pfad von `keyzapper-helper.exe` im Installationsordner (Key- und SSO-Profile); `'/mnt/c/…/keyzapper-helper.exe' credential --profile <UUID>` bei Projekten in WSL |
| `env.ANTHROPIC_BASE_URL` | LiteLLM-Endpunkt des Profils |
| `env.ANTHROPIC_DEFAULT_OPUS_MODEL`, `…_SONNET_MODEL`, `…_HAIKU_MODEL`, `env.ANTHROPIC_MODEL` | Modellnamen des Gateways aus dem Profil (falls gesetzt), z. B. `eu.anthropic.claude-sonnet-5-…` |
| weitere `env.*` | Zusätzliche Umgebungsvariablen des Profils, z. B. `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS=1` für Bedrock über LiteLLM |
| `env.ANTHROPIC_API_KEY`, `env.ANTHROPIC_AUTH_TOKEN` | `""`. Das neutralisiert geerbte Werte aus IDE oder Shell, die sonst gemischte Auth-Header erzeugen. |

Bestehende Einstellungen bleiben erhalten. Änderungen sind atomar und idempotent: Wiederholtes Einrichten ändert nichts.

Konflikte, die das Einrichten verhindern:
* Verwaltete Claude-Einstellungen, die `apiKeyHelper` oder `ANTHROPIC_*` setzen
* `CLAUDE_CODE_USE_BEDROCK`, `_VERTEX` oder `_FOUNDRY` in einer Einstellungsebene
* fremde Werte für dieselben Schlüssel in `settings.local.json`
* eine eingecheckte `settings.local.json`

**Helper-Schnittstelle:** `keyzapper-helper credential|store|status|delete --profile <UUID>`. Der Key fließt nur über stdin und stdout,
nie über Argumente oder Logs.

| Exit-Code | Bedeutung |
|---|---|
| 0 | ok |
| 64 | Aufruf |
| 65 | unbekanntes Profil |
| 66 | Key fehlt |
| 67 | Sitzung abgelaufen (nur SSO; neu anmelden) |
| 70 | intern |
| 77 | Key-Speicher (`keys.json`) nicht lesbar oder nicht beschreibbar |
| 78 | Metadaten defekt, Endpunkt nicht https (http nur für localhost) oder nicht in `AllowedGatewayHosts` |

Metadaten ohne Keys liegen in `state.json` im App-Datenordner, die Keys daneben in `keys.json`: `~/Library/Application Support/KeyZapper` unter macOS (nur für den Besitzer), `%LOCALAPPDATA%\KeyZapper` unter Windows (Benutzerprofil-Ordner). Keys aus der früheren Schlüsselbund-Version bietet die App unter macOS einmalig zur Übernahme an (macOS fragt dabei ggf. je Key um Erlaubnis).

## Projekte in WSL (Windows)

Ein Projektordner in einer WSL-Distribution (`\\wsl.localhost\<Distro>\home\…`, auch das ältere `\\wsl$\…`) wird wie jeder
andere Ordner zugeordnet: im Ordnerdialog unter „Linux“ auswählen oder auf das Fenster ziehen. Git-Root, Status und der
Git-Exclude-Eintrag kommen vom Git der Distribution (`wsl.exe -d <Distro> --exec git …`), nicht von Windows-Git, das auf
diesen Freigaben „dubious ownership“ meldet.

Claude Code in WSL ist ein Linux-Prozess und führt `apiKeyHelper` mit `/bin/sh` aus; in der `settings.local.json` eines
solchen Projekts steht deshalb der Windows-Helper über seinen Mount-Pfad, für sh gequotet:

```json
"apiKeyHelper": "'/mnt/c/Users/<Benutzer>/AppData/Local/Programs/KeyZapper/keyzapper-helper.exe' credential --profile <UUID>"
```

Der Helper läuft per WSL-Interop als Windows-Prozess und liest `keys.json` wie gewohnt aus `%LOCALAPPDATA%\KeyZapper`;
der Key gelangt nie ins Dateisystem der Distribution. Voraussetzungen: WSL-Interop und der Laufwerks-Automount müssen
aktiv sein (Standard; `[interop] enabled=true` und `[automount] enabled=true` mit Standard-Root `/mnt` in `/etc/wsl.conf`),
und in der Distribution muss git installiert sein, damit die Git-Root erkannt wird (sonst wird der gewählte Ordner selbst
zugeordnet).

## Verteilung über Microsoft Intune

Jeder Git-Tag `X.Y.Z` baut per CI die Pakete `KeyZapper-X.Y.Z.pkg` (macOS) und `KeyZapper-X.Y.Z.msi` (Windows) und hängt beide an das GitHub-Release (siehe [Release](#release)).

**macOS: App anlegen:** *Apps › Alle Apps › Erstellen › macOS-App (PKG)*. Das ist der nicht verwaltete PKG-Typ, der auch
unsignierte Pakete annimmt; Voraussetzung ist der Intune Management Agent ≥ 2308.006.
* Mindest-OS: macOS 14
* Erkennungsregeln › Enthaltene Apps: `io.github.bl0rb.keyzapper` mit der Paketversion, „App-Version ignorieren“ = Nein

**Windows: App anlegen:** *Apps › Alle Apps › Erstellen › Branchen-App (Line-of-business)*, die `.msi` hochladen (Installation pro Computer nach Program Files).

**Signatur:** Die Keys liegen nicht im Schlüsselbund, die Signatur beeinflusst den Key-Zugriff daher nicht. Für macOS signieren
`SIGN_IDENTITY` und `INSTALLER_IDENTITY` (`desktop/scripts/build-pkg.sh`) mit einer Developer ID; ohne sie ist der Build ad-hoc-signiert.

**Deinstallation:** Intune kennt für PKG-Apps keine Deinstallationszuweisung. Vorher in der App „Zuordnung entfernen“
wählen, dann `/Applications/KeyZapper.app` löschen. Unter Windows die MSI über Intune oder *Apps & Features* deinstallieren.

### Konfiguration

**macOS:** *Geräte › Konfiguration › Erstellen › macOS › Vorlagen › Einstellungsdatei (Preference file)*, Domäne `io.github.bl0rb.keyzapper`.
Vorlage mit allen Schlüsseln: [docs/intune/io.github.bl0rb.keyzapper.plist](docs/intune/io.github.bl0rb.keyzapper.plist). Sie enthält nur
Schlüssel-Wert-Paare ohne `<plist>`/`<dict>`-Rahmen, wie Intune es verlangt.

**Windows:** Dieselben Schlüssel werden als Registrywerte unter `HKLM\SOFTWARE\Policies\KeyZapper` (Computer-Richtlinie hat Vorrang) bzw. `HKCU\SOFTWARE\Policies\KeyZapper` gelesen:
Zeichenfolgen als `REG_SZ`, Listen als `REG_MULTI_SZ`, Schalter als `REG_DWORD` 0/1, `ManagedProfiles` als JSON-Array in einem `REG_SZ`.
Vorlage mit allen Schlüsseln: [docs/intune/keyzapper-windows.reg](docs/intune/keyzapper-windows.reg) (z. B. per PowerShell-Skript oder Einstellungskatalog verteilen).

**Wichtig:** Für Benutzer, die SSO-Profile per verwalteten Einstellungen erhalten, nicht `apiKeyHelper`, `env.ANTHROPIC_BASE_URL`,
`ANTHROPIC_API_KEY` oder `ANTHROPIC_AUTH_TOKEN` in `managed-settings.json` (oder `managed-settings.d/*.json`) setzen.
Verwaltete Einstellungen haben Vorrang vor Projekteinstellungen; KeyZapper bricht das Binding bei Konflikt ab.

| Schlüssel | Typ | Wirkung |
|---|---|---|
| `ManagedProfiles` | Array von Dicts: `Name`, `Endpoint`, optional `OpusModel`, `SonnetModel`, `HaikuModel`, `ModelAlias`, `Environment` (Dict), `ID`, `Type` (`apiKey` Standard oder `oidc`), `OIDCIssuer`, `OIDCClientID`, `OIDCScope`, `OIDCTokenType`, `Office`, `ClaudeDesktop` (Dicts, siehe unten) | Profile werden automatisch angelegt und sind nicht editier- oder löschbar; Entwickler tragen nur den Key ein (oder melden sich für SSO an) und können *Claude in Office* und *Claude Desktop (SSO)* für das Profil selbst einschalten, wo die IT beides nicht setzt (siehe unten). Ohne `ID` wird die Profil-ID aus `Name` abgeleitet, eine Umbenennung erzeugt also ein neues Profil. Bei SSO (`oidc`) sind `OIDCIssuer` und `OIDCClientID` erforderlich; `OIDCScope` Standard `openid profile offline_access`; `OIDCTokenType` ist `access` (Standard) oder `id`, andere Werte verwerfen das Profil. |
| `ClaudeDesktop` (innerhalb eines `ManagedProfiles`-Eintrags) | Dict: `OIDCIssuer`, `OIDCClientID`, optional `GatewayURL` (Standard: `Endpoint` des Profils), `Scopes`, `BearerTokenType` (`id_token` Standard oder `access_token`), `CredentialKind` (`external-idp` Standard oder `interactive`), `Config` (Dict weiterer Claude-Desktop-Schlüssel), `AutoApply` (Bool, Standard `false`) | Claude Desktop meldet sich selbst mit dem OIDC-Client des Gateways an, wenn dieses Profil dafür gewählt ist (siehe [Claude-Tools](#claude-tools)). `external-idp` braucht Claude Desktop ab 2.7032.0; solange ältere Versionen im Einsatz sind, `interactive` setzen. Ein Block mit unbrauchbaren Werten (auch ein unbekanntes `CredentialKind`) bleibt erhalten; KeyZapper schreibt dann nichts für Claude Desktop. Die Schlüssel entsprechen der Claude-Desktop-Konfiguration: `GatewayURL` → `inferenceGatewayBaseUrl`, `OIDCIssuer`/`OIDCClientID`/`Scopes`/`BearerTokenType` → `issuer`, `clientId`, `scopes` und `bearerTokenType` des OIDC-Blocks, `CredentialKind` → `inferenceCredentialKind`; `inferenceProvider: gateway` und `appendOfflineAccess: true` werden immer geschrieben, alles Weitere (Telemetrie, Banner, erlaubte Hosts, …) gehört in `Config`. Ohne das Dict können Entwickler *Claude Desktop (SSO)* im Profil selbst ausfüllen; das Dict der IT ersetzt ihren Eintrag. |
| `Office` (innerhalb eines `ManagedProfiles`-Eintrags) | Dict: `Enabled` (Bool, Standard `true`), `AddinID`, `GatewayURL`, `EntraClientID`, `EntraScope`, `DisabledFeatures` (Array oder kommagetrennter String) | Richtet [Claude in Microsoft 365](#claude-in-microsoft-365-über-das-profil) aus diesem Profil ein. Werte werden getrimmt. Ein Block mit unbrauchbaren Werten bleibt erhalten und wird in der Profilkarte gemeldet; dann wird nichts geschrieben. Ohne das Dict können Entwickler *Claude in Office* im Profil selbst einschalten; das Dict der IT ersetzt ihren Eintrag. |
| `AllowedGatewayHosts` | Array von Strings (`host` oder `*.domain`) | Profile nur für diese Hosts. Der Helper gibt Keys für andere Hosts nicht heraus (Exit 78). Gilt nur für den Gateway-Host; der IdP-Host muss nicht gelistet sein. Leer bedeutet keine Einschränkung. |
| `DefaultEndpoint`, `DefaultModelAlias` | String | Vorbelegung beim Anlegen eigener Profile |
| `MinimumClaudeCodeVersion` | String | Schwelle für die Warnung vor veralteten Claude-CLIs (Standard 2.1.288) |
| `OneDriveBackup` | Bool | Backup von Profilen und Zuordnungen nach OneDrive (siehe unten) |
| `BackupDirectory` | String, `~` erlaubt | Expliziter Backup-Ordner; aktiviert das Backup auch ohne `OneDriveBackup` |
| `UpdateCheckEnabled` | Bool (Standard `true`) | In-App-Update-Prüfung. Bei Verteilung über Intune auf `false` setzen, sonst kann Intune eine selbst installierte neuere Version wieder überschreiben. |
| `AllowKeyExport` | Bool (Standard `true`) | Ob Keys die App verlassen dürfen. `false`: kein Kopieren in die Zwischenablage; Backups enthalten nur Profile und Zuordnungen. |
| `ManagedMcpServersURL` | String (https-URL) | Adresse der zentralen MCP-Liste (siehe [Zentrale MCP-Liste](#zentrale-mcp-liste)). |
| `ClaudeToolsEnabled` | Bool (Standard `true`) | Zeigt die Seite *Claude-Tools* zum Installieren von Claude Code, den IDE-Plugins und Claude Desktop. |
| `ClaudeDesktopAutoInstall` | Bool (Standard `false`) | Installiert ein fehlendes Claude Desktop im Hintergrund aus den offiziellen Quellen von Anthropic, auch wenn die Seite *Claude-Tools* ausgeblendet ist (siehe [Claude-Tools](#claude-tools)). |
| `ProxyURL` | String (`http://host:port`, optional `user:passwort@`) | Proxy für den eigenen Internetverkehr von KeyZapper: Update-Prüfung und Installer-Downloads von `github.com`, die Claude-Installer von `claude.ai`. Ohne den Schlüssel nutzt KeyZapper die Proxy-Umgebungsvariablen (`HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, `NO_PROXY` wird beachtet), sonst den System-Proxy (Netzwerkeinstellungen von macOS, Internetoptionen von Windows; eine PAC-Datei wird nicht ausgewertet). Aufrufe an Gateway und Identity Provider gehen nicht darüber. |
| `IgnoreDownloadCertificateErrors` | Bool (Standard `false`) | Update-Prüfung und Downloads akzeptieren Zertifikate, denen das System nicht vertraut, für einen Proxy mit TLS-Inspektion, dessen Stammzertifikat nicht auf dem Gerät installiert ist (Fehler „The certificate was not trusted“). Betrifft nur den eigenen Internetverkehr von KeyZapper; die Installer bleiben durch SHA-256-Prüfsumme und minisign-Signatur (KeyZapper) bzw. die Code-Signatur von Anthropic (Claude Desktop) geschützt. Besser das Stammzertifikat des Proxys auf dem Gerät installieren. |

Lokal testen (Benutzerebene; per Intune verwaltete Werte haben Vorrang):

```bash
defaults write io.github.bl0rb.keyzapper AllowedGatewayHosts -array litellm.firma.example
```

### OneDrive-Backup

Die App sichert Profile und Projektzuordnungen nach `~/Library/CloudStorage/OneDrive-<Firma>/KeyZapper/keyzapper-backup.json` (macOS) bzw. in den Ordner `KeyZapper` im OneDrive-Ordner (Windows).
Ein Geschäftskonto hat Vorrang vor „OneDrive-Personal“. **Keys werden nie gesichert.** Auf einem neuen Rechner bietet die App
an, das Backup wiederherzustellen oder zu verwerfen. Zuordnungen werden nur für vorhandene Ordner übernommen, die Keys
trägt man neu ein. Solange ein gefundenes Backup nicht wiederhergestellt oder verworfen wurde, wird es nicht überschrieben.

### Zentrale MCP-Liste

Die IT pflegt die MCP-Server in einer JSON-Datei auf ihrem eigenen Server und verteilt per Intune nur deren Adresse
(`ManagedMcpServersURL`). Die Datei hat das `.mcp.json`-Format von Claude Code; Vorlage:
[docs/intune/mcp-servers.example.json](docs/intune/mcp-servers.example.json).

```json
{
  "mcpServers": {
    "weave": { "type": "http", "url": "https://weave.firma.example/mcp",
               "oauth": { "clientId": "c1abe…", "callbackPort": 8976, "scopes": "api://weave/mcp.read" } },
    "wiki":  { "type": "http", "url": "https://wiki.firma.example/mcp", "headers": { "X-Tenant": "firma" } },
    "fetch": { "type": "stdio", "command": "uvx", "args": ["mcp-server-fetch"] }
  }
}
```

* KeyZapper lädt die Liste beim Start, alle 10 Minuten, nach einer SSO-Anmeldung und per *Neu laden* auf der Seite *MCP-Server*.
  Der Server beantwortet `GET` mit `200` und dem JSON (höchstens 1 MB); `401`/`403` erscheint als „Anmeldung erforderlich“.
* **Anmeldung:** Ist ein SSO-Profil angemeldet, dessen Gateway oder Identity Provider denselben Host wie die Liste hat, schickt
  KeyZapper dessen Token als `Authorization: Bearer …` mit (Token-Art des Profils). So kann der Server je Nutzer oder Gruppe
  antworten. Das Token geht nur an Hosts, die es ohnehin bekommen.
* Die Server erscheinen mit dem Hinweis *Von IT verwaltet*: Entwickler schalten sie je Projekt ein, können sie aber nicht
  bearbeiten oder löschen. Änderungen an der Liste werden in die nutzenden Projekte geschrieben; Server, die die IT aus der
  Liste nimmt, verschwinden aus den Projekten.
* Die zuletzt geladene Liste bleibt in `mcp-catalog.json` im App-Datenordner, damit die Server auch offline funktionieren;
  ein fehlgeschlagener Abruf wird auf der Seite angezeigt, die bisherige Liste gilt weiter.
* Nur `https`-Adressen (einfaches http nur für localhost), auch für Remote-Server in der Liste. Einträge mit ungültigem
  Namen, ohne Befehl oder URL werden ausgelassen und auf der Seite genannt; ebenso ein Eintrag mit dem Namen eines eigenen Servers.
* Secrets gehören nicht in die Datei; `${NAME}`-Verweise (Claude Code setzt sie aus der Umgebung ein) oder OAuth nutzen.

## Prüfung der Claude-Einstellungen und Standardprofil

Die Seite *Claude-Einstellungen* in der Seitenleiste prüft `~/.claude/settings.json` auf Probleme, die Claude
Code stillschweigend hinnimmt: ungültiges JSON (geprüft mit demselben strengen `JSON.parse` wie Claude Code, z. B. Kommas
nach dem letzten Eintrag), Klartext-Keys, einen Endpunkt ohne Key, Nicht-Text-Werte in `env`, Provider-Schalter an
LiteLLM vorbei, Modell-Variablen und feste Modell-IDs statt Alias. Probleme zeigt jederzeit ein Banner an.

* **Reparieren / Erzeugen** macht die Datei gültig, entfernt Klartext-Keys und wandelt `env`-Werte in Text um. Eine
  fehlende Datei wird angelegt. Vor jeder Änderung sichert KeyZapper sie als `settings.json.keyzapper-<Zeit>.bak`.
* **Standardprofil** trägt ein Profil in `~/.claude/settings.json` ein. Claude Code nutzt es in allen Ordnern ohne eigene
  Zuordnung statt eines Klartext-Keys oder der normalen Anmeldung. Zugeordnete Projekte behalten ihr eigenes Profil.

## Claude-Tools

Die Seite *Claude-Tools* (ausgeblendet mit `ClaudeToolsEnabled = false`) zeigt, was installiert ist, und installiert ohne Administratorrechte:

| Tool | Quelle | Installiert nach |
|---|---|---|
| Claude Code (CLI) | Nativer Installer von Anthropic (`curl -fsSL https://claude.ai/install.sh \| bash`, Windows `irm https://claude.ai/install.ps1 \| iex`) | `~/.local/bin/claude`, aktualisiert sich selbst |
| Claude Code für VS Code | `code --install-extension anthropic.claude-code` (VS Code Marketplace) | VS Code des Nutzers |
| Claude Code für IntelliJ IDEA | `idea installPlugins com.anthropic.code.plugin` (JetBrains Marketplace); IntelliJ muss geschlossen sein | jedes gefundene IntelliJ IDEA (auch aus der Toolbox) |
| Claude Desktop | `.dmg` von `claude.ai/api/desktop/darwin/universal/dmg/latest/redirect` (macOS; prüft vor dem Kopieren nach `/Applications` bzw. `~/Applications` die Developer ID `Q6L2SF6YDW` von Anthropic), `.msix` über `Add-AppxPackage` (Windows) | Programme-Ordner / aktueller Nutzer |

**Anmeldung:** Claude Code im Terminal, in VS Code und in IntelliJ nutzt in zugeordneten Projekten deren Profil, sonst das
Standardprofil (Key, SSO oder Claude-Abo). Für Key- und SSO-Profile in VS Code *Disable Login Prompt*
(`claudeCode.disableLoginPrompt`) einschalten; die Seite öffnet diese Einstellung. Findet IntelliJ `claude` nicht, den auf der
Seite angezeigten Pfad unter *Settings › Tools › Claude Code [Beta] › Claude command* eintragen.

**Claude Desktop mit Profil:** Key- oder SSO-Profil wählen und *Übernehmen*. KeyZapper legt eine Konfigurationsdatei für den
Drittanbieter-Modus von Claude Desktop in dessen Konfigurationsbibliothek an (`~/Library/Application Support/Claude-3p/configLibrary`,
`%LOCALAPPDATA%\Claude-3p\configLibrary`) und wählt sie in `_meta.json` aus:

```json
{
  "inferenceProvider": "gateway",
  "inferenceGatewayBaseUrl": "https://litellm.firma.example",
  "inferenceCredentialKind": "helper-script",
  "inferenceCredentialHelper": "/Applications/KeyZapper.app/Contents/MacOS/keyzapper-helper",
  "inferenceCredentialHelperArgs": ["credential", "--profile", "<UUID>"],
  "inferenceCredentialHelperTtlSec": 300,
  "inferenceModels": [{ "name": "eu.anthropic.claude-sonnet-5-team", "anthropicFamilyTier": "sonnet" }]
}
```

Claude Desktop holt Key oder SSO-Token so über `keyzapper-helper`, genau wie Claude Code; in der Datei steht kein Key. Die
Helper-TTL folgt `CLAUDE_CODE_API_KEY_HELPER_TTL_MS` des Profils (Standard 300 s). Danach Claude Desktop beenden, neu öffnen
und auf dem Anmeldebildschirm den Start mit dieser Konfiguration wählen. *Normale Anmeldung (claude.ai)* entfernt den Eintrag
wieder und aktiviert die zuvor aktive Konfiguration, sofern es sie noch gibt. Ändert sich das Profil, aktualisiert KeyZapper
die Datei. Eine Claude-Desktop-Konfiguration, die die IT per MDM-Profil (`com.anthropic.claudefordesktop`) oder Registry
(`HKLM\SOFTWARE\Policies\Claude`) verteilt, hat Vorrang, wenn sie mehr als die Update-, Neustart- und Proxy-Schlüssel der App
setzt; KeyZapper schreibt dann nichts, die Seite weist darauf hin. Beim Deaktivieren entfernt KeyZapper den Eintrag, beim
Aktivieren schreibt es ihn wieder.

**Claude Desktop von der IT eingerichtet:** Im `ClaudeDesktop`-Dict eines verwalteten Profils fügt `Config` weitere Schlüssel
der [Konfigurationsreferenz](https://claude.com/docs/third-party/claude-desktop/configuration) von Claude Desktop in den
Eintrag ein (native Typen, z. B. `disableDeploymentModeChooser`, `coworkEgressAllowedHosts`). Die Anmeldeschlüssel, die
KeyZapper ableitet, haben Vorrang; Schlüssel mit Geheimnissen, Headern, lokalen Programmen oder Zugangsdateien und
Bootstrap-Schlüssel werden abgelehnt, dann wird nichts geschrieben. Mit `AutoApply` schreibt und aktiviert KeyZapper den
Eintrag ohne Klick und schreibt ihn beim Start und beim Aktualisieren wieder, falls er fehlt oder geändert wurde (nur ein
verwaltetes Profil darf das setzen). Dazwischen kann der Nutzer ihn ändern, erzwungen ist darin also nichts; was erzwungen
werden muss, bleibt im MDM. Mit `ClaudeDesktopAutoInstall` installiert KeyZapper ein fehlendes Claude Desktop
im Hintergrund, nachdem die Konfiguration geschrieben ist, unabhängig von `ClaudeToolsEnabled`; eine fehlgeschlagene
Installation wird bei *Status aktualisieren* wiederholt.

**Claude Desktop mit eigener SSO-Anmeldung:** Hat das Gateway einen eigenen OIDC-Client für Claude Desktop (andere Client-ID
oder Gateway-Route als Claude Code), im Profil *Claude Desktop (SSO)* einschalten und Issuer, Client-ID sowie optional
Gateway-URL, Scopes und Token-Art angeben (`id_token` Standard, `access_token` braucht die Scopes des Gateways). KeyZapper
schreibt dann statt des Helpers die Anmeldung über einen externen Identity Provider (`inferenceCredentialKind: external-idp`,
`inferenceIdpOidc`; Claude Desktop ab 2.7032.0): Claude Desktop meldet sich im Browser an, verwahrt sein Token selbst und listet
die Modelle des Gateways (`/v1/models`). Es wird weder Key noch Token geschrieben. Für ältere Claude-Desktop-Versionen im Feld
*Claude-Desktop-Version* des Profils *Älter als 2.7032.0* wählen: KeyZapper schreibt dann die ältere Schreibweise (`interactive`,
`inferenceGatewayOidc`), die neuere Versionen als dieselbe Anmeldung lesen. Die IT setzt das mit einem `ClaudeDesktop`-Dict in
`ManagedProfiles` (`CredentialKind`: `external-idp` oder `interactive`).

**Auf einem verwalteten Profil ausprobieren:** Ein Profil aus Intune kann *Claude in Office* und *Claude Desktop (SSO)* trotzdem
vom Entwickler bekommen, solange der Eintrag der IT kein `Office`- bzw. `ClaudeDesktop`-Dict hat: *Bearbeiten* öffnet das Profil
mit nur diesen beiden Abschnitten freigeschaltet, und die Einstellungen überstehen den nächsten Abgleich mit Intune und einen
Neustart. Setzt die IT das Dict später, ersetzt es die Einstellungen des Entwicklers und sperrt den Abschnitt; ein wieder
entferntes Dict bleibt als eigene Einstellung des Entwicklers stehen, bis er sie ausschaltet.

## KeyZapper deaktivieren

Der Schalter unten in der Seitenleiste deaktiviert KeyZapper: Die App entfernt ihre Einträge aus allen Projekten und aus
`~/.claude/settings.json`, Claude Code nutzt dann überall die normale Anmeldung. Die Zuordnungen bleiben gespeichert und
werden beim Aktivieren wieder geschrieben. Claude in Microsoft 365 wird aus Office entfernt, solange KeyZapper deaktiviert ist, und beim Aktivieren wieder eingerichtet.

## Verschlüsseltes Backup und Wiederherstellen

*Backup exportieren …* auf der Seite *Backup* schreibt Profile, Projektzuordnungen und Keys in eine
`.kzbackup`-Datei, verschlüsselt mit einem Passwort von mindestens 12 Zeichen. *Backup importieren …* stellt sie auf
demselben oder einem anderen Rechner wieder her: Profile und Keys mit gleicher ID werden überschrieben, Projekte werden
zugeordnet, sofern ihr Ordner existiert. Das Passwort wird nirgends gespeichert; ohne Passwort ist das Backup nicht
wiederherstellbar.

Verschlüsselung: AES-256-GCM mit einem Schlüssel aus PBKDF2-HMAC-SHA256 (600.000 Runden, zufälliges Salt). Die
Dateiparameter sind authentifiziert, ein falsches Passwort oder eine veränderte Datei wird erkannt. Die Datei wird mit
den Rechten `0600` gespeichert. Mit `AllowKeyExport = false` enthält das Backup keine Keys. Anders als das automatische
OneDrive-Backup (nie Keys) entsteht dieses Backup manuell und bei Bedarf.

## Updates und Version

Beim Start prüft die App das neueste [GitHub-Release](https://github.com/bl0rb/ClaudeKeyZapper/releases); manuell geht es über
*KeyZapper › Nach Updates suchen …* oder die Versionszeile unten in der App. Ist eine neuere Version verfügbar, lädt
„Installieren“ den Installer (`.pkg` unter macOS, `.msi` unter Windows) herunter, prüft die von GitHub veröffentlichte SHA-256-Prüfsumme und die minisign-Signatur des Releases (`<Installer>.minisig`) gegen den
in die App eingebauten öffentlichen Schlüssel ([desktop/update-key.pub](desktop/update-key.pub)) und öffnet dann den System-Installer.
Ohne Prüfsumme oder gültige Signatur wird das Update abgelehnt; der signierte Dateiname muss passen, damit ein älteres signiertes Paket
nicht als neueres durchgeht.
Dafür sind Administratorrechte nötig. Pakete werden nur von `github.com` geladen. Die installierte Version steht unten in der
App und unter *KeyZapper › Über KeyZapper*.

Unter Intune schaltet `UpdateCheckEnabled = false` die Prüfung ab. Updates kommen dann über ein neues Paket in Intune.

Hinter einem Firmen-Proxy laufen Prüfung und Download über den Proxy aus `ProxyURL` (Intune), sonst über die Proxy-Umgebungsvariablen,
sonst über den System-Proxy. Eine fehlgeschlagene Prüfung nennt den Proxy, über den sie ging. Inspiziert der Proxy TLS mit einem
Zertifikat, dem das Gerät nicht vertraut, dessen Stammzertifikat auf dem Gerät installieren oder `IgnoreDownloadCertificateErrors`
für Prüfung und Downloads setzen.

## Bekannte Einschränkungen

* Ein Profil pro Git-Repository; Unterordner und Worktrees lassen sich nicht abweichend zuordnen.
* Startet Claude außerhalb des zugeordneten Bereichs (z. B. in einem Unterordner eines Nicht-Git-Ordners), nutzt es die
  Standard-Anmeldung des Entwicklers.
* Die IntelliJ-Integration ist noch nicht im Pilot geprüft.

## Entwicklung

```
desktop/core        Rust-Bibliothek keyzapper-core: Datenmodell, Metadaten, Key-Speicher, Helper-Logik, Claude-Einstellungen, Intune-Konfiguration, Backup
desktop/src-tauri   Tauri-App (macOS, Windows) und keyzapper-helper (apiKeyHelper für Claude Code)
desktop/ui          Web-Oberfläche (HTML, JS, CSS)
desktop/locales     Englische Übersetzungen (deutsche Quelltexte)
desktop/scripts     build-pkg.sh (macOS), build-msi.sh (Windows), l10n_check.py
```

Tests:

```bash
cd desktop && cargo test -p keyzapper-core
```

App im Entwicklungsmodus starten (benötigt Rust und Node.js):

```bash
cd desktop && npm ci && npm run dev
```

Installer lokal bauen (`desktop/dist/KeyZapper-<version>.pkg` unter macOS, `.msi` unter Windows in Git Bash):

```bash
cd desktop && VERSION=1.0.0 scripts/build-pkg.sh
```

```bash
cd desktop && VERSION=1.0.0 scripts/build-msi.sh
```

## Release

Ein Tag im Format `X.Y.Z` startet [.github/workflows/release.yml](.github/workflows/release.yml). Der Workflow führt die
Tests aus, baut `KeyZapper-X.Y.Z.pkg` (macOS) und `KeyZapper-X.Y.Z.msi` (Windows), signiert beide mit minisign und veröffentlicht sie
mit ihren `.minisig`-Dateien in einem GitHub-Release. Zum Signieren braucht es das Repository-Secret `MINISIGN_SECRET_KEY` (der geheime
Schlüssel zu `desktop/update-key.pub`); ohne Secret bricht der Workflow ab, statt unsignierte Installer zu veröffentlichen.
Ein Tag wie `X.Y.Z-beta.N` erzeugt ein Pre-Release, das nur mit dem Schalter *Beta-Versionen* in der Seitenleiste angeboten
wird (Standard: an, solange eine Beta installiert ist).

```bash
git tag 1.0.0
```

```bash
git push origin 1.0.0
```

## Lizenz

[MIT](LICENSE) © 2026 bl0rb
