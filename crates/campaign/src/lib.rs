//! 最小 campaign 評価基盤（ADR 2026-10-15-mlsys-min-campaign）。
//! 既存の routing-eval manifest/hash/split を再利用し、別のオーケストレータは持たない。
pub mod aggregate;
pub mod dryrun;
pub mod manifest;

pub const CAMPAIGN_SCHEMA: &str = "celeris.campaign-manifest/1";
