# Security audit

**Scope:** the desktop app in `desktop/` (Rust core, Tauri shell, `keyzapper-helper`) as of 2026-11-03, version 1.5.4,
commit `3d470a0`. The Swift tree in `Sources/` is the former macOS-only app (≤ 1.x, keychain-based) and is out of scope.
**Method:** manual code review of every module that touches key material, network traffic, cryptography, the settings
files KeyZapper writes into projects, the update path and the CI pipeline. No penetration test.

## Threat model

| Threat | Relevant controls |
| --- | --- |
| Man-in-the-middle between Claude Code and the gateway | keys only leave the machine over https to allowed hosts; TLS verified against the system trust store |
| Man-in-the-middle on KeyZapper's own traffic (update check, installer downloads) | GitHub-only URLs, SHA-256 digest, minisign signature, code-signature check of downloaded Claude.app / MSIX |
| Other users on the same machine | owner-only data folder and files |
| Malware running as the same user | out of scope: it can read any file of the user, also keychains with helper access; compensating control is full-disk encryption plus endpoint protection |
| Hostile project folders, backups, catalog files | parsers reject what they do not understand, no code execution from data |
| Supply chain (dependencies, CI) | actions pinned to commit SHAs, `contents: read` by default, minisign secret required to publish, `cargo audit` in CI |

## Key storage

Keys live in `keys.json`, SSO tokens in `sso-tokens.json`, both in the app data folder
(`~/Library/Application Support/KeyZapper` on macOS, `%LOCALAPPDATA%\KeyZapper` on Windows). There is no keychain or
credential manager by design: the app and the helper are two binaries, and every keychain read by the helper would prompt.

* The folder is created `0o700`, the files `0o600`, and every write goes through a temporary file that is renamed into
  place (`paths::write_atomic`). **Fixed in this review:** the temporary file used to be created with the umask and made
  private afterwards, leaving the key world-readable for a moment; it is now created owner-only from the first byte.
* On Windows the files inherit the ACL of `%LOCALAPPDATA%` (the user, SYSTEM and Administrators). No explicit DACL is set;
  `KEYZAPPER_HOME` is only meant for tests and must not point outside the profile.
* The key is never an argument of a process: `keyzapper-helper store` reads it from stdin, `credential` prints it to stdout
  without a newline, stderr never carries key material. Profile IDs are validated as UUIDs before they reach a command line.
* The key never reaches the web view: the UI gets the masked hint (`••••7f3a`), the full key only travels from the web view
  to Rust when the user types it. Copy to clipboard happens in Rust, marked confidential and cleared after 60 s.
* Keys are never written into `settings.local.json`; Claude Code gets `apiKeyHelper` and `ANTHROPIC_BASE_URL`, and
  `ANTHROPIC_API_KEY` / `ANTHROPIC_AUTH_TOKEN` are set to `""` so no key from the shell environment is mixed in. The file
  and `.mcp.json` are added to `.git/info/exclude`.
* Backups (`.kzbackup`) are AES-256-GCM with a PBKDF2-HMAC-SHA256 key (600 000 iterations, random 16-byte salt), the envelope
  parameters are authenticated as associated data, passwords need 12 characters, and hostile files cannot stall the KDF
  (iterations bounded). SSO tokens are never part of a backup.

## Transport (MITM)

* Profile endpoints, the OIDC issuer and the central MCP list must use https; plain http is accepted only for loopback
  hosts (`localhost`, `127.0.0.1`, `[::1]`). The helper refuses to release a key for anything else (exit 78).
* IT can pin the gateways with `AllowedGatewayHosts`; the helper enforces the list as well, so a rewritten profile cannot
  send a key elsewhere.
* All HTTP goes through `ureq` with the platform TLS stack (`native-tls`): Secure Transport on macOS, SChannel on Windows.
  Company CAs installed by IT are trusted like in the browser; nothing else is.
* Gateway and identity-provider calls never use a proxy and never relax certificate checks.
* `IgnoreDownloadCertificateErrors` (Intune/registry) disables certificate verification **only** for KeyZapper's own
  internet traffic (GitHub API, GitHub downloads, claude.ai installer downloads) behind a TLS-inspecting proxy. What is
  downloaded stays protected by signatures (see below). Prefer installing the proxy CA in the system trust store; the flag is
  a fallback.
