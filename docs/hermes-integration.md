# Hermes integration: setup, recovery, and acceptance

> **Experimental, exact-build support only.** KeyZapper writes Hermes configuration only when the selected installation matches the exact locally validated build identity. The current evidence is for one observed developer runtime: Hermes source archive commit `48438d0b5cd1909ca3b6cb733c410294c13e6d48`, Python `3.14.10`, Anthropic SDK `0.125.0`, and its exact console-entrypoint/source fingerprints recorded in `desktop/core/tests/fixtures/hermes-compatibility.json`. This is not blanket support for a Hermes version, another installation, macOS, or the original host shim. The selected launcher must be exactly the recorded entrypoint path; a byte-identical copy of it in another directory is refused before it is ever executed. Unknown or mismatched builds remain read-only/unsupported; do not bypass that guard.

This integration binds a **fixed KeyZapper credential profile, tenant (if selected), and model** to one explicitly chosen Hermes installation. It does not add project-based credential or model routing. The selected model is explicitly written as Hermes's default model; switching projects does not switch this KeyZapper selection.

## Connect

### Before you begin

- Use a KeyZapper build that includes the Hermes page, and an existing Hermes installation. The page lists the executable and Hermes home it discovered; check both carefully. A target marked **Not validated** is not writable.
- Have the gateway/model authorization approved through your normal administrator. Do not paste credentials into chat or this guide. KeyZapper uses the credential already stored in its selected profile; it does not ask you to copy a token into Hermes or edit Hermes configuration by hand.
- For SSO, use **Sign In** on the existing KeyZapper SSO profile. Confirm that profile reports **Signed in**. Do not transfer refresh tokens, copy token files, or set up a second/manual Hermes credential path.
- Confirm the selected profile belongs to the intended gateway tenant and that the exact model ID is authorized. Subscription profiles are not offered for this integration.

### Connect and verify the selection

1. Open **Hermes** in KeyZapper and refresh installations. Select the exact validated installation, KeyZapper credential profile, tenant if applicable, and exact model ID. There is no automatic model or profile fallback.
2. Choose **Connect** (or **Update connection**). Review the confirmation preview: installation/home/executable, credential profile, tenant and model. Connecting changes Hermes's active default model to the selected model. Cancel if any value is unexpected; cancellation makes no configuration change.
3. Confirm only when the displayed target is correct. KeyZapper performs a local write and reads back status. This establishes that the exact local configuration was applied; it does **not** prove that a real gateway request or paid model test succeeded.
4. Start a new or restarted Hermes session. Existing sessions may retain their previously cached credential/model. Check the Hermes page's separate **connection status** and **test status**; the test status stays **Not tested** because no paid model-test command is implemented. Do not infer a successful model test from “Configured.”

The chosen installation, credential profile, tenant, and model remain fixed until explicitly updated. There is no project resolver or per-project routing in this integration. If a tenant is selected, the model ID must begin with that exact `<tenant>/` prefix; a mismatching pair is refused and neither value is rewritten automatically. While a profile is bound, its gateway endpoint, sign-in method and OIDC identity cannot be changed — remove the Hermes connection first, then update the profile and reconnect.

## Update and remove

**Update:** select the intended installation/profile/tenant/model again, inspect the preview, then explicitly confirm **Update connection**. It is not automatic. Recheck the reported status and start a new Hermes session.

**Remove:** select **Remove** for the configured connection, inspect the target in its confirmation, then confirm. KeyZapper restores only the Hermes configuration fields it owns to their recorded original values (including preserving whether a field was absent rather than replacing it with a default). It leaves unrelated Hermes settings untouched.

Before either action, KeyZapper checks for drift and ownership conflicts. If a managed value changed outside KeyZapper, or ownership/backup/recovery state is inconsistent, stop and use **Recovery** below; do not hand-edit the managed values or delete KeyZapper's recovery data. A moved helper path requires the explicit repair/recovery flow, not a guessed replacement.

## Credential caching

The 900-second helper TTL applies **only to static outputs that carry no expiry** (for example a stored API key, or an SSO response without a usable lifetime). That static value does not refresh on HTTP 401 by itself, so a running Hermes session can outlive an update. Restart the Hermes session and, if needed, re-run the KeyZapper SSO **Sign In** flow for the selected profile. Do not change the system clock or copy/transfer refresh tokens.

