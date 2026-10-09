---
tasks: [01M4F9X90M8BK6RG7KJ2ZQEBB2]
---
# Credential evidence selfdeploy wiring

`sd_browser_ledger` now runs the credential evidence generator after P4-B and before the daemon-equivalent ledger check. It passes `CELERIS_USERNS_TESTS=1`, both P4-B backends, and writes evidence logs under `browser/credential/`. The credential ledger replaces the P4-B ledger only when the generator exits successfully and produces `conformance.json`; otherwise the public ledger remains and `ledger-status.json` records `credential_evidence` (`ok`, `code`, `reason`). A P4-B failure records `p4b_incomplete`. The shared function means `release.sh` and `browser-ledger.sh` use the same sequence. The overall timeout default is now 3600 seconds.

The fake-tool regression coverage in `browser_ledger_credential_evidence.sh` exercises credential success, failure with public-ledger retention and a logged reason, P4-B failure, and the `browser-ledger.sh` path. It delegates to the expanded release-stage fixture.

## Checks

- `bash -n scripts/selfdeploy/lib.sh scripts/selfdeploy/tests/browser_ledger_release_stages.sh scripts/selfdeploy/tests/browser_ledger_credential_evidence.sh` — passed.
- `bash scripts/selfdeploy/tests/browser_ledger_credential_evidence.sh` — passed.
- `bash scripts/selfdeploy/tests/browser_ledger_release_stages.sh` — passed (included in the credential wrapper run).
