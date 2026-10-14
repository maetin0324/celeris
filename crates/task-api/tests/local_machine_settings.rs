//! ADR 2026-10-14-local-machine-settings: task の受け入れ条件の名前（`--test local_machine_settings`）で
//! `tests/local_settings_admin.rs` の試験（`GET /clusters` の `local`・`PUT /clusters/local/settings`）を走らせる。

#[path = "local_settings_admin.rs"]
mod local_settings_admin;
