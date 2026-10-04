---
tasks: [01M4267QN3VQ5QDPHX7GMP7S30]
---
# 日次整理 apply 運用文書の修正

## 変更

- `docs/ops/cron-jobs.md` §6.1 の commit 題を実装と同じ `knowledge curation <YYYY-MM-DD>: 統合 n・新規 n・削除 n・修正 n` に修正した。
- commit 対象から `.gitignore` 対象の派生 `index.json` を除外し、追跡対象の変更 path に `_curation/YYYY-MM-DD.md` と `README.md` が含まれることを記載した。
- 検証失敗・元ページ変更・hash 不一致では task を失敗扱いにせず、`Done` の報告に「反映していない」と残す `Reported` / `Stale` の扱いに合わせた。
- upstream を優先し、無ければ `origin` に push すること、event `knowledge_curation_applied` の `push` 値（`pushed` / `no_remote` / `failed` / `skipped`）を記載した。
- §6.3 後に重複していた旧説明を削除し、remote 追加と push 失敗時の手動 push を §6.3 にまとめた。commit 探索は subject に合わせ `git log --grep '^knowledge curation'` とした。
- ADR-0131 の日次整理自動適用付記に、旧方式で `AwaitingApproval` の計画も次 tick の再検証に通れば承認なしで適用する一文を追加した。

## 証拠

- `rg -n 'curation_commit_subject|index.json は派生物|AwaitingApproval|push_remote' crates/task-ops/src/knowledge.rs crates/celeris/src/knowledge_curation.rs` — 題の format、派生 index の除外、push remote の選択を実装で確認。
- `sed -n '640,705p' crates/celeris/src/knowledge_curation.rs && sed -n '987,1068p' crates/celeris/src/knowledge_curation.rs` — 検証失敗時の `Reported` / `Stale` と旧 `AwaitingApproval` の再検証を確認。
- `git diff --check` — exit 0。
- `sh scripts/dev/check-doc-links.sh docs/ops/cron-jobs.md agent-docs/adr/0131-cron-jobs.md` — exit 0 (`check-doc-links: ok`)。
- `sh scripts/dev/check-adr-numbers.sh` — exit 0 (`check-adr-numbers: ok (135 files)`)。
- `git diff --name-only` — `docs/ops/cron-jobs.md` と `agent-docs/adr/0131-cron-jobs.md` のみ（`crates/`・`config/`・`gui/`・`web/` に差分なし）。
