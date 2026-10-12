//! API-v2 source/account admin regression suite.
//!
//! The focused route suites are included here so the combined API contract can be
//! run as `cargo test -p task-api --test llm_routes_admin`.

#[path = "llm_sources.rs"]
mod llm_sources;
#[path = "accounts_admin.rs"]
mod accounts_admin;
