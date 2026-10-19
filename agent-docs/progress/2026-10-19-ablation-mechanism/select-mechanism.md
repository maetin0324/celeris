---
task: 01M59KY5FRH6P13NZEH9QMCPBA
work_unit: select-mechanism
status: done
completed: 2026-10-19
---
# 機構アブレーションの選択（select-mechanism）

## 結果
- 選択: 専門分担（`specialization`、比較 ID `M1-specialization`: `M1-spec-on` / `M1-spec-off`）。延期: 内部レビュー・KB 共有。
- 正本: `crates/campaign/fixtures/ablation/selection.json`、canonical JSON（キー整列・compact・UTF-8）の sha256 は同名 `.sha256`（`d73b6e82…1247`）。
- 内部レビューは既存設定で off にできない（ReviewPrefs に off 欄なし、done_when/空 acceptance → `Check::Reviewer` の構造的注入、最終レビューは無条件）ため、実 reviewer run 0 を保証できず延期。
- 専門分担は既存の org（DB、seed は org.toml / `/api/v1/org`）だけで off にでき、`task_ops::matching::decide` が子孫を決定的に同じ node へ、retry は assignee を保持する。

## 証拠
- `cargo nextest run -p task-ops -E 'test(/matching::tests/)'` → exit 0、13 passed
- `cargo nextest run -p task-core -E 'test(/reviewer/)'` → exit 0、7 passed
- `celerisctl show` で既存 3 task（設定確定 Running・無LLM統合 Ready・主比較 pilot Ready）を確認。依存は作っていない。
- `celerisctl knowledge record ... --source task:01M59KY5FRH6P13NZEH9QMCPBA` → `_inbox/20261019T083119Z-celeris-kb.md`（記録した hash `712ec458…` は KB 記録の証拠を足す前の版。差分は development_evidence の 1 項目だけ）

## 未解決事項
- spec-off の org seed（単一 node の profile を spec-on の coding 課と同値にする）の具体化と伝播検査は後続 WU（wire-variant / application-proof）。

## 提案
- 内部レビューの off は、将来やるなら profile か task に明示の欄を置き、gate.rs の Reviewer 注入と最終レビュー起動の両方で参照する配線が要る。
