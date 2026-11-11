#[path = "../../../../tests/support/http_async.rs"]
mod fake_http;
#[path = "../support/git.rs"]
mod git_support;
#[path = "../support/webhook.rs"]
mod webhook;

mod cos_chat_stagnation_notify;
mod cos_chat_triage_notify;
mod cos_chat_triage_scenarios;
mod deployment_audit;
mod releases_api;
mod stranded_default_branch;
mod stranded_integration;