* SSO: authorization-code flow with PKCE (S256), a random `state`, discovery whose `issuer` must match the configured one,
  and a loopback redirect on a random port that only accepts `/callback` with the right state. Refreshes are serialised
  across processes with a lock file.

## Updates and installers

* Release metadata comes from `api.github.com`; package, signature and release page are accepted only from `github.com`.
  **Fixed in this review:** the release page link was not host-checked and could have pointed anywhere when certificate
  checks are relaxed.
* A package installs only if its SHA-256 matches the published digest **and** its minisign signature verifies against the
  public key compiled into the app (`desktop/update-key.pub`). The trusted comment names the file, so an old signed package
  cannot pass as a newer version. Download sizes are capped (512 MB package, 16 KB signature).
* CI refuses to publish a release without the signing secret and verifies every signature it makes.
* Claude Desktop downloads are checked before installation: on macOS `codesign --verify --strict` against Anthropic's team
  ID, on Windows `Add-AppxPackage` verifies the MSIX signature. The Claude CLI uses Anthropic's own installer through the
  system `curl`, which is not affected by the ignore flag.

## Application shell

* Tauri 2 with CSP `default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:` and the minimal capability
  set (`core:default`, window dragging). No shell, fs or http plugin is exposed to the web view.
* Only `innerHTML` use is a static icon table; all user and server data is rendered as text.
* Process invocations use fixed program paths or arguments arrays, never a shell string with user data; the helper path is
  shell-quoted for `apiKeyHelper`.

## Findings

| # | Severity | Finding | Status |
| --- | --- | --- | --- |
| 1 | Low | Private files (`keys.json`, `sso-tokens.json`, Claude Desktop config) were created with the umask and made `0o600` afterwards. | **Fixed** (`paths::write_atomic`, test added) |
| 2 | Low | The release page URL from the GitHub API was opened unchecked; with relaxed certificate checks a proxy could hand out a phishing link. | **Fixed** (`update::release_info`, test added) |
| 3 | Medium | CI builds installers ad hoc signed (`SIGN_IDENTITY=-`), without Developer ID, notarization or Authenticode. First installs rely on TLS to GitHub alone and Gatekeeper/SmartScreen warnings teach users to bypass them. In-app updates are protected by minisign. | Open: sign and notarize release builds |
| 4 | Medium | `IgnoreDownloadCertificateErrors` turns off TLS verification for KeyZapper's own internet traffic when IT sets it. Mitigated by the signatures above; the gateway and the identity provider are never affected. | Accepted, documented; prefer a trusted proxy CA |
| 5 | Low | Keys and SSO refresh tokens are plaintext files protected by file-system permissions only; any process of the same user can read them. A keychain was dropped on purpose (prompts), and DPAPI would not change the same-user threat model. | Accepted; rely on full-disk encryption and endpoint protection |
| 6 | Low | The central MCP list may define `stdio` servers (`command`, `args`, `env`). A compromised list host (IT's own, https only) could push a command that Claude Code runs once a developer enables that server. | Accepted; IT should prefer http/sse servers and keep the host under its control |
| 7 | Low | Windows: no explicit DACL on the data files; protection is the inherited `%LOCALAPPDATA%` ACL. | Accepted |
| 8 | Info | No dependency vulnerability scan in CI. | **Fixed**: `security.yml` runs `cargo audit` on pushes, PRs and weekly |
| 9 | Info | Subscription usage reads Claude Code's own OAuth token (keychain item or `.credentials.json`) to call `api.anthropic.com`; the token stays in memory and goes out over the direct, verified agent. | By design |

Dependency versions at review time: `ureq` 3.4.2, `native-tls` 0.2.18, `openssl` 0.10.81, `aes-gcm` 0.10.3,
`pbkdf2` 0.12.2, `minisign-verify` 0.3.0, `tauri` 2.12.1, `wry` 0.57.0. No known advisories were checked offline; the CI
job covers that from now on.
