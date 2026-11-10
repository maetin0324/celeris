//! CLI integration tests bundled according to ADR 2026-11-10-test-target-bundles.

#[path = "../common/mod.rs"]
mod common;

#[path = "../../../../tests/support/http_sync.rs"]
mod fake_http;

mod bin_smoke;
mod browser_doctor;
mod browser_ledger_release;
mod campaign_connected;
mod cli_resolution;
mod mcp_client_scopes;
mod no_migrate;
