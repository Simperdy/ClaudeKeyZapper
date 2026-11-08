**English** · [Deutsch](README.de.md)

<img src="Resources/AppIcon.svg" width="96" alt="KeyZapper icon">

# KeyZapper

[![Build](https://github.com/bl0rb/ClaudeKeyZapper/actions/workflows/desktop.yml/badge.svg)](https://github.com/bl0rb/ClaudeKeyZapper/actions/workflows/desktop.yml)
[![Release](https://github.com/bl0rb/ClaudeKeyZapper/actions/workflows/release.yml/badge.svg)](https://github.com/bl0rb/ClaudeKeyZapper/actions/workflows/release.yml)
[![Security audit](https://github.com/bl0rb/ClaudeKeyZapper/actions/workflows/security.yml/badge.svg)](https://github.com/bl0rb/ClaudeKeyZapper/actions/workflows/security.yml)
[![Latest release](https://img.shields.io/github/v/release/bl0rb/ClaudeKeyZapper?display_name=tag&label=release)](https://github.com/bl0rb/ClaudeKeyZapper/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
[![Platforms](https://img.shields.io/badge/platforms-macOS%2014%2B%20%7C%20Windows%2010%2B-lightgrey)](#requirements)
[![Signed updates](https://img.shields.io/badge/updates-minisign%20signed-success)](docs/security-audit.md#updates-and-installers)
[![Key storage](https://img.shields.io/badge/keys-local%20file%20%C2%B7%20owner--only%20%C2%B7%20https%20only-success)](docs/security-audit.md#key-storage)
[![Security review](https://img.shields.io/badge/security%20review-2026--11--03-informational)](docs/security-audit.md)

Desktop app for macOS and Windows that manages approved, project-specific LiteLLM keys in a local key file and assigns them to local project folders.
Claude Code (VS Code, IntelliJ, CLI) then automatically uses the matching key in every project. Several projects open at
the same time work independently of each other.

**Assign once, never switch keys again.** Claude Code fetches the matching key itself via `keyzapper-helper`,
even when KeyZapper is closed. The app explains this under “How it works”.

![KeyZapper overview with two key profiles, one SSO profile and assigned projects](docs/screenshots/keyzapper-overview.png)

<sub>Screenshot with made-up sample data.</sub>

```
Claude Code ──apiKeyHelper──▶ keyzapper-helper ──▶ local key file
     │
     └──── requests with project key ────▶ LiteLLM ──▶ Amazon Bedrock
```

## Why KeyZapper

LLM costs have to be tracked per project and kept within project budgets. For this, a dedicated middleware in front of
LiteLLM issues API keys per project. Each key carries its project's budget and allowed models, so every request made
with it is booked to that project. KeyZapper makes sure Claude Code always uses the key of the project you are working
in, so the costs land on the right project without anyone switching keys by hand.

## Features

* Profiles with name, LiteLLM endpoint (https; plain http only for a gateway on localhost), optional model alias and key. The key is stored exclusively in the local key file `keys.json` in the app data folder (no Keychain or credential-manager prompts).
* Assign project folders to a profile. The app only adds to `.claude/settings.local.json` and excludes the file from Git via `.git/info/exclude`.
* Status per project: active, differing, folder missing, key missing, conflicts with other settings. A summary shows what needs attention; *Repair all* writes the settings of every affected project again.
* Drag project folders from Finder or Explorer onto a profile to assign them.
* MCP servers (stdio, HTTP or SSE) defined once – or from templates such as GitHub, Notion or Sentry – and switched on per project. The app writes them into the repository's `.mcp.json` (excluded from Git) and approves them in `.claude/settings.local.json`, so Claude Code loads them there automatically. Remote servers with OAuth (e.g. Weave-MCP via Entra ID or Keycloak) take a client ID, callback port and scopes; Claude Code handles the browser sign-in.
* Central MCP server list: IT publishes the MCP servers as one JSON file on its own server (e.g. the SSO server); KeyZapper loads it and offers the servers read-only for every project (see [Central MCP server list](#central-mcp-server-list)).
* *Claude Tools* page: installs Claude Code (CLI), its VS Code extension and IntelliJ plugin and Claude Desktop from Anthropic's official sources and connects Claude Desktop to a key or SSO profile (see [Claude tools](#claude-tools)).
* Test connection against LiteLLM (invalid or blocked key, rate limit, budget, gateway unreachable).
* Remaining budget per key, as reported by LiteLLM (`/key/info`): a bar that turns orange from 70 % and red from 90 % used, amount left of the limit and next reset, or the spend for keys without a limit.
* Claude subscription profiles: projects use your Claude Pro, Max, Team or Enterprise plan through Claude Code's own login instead of a gateway key (see below).
* Claude in Microsoft 365: a profile can set up the Claude add-in for Excel, Word and PowerPoint behind the company gateway with Entra SSO (see below).
* Menu bar (macOS) or notification area (Windows) icon: a click shows usage at a glance – your Claude subscription's limits (session, week, per model) and every key's budget; a right-click switches a project's profile without opening the window. Closing the window keeps KeyZapper running there.
* Removing an assignment only removes the entries the app set itself.
* Sidebar with profiles, projects, MCP servers, Claude settings and backup. Everything can be changed directly – via the ⋯ menu or a right-click; keys can be copied and replaced.
* Update check against the GitHub releases with one-click installation; the version is shown in the app and under “About KeyZapper”.
* UI in English and German (follows the system language).
* Encrypted backup and restore of profiles, assignments and keys (`.kzbackup`, password-protected).
* Manageable via Intune: predefined profiles (including Claude in Microsoft 365), gateway allowlist, defaults, minimum CLI version, OneDrive backup, update check, central MCP server list, Claude tools page.

## Requirements

* macOS 14 or newer or Windows 10 or newer (with the WebView2 runtime)
* Claude Code ≥ 2.1.288 (VS Code extension or the `claude` CLI for IntelliJ). Older versions only read the project assignment
  at startup directly in the project folder; the app warns in that case.

## Usage

1. **Create profile:** name, LiteLLM endpoint, key and optionally the gateway model names for Opus, Sonnet and Haiku. *Load models from gateway* lists the models the key may use (LiteLLM `/v1/models`) and fills the tiers. Profiles managed by IT are already there; just enter the key.
2. **Assign project:** choose a folder and select a profile. For Git repositories the assignment applies to the whole repository
   including subfolders and worktrees. Claude Code reads the project settings only at the root of the main checkout.
3. **Restart the Claude session in the project**; in VS Code, start a new conversation or reload the window.

The overview shows every profile with endpoint, model and the assigned key (masked, e.g. `••••7f3a`) along with all
assigned projects. For each project you can change the profile, reapply the settings and remove the assignment.

**Copy key:** The key is placed on the clipboard marked as confidential so that clipboard managers do not store it.
After 60 seconds it is removed again, unless something else has been copied in the meantime.

**Changing a key:** “Change” replaces the key; this takes effect immediately for new sessions. Running sessions pick up the key after the
helper cache expires (default 5 min), after an HTTP 401, or after a restart.

**If a key is missing** or blocked, requests fail. Claude Code never falls back to other credentials.

## SSO profiles

KeyZapper also has an **experimental, exact-build Hermes integration**; macOS/workplace acceptance is still pending. See the [Hermes setup, recovery and acceptance guide](docs/hermes-integration.md) before using it.

Profiles can authenticate via a static LiteLLM key (Key type) or via OIDC login (SSO type). SSO requires
an OIDC-verifying proxy in front of LiteLLM that validates the token (access token by default) and injects the virtual key into requests.

**SSO fields:** Issuer (https URL), Client ID (public client, no secret), Scope (default `openid profile offline_access`),
Token type (*Access token* default, or *ID token* for gateways that expect the OIDC ID token as bearer token).
For Microsoft Entra ID use scope `api://<app-id>/.default offline_access` with access tokens; a refresh token is required.
With ID tokens the scope must include `openid` (Entra ID: also `profile`, otherwise the `oid` claim is missing) and `offline_access`;
the token's expiry comes from its `exp` claim.

**Login:** *Sign In* opens the system browser for Authorization Code flow with PKCE. The redirect URI is a loopback address
`http://127.0.0.1:<random port>/callback`. To register the redirect URI:

* **Microsoft Entra ID:** App registration › Mobile and desktop applications › Redirect URI: `http://127.0.0.1`
  (Entra ignores the port for loopback redirects).
* **Keycloak:** Public client › Standard flow › PKCE S256 › Valid redirect URI: `http://127.0.0.1/*`.

**Tokens:** Refresh and access tokens are stored locally in `sso-tokens.json` in the app data folder (owner-only, `0600`),
never in backups or via "copy key". The helper returns a cached access token or refreshes it once, serialized safely across parallel
sessions. Exit code 67 means the session expired; sign in again in KeyZapper.

**Settings:** Set `CLAUDE_CODE_API_KEY_HELPER_TTL_MS` in the profile's Environment field to a value below the token lifetime
(e.g., 900000 for 60–90 min tokens). For long streams over a gateway, also set `API_TIMEOUT_MS`,
`CLAUDE_STREAM_IDLE_TIMEOUT_MS`, `CLAUDE_STREAM_FIRST_BYTE_TIMEOUT_MS`.

**Limitations:** SSO profiles do not support the Budget-Killer pool or key copying. Model loading, test connection and the budget
display (one budget per tenant, see below) use the access token.

## Several tenants under one SSO profile

A gateway that serves several tenants (teams, cost centres) lists its models as `<tenant>/<model>`, for example
`acme/claude-sonnet-5-5` and `globex/claude-sonnet-5-5`. A person with access in two tenants still keeps **one** profile and
**one** login: the same SSO token, the same helper, the same base URL. KeyZapper reads the model list from the gateway
(`/v1/models`) together with the budgets; once it contains two or more tenants, every project row shows a **Tenant** selection.

Choosing a tenant replaces only the four model settings of that project (`ANTHROPIC_MODEL` and
`ANTHROPIC_DEFAULT_{OPUS,SONNET,HAIKU}_MODEL`) with the models of that tenant: each model of the profile moves to
`<tenant>/<name>` if the tenant has it, otherwise the tenant's newest model of that tier is used. The default model keeps tier
aliases such as `sonnet` or `opusplan`; a model ID moves along, or falls back to the tenant's Sonnet model. The helper
(`--profile <ID>`) and `ANTHROPIC_BASE_URL` stay the same, so Claude Code in that project calls the chosen tenant after one
sign-in. *As in the profile* returns to the profile's own models.

The choice is saved with the project assignment and survives *Apply again*, profile edits and backups; assigning the project to
another profile clears it. Subscription profiles have no tenants.

## Claude subscription profiles

A profile of type *Claude subscription* has no endpoint and no key: assigned projects use the Claude account Claude Code is
logged in with (`/login`). KeyZapper writes `apiKeyHelper`, `ANTHROPIC_BASE_URL`, `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`
and the gateway model variables as empty values into `.claude/settings.local.json`, so a default profile or shell variables do
not apply there; an optional default model (e.g. `opus`) goes into `ANTHROPIC_MODEL`.

For plan and usage, KeyZapper reads Claude Code's login (macOS keychain item "Claude Code-credentials", otherwise
`~/.claude/.credentials.json`) and asks `api.anthropic.com/api/oauth/usage`, like Claude Code's `/usage`. The token is never
refreshed, stored or copied; while it has expired (Claude Code renews it when it runs), the last known values are shown. The
endpoint is not a public API and may change. The gateway allowlist does not apply to these profiles, and the repository analysis
needs a gateway profile.

## Claude in Microsoft 365 through the profile

A profile can also set up **Claude for Microsoft 365**, the Claude add-in for Excel, Word and PowerPoint, behind the company
gateway. KeyZapper does this for the profile that has *Claude in Office* switched on: in the profile editor, or through the
`Office` dict of a managed profile (see [Distribution via Microsoft Intune](#distribution-via-microsoft-intune)). If several
profiles have it on, those defined by IT come first, then the first of your own.

**What it does:** KeyZapper loads Anthropic's manifest template (`https://pivot.claude.ai/manifest.xml`, only while Claude in Office
is switched on), sets the add-in ID of your company and appends the gateway and Entra parameters to the add-in's two page addresses: `gateway_url`,
`gateway_api_format=anthropic`, `entra_sso=1`, `graph_client_id`, `entra_scope`, `gateway_auth_source=entra` and, if set,
`disabled_features`. The result is sideloaded:

* **macOS:** `<add-in ID>.manifest.xml` in the `wef` folder of each installed app below `/Applications` (Excel, Word, PowerPoint),
  that is `~/Library/Containers/com.microsoft.<App>/Data/Documents/wef`.
* **Windows:** the file in `%LOCALAPPDATA%\KeyZapper\office`, registered as a value named after the add-in ID under
  `HKCU\SOFTWARE\Microsoft\Office\16.0\Wef\Developer`.

**Entra only, no secret:** the add-in signs in through Microsoft Entra SSO. The manifest contains no secret, and KeyZapper never
reads or writes the add-in's own storage. The Entra app registration and the gateway route that accepts its tokens belong to the
gateway side; KeyZapper only passes their IDs.

**Fields:** gateway URL (https; its host must be allowed by `AllowedGatewayHosts`), Entra client ID, Entra scope
(`api://<client ID>/access_as_user`), add-in ID and, optionally, add-in features to switch off (comma-separated). The add-in ID is
a GUID of your own in lowercase; the ID of the marketplace add-in is refused, so the sideloaded add-in never takes its place.

**Restart Office once,** then open Claude via *Insert → My Add-ins*. The profile card shows the apps it was set up for, or why
that did not work. On macOS the first write makes the system ask whether KeyZapper may access data of other apps (the Office
containers); allow it. If it was denied, the profile card shows the error, and a change of the profile or *Refresh status* tries again.

**When it writes:** only if something changed: the profile's Office settings, Anthropic's template or the installed Office apps.
KeyZapper keeps its own record in `state.json` and never reads the Office folders to find out. Without a connection it uses
the last template it loaded. Without one, or if the template does not look like Anthropic's, nothing is written and an
existing manifest stays.

**Removing:** switching *Claude in Office* off, deleting the profile or deactivating KeyZapper removes KeyZapper's own manifest
(on Windows also its registry value) from Office. Files of other add-ins stay.

**Quitting or uninstalling KeyZapper leaves the manifest in place,** because only KeyZapper removes it, and only while it runs.
Before uninstalling, switch *Claude in Office* off in the profile or deactivate KeyZapper; that removes the manifest (on Windows
also its registry value) first.

**Office may cache sideloaded manifests.** If a change (another gateway, another add-in ID, switched off) does not show up after
the restart, restart Office once more or, if that does not help, clear Office's add-in cache. This is a hint: KeyZapper neither
checks nor touches that cache.

**Sideloading is Microsoft's route for developing and testing add-ins,** not for distributing them in production. For that,
Microsoft supports central deployment of the manifest through the Microsoft 365 admin center. Use the same add-in ID there, so
both routes refer to the same add-in.

## How it works

**Setup (once):** The key goes to the helper via stdin and ends up only in the key file `keys.json`. The app writes only references into the project.

![Setup: KeyZapper stores the key via keyzapper-helper and writes settings.local.json in repo A and repo B](docs/diagrams/keyzapper-flow-setup.svg)

**Runtime (every Claude request):** Claude Code fetches the key for each project itself via the helper, even when the app is closed.

![Runtime: Claude Code in repo A and repo B calls keyzapper-helper with its own profile ID and sends key A or key B to LiteLLM](docs/diagrams/keyzapper-flow-runtime.svg)

The full view with explanations is at [docs/diagrams/keyzapper-flow.html](docs/diagrams/keyzapper-flow.html).

The app writes to `<project>/.claude/settings.local.json`:

| Key | Value |
|---|---|
| `apiKeyHelper` | `'/Applications/KeyZapper.app/Contents/MacOS/keyzapper-helper' credential --profile <UUID>` on macOS, the quoted path of `keyzapper-helper.exe` in the install folder on Windows (Key and SSO profiles); `'/mnt/c/…/keyzapper-helper.exe' credential --profile <UUID>` for projects in WSL |
| `env.ANTHROPIC_BASE_URL` | LiteLLM endpoint of the profile |
| `env.ANTHROPIC_DEFAULT_OPUS_MODEL`, `…_SONNET_MODEL`, `…_HAIKU_MODEL`, `env.ANTHROPIC_MODEL` | Gateway model names of the profile (if set), e.g. `eu.anthropic.claude-sonnet-5-…` |
| further `env.*` | Additional environment variables of the profile, e.g. `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS=1` for Bedrock via LiteLLM |
| `env.ANTHROPIC_API_KEY`, `env.ANTHROPIC_AUTH_TOKEN` | `""`. This neutralizes inherited values from the IDE or shell that would otherwise produce mixed auth headers. |

Existing settings are preserved. Changes are atomic and idempotent: setting up repeatedly changes nothing.

Conflicts that prevent setup:
* Managed Claude settings that set `apiKeyHelper` or `ANTHROPIC_*`
* `CLAUDE_CODE_USE_BEDROCK`, `_VERTEX` or `_FOUNDRY` in any settings level
* foreign values for the same keys in `settings.local.json`
* a checked-in `settings.local.json`

**Helper interface:** `keyzapper-helper credential|store|status|delete --profile <UUID>`. The key flows only via stdin and stdout,
never via arguments or logs.

| Exit code | Meaning |
|---|---|
| 0 | ok |
| 64 | usage |
| 65 | unknown profile |
| 66 | key missing |
| 67 | session expired (SSO only; sign in again) |
| 70 | internal |
| 77 | key store (`keys.json`) unreadable or not writable |
| 78 | metadata corrupt, endpoint not https (http only for localhost) or not in `AllowedGatewayHosts` |

Metadata without keys is stored in `state.json` in the app data folder, the keys in `keys.json` next to it: `~/Library/Application Support/KeyZapper` on macOS (owner-only), `%LOCALAPPDATA%\KeyZapper` on Windows (user profile folder). Keys from the former Keychain-based version are offered for a one-time move on macOS (macOS may ask for permission once per key).

## Projects in WSL (Windows)

A project folder inside a WSL distribution (`\\wsl.localhost\<distro>\home\…`, also the older `\\wsl$\…`) is assigned like any
other folder: pick it in the folder dialog under "Linux" or drag it onto the window. Git root, status and the Git exclude
entry come from the distribution's own git (`wsl.exe -d <distro> --exec git …`), not from Windows git, which reports
"dubious ownership" on these shares.

Claude Code inside WSL is a Linux process and runs `apiKeyHelper` with `/bin/sh`, so `settings.local.json` in such a
project contains the Windows helper by its mount path, quoted for sh:

```json
"apiKeyHelper": "'/mnt/c/Users/<user>/AppData/Local/Programs/KeyZapper/keyzapper-helper.exe' credential --profile <UUID>"
```

The helper runs through WSL interop as a Windows process and reads `keys.json` from `%LOCALAPPDATA%\KeyZapper` as usual;
the key never enters the distribution's file system. Requirements: WSL interop and the drive automount must be on (the
defaults; `[interop] enabled=true` and `[automount] enabled=true` with the default root `/mnt` in `/etc/wsl.conf`), and git
must be installed in the distribution for the Git root to be detected (otherwise the picked folder itself is assigned).

## Distribution via Microsoft Intune

Every Git tag `X.Y.Z` builds the packages `KeyZapper-X.Y.Z.pkg` (macOS) and `KeyZapper-X.Y.Z.msi` (Windows) via CI and attaches both to the GitHub release (see [Release](#release)).

**macOS: create the app:** *Apps › All apps › Create › macOS app (PKG)*. This is the unmanaged PKG type, which also accepts
unsigned packages; it requires the Intune Management Agent ≥ 2308.006.
* Minimum OS: macOS 14
* Detection rules › Included apps: `io.github.bl0rb.keyzapper` with the package version, “Ignore app version” = No

**Windows: create the app:** *Apps › All apps › Create › Line-of-business app*, upload the `.msi` (per-machine install to Program Files).

**Signing:** Keys are not in the Keychain, so signing does not affect key access. For macOS, `SIGN_IDENTITY` and `INSTALLER_IDENTITY`
(`desktop/scripts/build-pkg.sh`) sign with a Developer ID; without them the build is ad-hoc signed.

**Uninstall:** Intune has no uninstall assignment for PKG apps. First choose “Remove assignment” in the app,
then delete `/Applications/KeyZapper.app`. On Windows, uninstall the MSI via Intune or *Apps & features*.

### Configuration

**macOS:** *Devices › Configuration › Create › macOS › Templates › Preference file*, Preference domain name `io.github.bl0rb.keyzapper`.
Template with every key: [docs/intune/io.github.bl0rb.keyzapper.plist](docs/intune/io.github.bl0rb.keyzapper.plist). It contains only
key-value pairs without the `<plist>`/`<dict>` wrapper, as Intune requires.

**Windows:** The same keys are read as registry values under `HKLM\SOFTWARE\Policies\KeyZapper` (machine policy wins) or `HKCU\SOFTWARE\Policies\KeyZapper`:
strings as `REG_SZ`, lists as `REG_MULTI_SZ`, switches as `REG_DWORD` 0/1, `ManagedProfiles` as a JSON array in a `REG_SZ`.
Template with every key: [docs/intune/keyzapper-windows.reg](docs/intune/keyzapper-windows.reg) (deploy e.g. via PowerShell script or Settings catalog).

**Important:** For users receiving SSO profiles via managed settings, do not set `apiKeyHelper`, `env.ANTHROPIC_BASE_URL`,
`ANTHROPIC_API_KEY` or `ANTHROPIC_AUTH_TOKEN` in `managed-settings.json` (or `managed-settings.d/*.json`). Managed settings
outrank project settings; KeyZapper aborts binding on conflict.

| Key | Type | Effect |
|---|---|---|
| `ManagedProfiles` | Array of dicts: `Name`, `Endpoint`, optional `OpusModel`, `SonnetModel`, `HaikuModel`, `ModelAlias`, `Environment` (dict), `ID`, `Type` (`apiKey` default or `oidc`), `OIDCIssuer`, `OIDCClientID`, `OIDCScope`, `OIDCTokenType`, `Office`, `ClaudeDesktop` (dicts, see below) | Profiles are created automatically and cannot be edited or deleted; developers only enter the key (or sign in for SSO) and may switch on *Claude in Office* and *Claude Desktop (SSO)* for the profile where IT sets neither (see below). Without `ID` the profile ID is derived from `Name`, so renaming creates a new profile. For SSO (`oidc` type), `OIDCIssuer` and `OIDCClientID` are required; `OIDCScope` defaults to `openid profile offline_access`; `OIDCTokenType` is `access` (default) or `id`, other values discard the profile. |
| `ClaudeDesktop` (inside a `ManagedProfiles` entry) | Dict: `OIDCIssuer`, `OIDCClientID`, optional `GatewayURL` (default: the profile's `Endpoint`), `Scopes`, `BearerTokenType` (`id_token` default or `access_token`), `CredentialKind` (`external-idp` default or `interactive`), `Config` (dict of further Claude Desktop keys), `AutoApply` (Bool, default `false`) | Claude Desktop signs in itself with the gateway's OIDC client when this profile is chosen for it (see [Claude tools](#claude-tools)). The keys map to Claude Desktop's configuration: `GatewayURL` → `inferenceGatewayBaseUrl`, `OIDCIssuer`/`OIDCClientID`/`Scopes`/`BearerTokenType` → the `issuer`, `clientId`, `scopes` and `bearerTokenType` of the OIDC block, `CredentialKind` → `inferenceCredentialKind`; `inferenceProvider: gateway` and `appendOfflineAccess: true` are always written, everything else (telemetry, banner, allowed hosts, …) goes into `Config`. Without the dict, developers may fill in *Claude Desktop (SSO)* in the profile themselves; the dict from IT replaces their entry. `external-idp` needs Claude Desktop 2.7032.0 or newer; set `interactive` while older versions are in use. A block with unusable values (also an unknown `CredentialKind`) is kept; KeyZapper then writes nothing for Claude Desktop. |
| `Office` (inside a `ManagedProfiles` entry) | Dict: `Enabled` (Bool, default `true`), `AddinID`, `GatewayURL`, `EntraClientID`, `EntraScope`, `DisabledFeatures` (array or comma-separated string) | Sets up [Claude in Microsoft 365](#claude-in-microsoft-365-through-the-profile) from this profile. Without the dict, developers may switch on *Claude in Office* in the profile themselves; the dict from IT replaces their entry. Values are trimmed. A block with unusable values is kept and reported on the profile card; nothing is written then. |
| `AllowedGatewayHosts` | Array of strings (`host` or `*.domain`) | Profiles only for these hosts. The helper does not release keys for other hosts (exit 78). Applies to the gateway host only; the IdP host need not be listed. Empty means no restriction. |
| `DefaultEndpoint`, `DefaultModelAlias` | String | Prefill when creating your own profiles |
| `MinimumClaudeCodeVersion` | String | Threshold for the warning about outdated Claude CLIs (default 2.1.288) |
| `OneDriveBackup` | Bool | Backup of profiles and assignments to OneDrive (see below) |
| `BackupDirectory` | String, `~` allowed | Explicit backup folder; enables the backup even without `OneDriveBackup` |
| `UpdateCheckEnabled` | Bool (default `true`) | In-app update check. When distributing via Intune, set to `false`, otherwise Intune may overwrite a newer version that was installed by the user. |
| `AllowKeyExport` | Bool (default `true`) | Whether keys may leave the key store via the app. `false`: no copying to the clipboard; backups contain profiles and assignments only. |
| `ManagedMcpServersURL` | String (https URL) | Address of the central MCP server list (see [Central MCP server list](#central-mcp-server-list)). |
| `ClaudeToolsEnabled` | Bool (default `true`) | Shows the *Claude Tools* page for installing Claude Code, its IDE plugins and Claude Desktop. |
| `ClaudeDesktopAutoInstall` | Bool (default `false`) | Installs a missing Claude Desktop in the background from Anthropic's official sources, also with the *Claude Tools* page hidden (see [Claude tools](#claude-tools)). |
| `ProxyURL` | String (`http://host:port`, optionally `user:password@`) | Proxy for KeyZapper's own internet traffic: the update check and installer downloads from `github.com`, the Claude installers from `claude.ai`. Without it, KeyZapper uses the proxy environment variables (`HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, honouring `NO_PROXY`) and otherwise the system proxy (macOS network settings, Windows internet options; a PAC file is not evaluated). Gateway and identity-provider calls are not routed through it. |
| `IgnoreDownloadCertificateErrors` | Bool (default `false`) | The update check and the downloads accept certificates the system does not trust, for a proxy with TLS inspection whose root certificate is not installed on the device (error "The certificate was not trusted"). Only KeyZapper's own internet traffic is affected; the installers stay protected by the SHA-256 checksum and minisign signature (KeyZapper) and by Anthropic's code signature (Claude Desktop). Prefer installing the proxy's root certificate on the device. |

Test locally (user level; values managed via Intune take precedence):

```bash
defaults write io.github.bl0rb.keyzapper AllowedGatewayHosts -array litellm.firma.example
```

### OneDrive backup

The app backs up profiles and project assignments to `~/Library/CloudStorage/OneDrive-<Company>/KeyZapper/keyzapper-backup.json` (macOS) or into the `KeyZapper` folder of the OneDrive folder (Windows).
A business account takes precedence over “OneDrive-Personal”. **Keys are never backed up.** On a new computer the app offers
to restore or discard the backup (“Restore” or “Discard”). Assignments are only applied for existing folders; the keys
have to be entered again. As long as a found backup has been neither restored nor discarded, it is not overwritten.

### Central MCP server list

IT maintains the MCP servers in one JSON file on its own server and distributes only its address via Intune
(`ManagedMcpServersURL`). The file uses Claude Code's `.mcp.json` format; template:
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

* KeyZapper loads the list at startup, every 10 minutes, after an SSO sign-in and via *Reload* on the *MCP Servers* page.
  The server answers `GET` with `200` and the JSON (at most 1 MB); `401`/`403` is shown as "sign-in required".
* **Sign-in:** If an SSO profile is signed in whose gateway or identity provider has the same host as the list, KeyZapper
  sends its token as `Authorization: Bearer …` (the token type of the profile). The server can thus answer per user or group.
  The token only goes to hosts that receive it anyway.
* The servers appear with the badge *Managed by IT*: developers switch them on per project but cannot edit or delete them.
  Changes to the list are written into the projects using them; servers IT removes from the list leave the projects.
* The last list fetched stays in `mcp-catalog.json` in the app data folder, so the servers keep working offline; a failed
  fetch is shown on the page and the previous list stays in effect.
* Only `https` addresses (plain http only for localhost), also for remote servers on the list. Entries with an invalid
  name, without command or URL are left out and listed on the page; so is an entry with the name of a developer's own server.
* Secrets do not belong in the file; use `${NAME}` references (Claude Code fills them from the environment) or OAuth.

## Claude settings check and default profile

The *Claude settings* page in the sidebar checks `~/.claude/settings.json` for problems Claude Code accepts
silently: invalid JSON (validated with the same strict `JSON.parse` as Claude Code, e.g. trailing commas), plaintext keys,
an endpoint without a key, non-text values in `env`, provider switches that bypass LiteLLM, model variables and fixed
model IDs instead of an alias. A banner shows problems at any time.

* **Repair / Create** makes the file valid, removes plaintext keys and turns `env` values into text. A missing file is
  created. Before every change KeyZapper saves a backup `settings.json.keyzapper-<time>.bak`.
* **Default profile** writes a profile into `~/.claude/settings.json`, so Claude Code uses it in all folders without their
  own assignment instead of a plaintext key or the normal login. Assigned projects keep their own profile.

## Claude tools

The *Claude Tools* page (hidden with `ClaudeToolsEnabled = false`) shows what is installed and installs, without administrator rights:

| Tool | Source | Installed to |
|---|---|---|
| Claude Code (CLI) | Anthropic's native installer (`curl -fsSL https://claude.ai/install.sh \| bash`, Windows `irm https://claude.ai/install.ps1 \| iex`) | `~/.local/bin/claude`, updates itself |
| Claude Code for VS Code | `code --install-extension anthropic.claude-code` (VS Code Marketplace) | VS Code of the user |
| Claude Code for IntelliJ IDEA | `idea installPlugins com.anthropic.code.plugin` (JetBrains Marketplace); IntelliJ must be closed | every IntelliJ IDEA found (also Toolbox installs) |
| Claude Desktop | `.dmg` from `claude.ai/api/desktop/darwin/universal/dmg/latest/redirect` (macOS; checks Anthropic's Developer ID `Q6L2SF6YDW` before copying to `/Applications` or `~/Applications`), `.msix` via `Add-AppxPackage` (Windows) | Applications folder / current user |

**Sign-in:** Claude Code in the terminal, in VS Code and in IntelliJ uses the profile of assigned projects and the default
profile elsewhere (key, SSO or Claude subscription). For key and SSO profiles, turn on *Disable Login Prompt*
(`claudeCode.disableLoginPrompt`) in VS Code; the page opens that setting. If IntelliJ does not find `claude`, enter the path
shown on the page under *Settings › Tools › Claude Code [Beta] › Claude command*.

**Claude Desktop with a profile:** Choose a key or SSO profile and *Apply*. KeyZapper writes one configuration file for
Claude Desktop's third-party mode into its configuration library (`~/Library/Application Support/Claude-3p/configLibrary`,
`%LOCALAPPDATA%\Claude-3p\configLibrary`) and selects it in `_meta.json`:

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

Claude Desktop thus gets the key or SSO token from `keyzapper-helper`, like Claude Code; no key is in the file. The
helper TTL follows `CLAUDE_CODE_API_KEY_HELPER_TTL_MS` of the profile (default 300 s). Then quit and reopen Claude Desktop
and choose to start with this configuration on the sign-in screen. *Normal sign-in (claude.ai)* removes the entry again
and applies the configuration that was applied before, if it still exists. If the profile changes, KeyZapper updates the
file. A Claude Desktop configuration IT deploys via MDM profile (`com.anthropic.claudefordesktop`) or registry
(`HKLM\SOFTWARE\Policies\Claude`) takes precedence when it sets more than the app's update, restart and proxy keys; KeyZapper
then writes nothing and the page points this out. Deactivating KeyZapper removes the entry and activating writes it again.

**Claude Desktop set up by IT:** In the `ClaudeDesktop` dict of a managed profile, `Config` adds further keys of Claude
Desktop's [configuration reference](https://claude.com/docs/third-party/claude-desktop/configuration) to the entry (native
types, e.g. `disableDeploymentModeChooser`, `coworkEgressAllowedHosts`). The sign-in keys KeyZapper derives win; keys with
secrets, headers, local programs or credential files and bootstrap keys are refused, and then nothing is written. With
`AutoApply` KeyZapper writes and applies the entry without a click and writes it again at start and on refresh if it is
missing or was changed (only one managed profile may set it). In between the user can still change it, so nothing in it is
enforced; keys that must be enforced stay in MDM. With `ClaudeDesktopAutoInstall` KeyZapper installs a missing Claude Desktop
in the background after writing the configuration, independent of `ClaudeToolsEnabled`; a failed installation is tried again
on *Refresh status*.

**Claude Desktop with its own SSO sign-in:** If the gateway has its own OIDC client for Claude Desktop (another client ID or
gateway route than Claude Code), switch on *Claude Desktop (SSO)* in the profile and enter the issuer, the client ID and
optionally the gateway URL, the scopes and the token type (`id_token` default, `access_token` needs the gateway's scopes).
KeyZapper then writes the external-IdP sign-in instead of the helper (`inferenceCredentialKind: external-idp`,
`inferenceIdpOidc`; read by Claude Desktop 2.7032.0 and newer): Claude Desktop signs in in the browser, keeps its token itself
and lists the gateway's models (`/v1/models`). No key or token is written. For older Claude Desktop versions choose *Older than
2.7032.0* in the profile's *Claude Desktop version* field: KeyZapper then writes the older spelling (`interactive`,
`inferenceGatewayOidc`), which newer versions read as the same sign-in. IT sets this with a `ClaudeDesktop` dict in
`ManagedProfiles` (`CredentialKind`: `external-idp` or `interactive`).

**Trying it on a managed profile:** A profile from Intune can still get *Claude in Office* and *Claude Desktop (SSO)* from the
developer, as long as IT's entry has no `Office` or `ClaudeDesktop` dict: *Edit* opens the profile with only those two sections
enabled, and the settings survive the next sync with Intune and a restart. Once IT adds the dict, it replaces the developer's
settings and locks the section; a dict IT removes again stays as the developer's own until they switch it off.

## Deactivating KeyZapper

The switch at the bottom of the sidebar deactivates KeyZapper: it removes its entries from all projects and from
`~/.claude/settings.json`, so Claude Code uses its normal login everywhere. The assignments stay saved and are written
again when activating. Claude in Microsoft 365 is removed from Office while KeyZapper is deactivated and set up again on activation.

## Encrypted backup and restore

*Export Backup…* on the *Backup* page writes profiles, project assignments and keys into a `.kzbackup` file,
encrypted with a password of at least 12 characters. *Import Backup…* restores them on the same or another computer:
profiles and keys with the same ID are overwritten, and projects are assigned if their folder exists. The password is
stored nowhere – without it the backup cannot be restored.

Encryption: AES-256-GCM with a key derived via PBKDF2-HMAC-SHA256 (600,000 iterations, random salt). The file
parameters are authenticated, so a wrong password or a modified file is detected. The file is saved with permissions
`0600`. With `AllowKeyExport = false` the backup contains no keys. Unlike the automatic OneDrive backup (never keys),
this backup is created manually and on demand.

## Updates and version

On startup the app checks the latest [GitHub release](https://github.com/bl0rb/ClaudeKeyZapper/releases); you can also check manually via
*KeyZapper › Check for Updates…* or the version line at the bottom of the app. If a newer version is available, “Install”
downloads the installer (`.pkg` on macOS, `.msi` on Windows), verifies the SHA-256 checksum published by GitHub and the release's
minisign signature (`<installer>.minisig`) against the public key built into the app ([desktop/update-key.pub](desktop/update-key.pub)),
then opens the system installer. Without a checksum or a valid signature the update is refused; the signed file name must match, so an
older signed package cannot pass as a newer one.
Administrator rights are required. Packages are only downloaded from `github.com`. The installed version is shown at the bottom of the
app and under *KeyZapper › About KeyZapper*.

With Intune, `UpdateCheckEnabled = false` turns the check off. Updates then arrive as a new package in Intune.

Behind a company proxy the check and the download use the proxy from `ProxyURL` (Intune), else the proxy environment variables, else
the system proxy. A failed check names the proxy it went through. If the proxy inspects TLS with a certificate the device does not
trust, install its root certificate on the device, or set `IgnoreDownloadCertificateErrors` for the check and the downloads.

## Known limitations

* One profile per Git repository; subfolders and worktrees cannot be assigned differently.
* If Claude is started outside the assigned area (e.g. in a subfolder of a non-Git folder), it uses the
  developer's default sign-in.
* The IntelliJ integration has not yet been verified in the pilot.

## Development

```
desktop/core        Rust library keyzapper-core: data model, metadata, key store, helper logic, Claude settings, Intune configuration, backup
desktop/src-tauri   Tauri app (macOS, Windows) and keyzapper-helper (apiKeyHelper for Claude Code)
desktop/ui          Web interface (HTML, JS, CSS)
desktop/locales     English translations (German source strings)
desktop/scripts     build-pkg.sh (macOS), build-msi.sh (Windows), l10n_check.py
```

Tests:

```bash
cd desktop && cargo test -p keyzapper-core
```

Run the app in development mode (needs Rust and Node.js):

```bash
cd desktop && npm ci && npm run dev
```

Build the installer locally (`desktop/dist/KeyZapper-<version>.pkg` on macOS, `.msi` on Windows in Git Bash):

```bash
cd desktop && VERSION=1.0.0 scripts/build-pkg.sh
```

```bash
cd desktop && VERSION=1.0.0 scripts/build-msi.sh
```

## Release

A tag in the format `X.Y.Z` starts [.github/workflows/release.yml](.github/workflows/release.yml). The workflow runs the
tests, builds `KeyZapper-X.Y.Z.pkg` (macOS) and `KeyZapper-X.Y.Z.msi` (Windows), signs both with minisign and publishes them with
their `.minisig` files in one GitHub release. Signing needs the repository secret `MINISIGN_SECRET_KEY` (the secret key matching
`desktop/update-key.pub`); without it the workflow stops instead of publishing unsigned installers.
A tag like `X.Y.Z-beta.N` creates a pre-release, which is only offered with the *Beta versions* switch in the sidebar
(on by default while a beta is installed).

```bash
git tag 1.0.0
```

```bash
git push origin 1.0.0
```

## License

[MIT](LICENSE) © 2026 bl0rb
