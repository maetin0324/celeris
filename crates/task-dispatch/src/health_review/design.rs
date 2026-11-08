//! ADR 2026-11-07-health-review-integration D3.3・D3.5: 計画の採用前の risk 判定と、高 risk（`high`・`unknown`）の
//! 計画だけに既存の reviewer run で設計 review を起こす処理の置き場（後続の design-gate・design-verdict の unit）。
//! risk の判定そのものは `task_core::design_review::assess_risk`（純粋関数）で、review 方針は [`super::policy`] が
//! base 側から読んだものを使う。