SSO credentials with a real lifetime are handled differently: the helper re-mints in the background from the same live source once the remaining lifetime falls below its margin (Claude Code's helper TTL plus 60 s). That is the purpose of the integration, not the static TTL. Two limits remain. First, a cache entry written before this integration has no recorded expiry provenance, so the first strict Hermes read forces one refresh instead of trusting the old one-hour fallback. Second, if the identity provider returns neither a positive `expires_in` nor a future JWT `exp`, the Hermes-facing path refuses the credential (fail closed) rather than inventing a lifetime; the legacy Claude token-only path keeps its previous behaviour for genuinely missing metadata. An explicitly non-positive `expires_in` is treated as already due, never as a new one-hour cache.

Concurrent refresh and expiry pairing have been verified only in the isolated local mock contract. That is not evidence of a live Entra/workplace session surviving refresh: no live Entra session, workplace sign-in, or paid request was exercised. If an error indicates an expired session, sign in again through KeyZapper, then restart Hermes. If a gateway rejects an otherwise current token, involve the gateway/identity administrator rather than weakening validation or changing token type by guesswork.

## Recovery

1. Refresh the Hermes installation list and read the displayed status. If the page reports a pending recovery, use **Repair** only for the exact displayed target and review its preview (cleanup versus rollback) before confirming.
2. If the target is missing, unsupported, or no longer at the displayed executable/home, do not select another installation as a substitute. Restore the original installation/path or request an approved local validation; a pending recovery remains blocked until its exact target can be safely validated.
3. If KeyZapper reports configuration drift, conflicting ownership, unreadable state, or recovery failure, stop. Preserve the KeyZapper-owned recovery artifacts and ask the maintainer/operator to inspect the exact target and backup state. Do not overwrite a third-party value or erase the journal.
4. After a successful repair, refresh and read back the exact target/status. If the connection was removed, reconnect explicitly using the Connect procedure. Start a new Hermes session after any successful write or rollback.

Generic Hermes/launcher errors can mean the installation could not be validated, the configured home could not be read safely, or Hermes itself rejected a command. KeyZapper deliberately avoids displaying raw child output. Refresh once; if the fixed diagnostic persists, retain the executable/home shown in the UI and give them to the maintainer through an approved channel. Never enable writes by changing the fixture, copying a fingerprint, or treating a version string as proof.

For a maintainer's **offline, local diagnostic only**, the existing harness accepts this argument shape (replace labels with absolute paths and use a new child of a trusted scratch directory):

```text
python3 desktop/scripts/test_hermes_contract.py --hermes ABSOLUTE_EXECUTABLE --helper ABSOLUTE_EXECUTABLE --home FRESH_SCRATCH_HOME
```

Run it only inside the approved read-only, network-none runtime with synthetic credentials and an unused scratch home. Never pass a real user home. The harness's accepted fixture is exact-build evidence, not an end-user compatibility switch; a different build remains unsupported unless independently tested and deliberately accepted.

## Workplace acceptance

**Pending.** No permitted Mac package/UI acceptance or workplace gateway/Entra credentials were available for this documentation task. The exact developer-runtime evidence above does not validate a native macOS package, installed-app helper path, or a workplace connection. Do not describe the integration as production-ready or workplace-approved.

Complete the following with an operator on the permitted Mac and approved disposable Hermes profile before workplace use:

- [ ] Build/package on the permitted Mac; install the `.app`; exercise the UI from the installed app, not only a development/cargo-run path. Check English and German, keyboard/focus behavior, and long executable/home paths. Confirm the bundled helper path works and an app update does not silently invalidate its reference.
- [ ] Confirm Hermes capability, approved gateway/model, exact target profile, and write consent with the operator. Keep credential values out of chat and logs.
- [ ] In a disposable Hermes profile, connect using KeyZapper only; read back the exact managed configuration. Confirm cancellation writes nothing. Update and Remove; verify unrelated settings survive and only owned original fields are restored. Exercise recovery/drift safely, then restore the operator's explicitly chosen approved connection and read it back.
- [ ] Using the approved workplace gateway, test a harmless real request, streamed response, and harmless tool call. Record actual outcomes separately; local mocks do not count. For SSO, verify a still-running session after a real token refresh. Do not alter the system clock. Test a key profile only if IT provides one; otherwise mark key-profile coverage unavailable.
- [ ] Record reviewer, app/package identity, exact Hermes runtime identity, selected target and model identifier (no secret values), and pass/fail/blocked evidence in the approved workplace record. Do not claim acceptance while any required check is pending.

### Evidence boundary

The Task 8 native contract passed for the exact developer runtime in the fixture, including actual CLI round-trip/unset, helper parsing/refresh, SDK JSON/SSE/tool-call parsing without tool execution, and a synthetic streamed gateway request. A real workplace model request, real Entra sign-in/refresh, installed macOS app/helper path, and macOS UI checks are **not** established by that result. Task 8's fixture and report record those boundaries; this guide does not broaden them.
