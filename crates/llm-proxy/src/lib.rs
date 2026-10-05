//! llm-proxy: celeris が張るローカルの OpenAI 互換プロキシ（ADR-0053 D1/D2。Phase 65）。
mod auth;
pub mod config;
pub mod credentials;
pub mod fallback;
pub mod legacy_catalog;
pub mod log;
pub mod naming;
mod neterr;
pub mod openai;
pub mod reservation;
pub mod routing_context;
pub mod selection;
pub mod server;
pub mod shadow;
pub mod shadow_budget;
pub mod sources;
pub mod sources_view;
mod sse;

pub use server::{ProxyState, router, serve};
pub use sources_view::{AccountSourceView, SourceView, SourcesView};
