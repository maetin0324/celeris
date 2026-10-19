---
title: "整合判定と operator live 承認の分離、固定系列台帳による予算迂回の拒否"
tasks: [01M59KY5FPEWVNW0DT07HZRA3Q]
unit: repair-evidence-budget
status: done
updated: 2026-10-19
completed: 2026-10-19
---
# repair-evidence-budget

reviewer 01M5AH3PZV の 2 点を 1 WU で修正（人の指示 2026-10-19。新しい子 task・段・実測なし）。

## 区別（報告用）
- 取り込み元: 失敗子の保存ブランチ `celeris/01M59W8DFMVT3M8WRM4XP8ZEWE`（df14f543）を merge commit 6b92c3e3 で取り込み（manifest・累計予算・検証器・reader/reconcile・per-run 条件・手順書。作り直していない）。
- 本 WU の修正: 上記 merge の後の commit（series.rs、live 表現の除去、live 承認の手順と実在確認）。

## (1) live の判定
- `jev-live-evidence.py`: 合格文を「整合 OK（live かどうかは未判定）」へ。`live_proven`→`chain_ok`（Python・Rust `jev_smoke`/`jev_stages`・試験）、「live 合格」「live 証拠」等の文言を除去。`--require-live` は厳密な被覆検査として維持し、help に「live かどうかは判定しない」と明記。report に `scope` 欄。
- 回帰: `test_live_source_contract.py::ConsistentIsNotLive`（builder の `default_source/export_of/write` だけで作った evidence 外 SQLite を `CELERIS_JEV_SOURCE_DB` に与えると整合 exit 0・live 未判定。出力に live 合格の語なし）。改変・不足の非 0 試験は維持（88 件 → 全 pass）。
- live は operator 承認項目: `docs/ops/jev-c1-live-check.md` §7（`<STATE_DIR>/run/` に launcher.meta・config-identity.json・daemon-image.sha256・state-dir-db.txt・export/・operator-approval.md）。`scripts/dev/jev-live-approval-check.py`（証跡の実在確認、欠ければ非 0）と試験 4 件。
- 証跡は未取得: 実在確認コマンドは非 0（不足一覧は result.json）。live 未確認のまま。

## (2) 承認総枠
- `budget::series_prior_spend` を廃止し `series.rs`（測定 ID ごとの固定台帳・許可 out root・排他 lock）へ。launcher.env の `SERIES_ROOT`/`MEASUREMENT_ID`→campaign lock の `series`→runner。ADR `agent-docs/adr/2026-10-19-jev-series-ledger.md`。
- 試験: `side_budget.rs` 10 件（hash 変更・同 hash で別 out・再開/重複で二重計上なし・root 外/`..`/symlink/root 自身の拒否・台帳未設定/欠損/破損/別 ID の拒否・同時 run 拒否・既存 out の引継ぎ・主比較不変）+ 通常 CLI 経由 1 件 + launcher 経由 1 件（`launcher.rs::prepare_creates_the_pinned_series_ledger_from_launcher_env`）。
- 手順書 §4.1 の「別親では合算されない」を削除し、固定台帳・引継ぎ・拒否条件を記載。

## 本番への移行
副比較の launcher.env に `SERIES_ROOT`（測定専用 dir）と `MEASUREMENT_ID` を足し、`prepare` で台帳を作る。既存 out に消費があれば `SERIES_IMPORT_OUTS` で引き継ぐ。主比較の env は変更不要。本番 config・daemon は触らない。

## 未解決・提案
- live の実証（operator の証跡取得）は未取得。副比較は主比較の依存にしない。
- `final-accounting` への系列合算は未実装（実行前の予算 gate のみ）。
