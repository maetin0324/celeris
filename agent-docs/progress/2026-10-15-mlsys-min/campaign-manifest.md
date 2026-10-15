---
title: "campaign manifest・dry-run・routing-eval 回帰試験"
tasks: [01M4Z2989XX5XK159CCDNV1HYH]
status: done
updated: 2026-10-15
completed: 2026-10-15
---
# campaign-manifest（WU manifest）

ADR: `agent-docs/adr/2026-10-15-mlsys-min-campaign.md`。

## 実装済み
- `crates/campaign`: manifest（検証・正規化・sha256・入力 manifest 照合・split 混在拒否）、dry-run（JSON 見積り）、集計（失敗を分母に残す・hash/split 混在拒否）、CLI `celeris-campaign dry-run|validate --manifest`。
- routing-eval: `manifest_hash` を pub に。`aggregate::build` を dev 問題のみ集計に修正（D6）。`tests/aggregate_identity.rs` 3 件。

## 未検証・未実装
- runner・独立 checker・export・fixture・pilot 手順は後続 WU。
- dry-run の見積り係数は仮置き（実測なし）。実 LLM 測定はしていない。
- routing-eval の collect は DB 側に manifest hash を持たないため、同一 problem id の別 manifest 結果は DB 経路では区別できない（problem id が manifest に無いものは除外される）。campaign 側は `campaign_sha256` を結果に持たせて拒否する。

## 証拠
下の完了報告に記載（test-parallel・clippy の結果）。

## 提案
- 後続 runner は結果行に `campaign_sha256`/`split` を必ず書く（`TrialResult`）。
