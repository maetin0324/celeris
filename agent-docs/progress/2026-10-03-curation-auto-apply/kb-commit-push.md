---
task: 01M420EMSFS1VP5RWF2FGCV6XR
work_unit: kb-commit-push
status: done
completed: 2026-10-03
base: b742ec75f067
---

# 日次整理の自動適用: ADR-0131 付記と KB の commit・push 部品

## やったこと

- `agent-docs/adr/0131-cron-jobs.md` 末尾に『付記（2026-10-04、日次整理の自動適用）』を追加。承認なしの適用、
  `curation-human` は自動承認しない、検証失敗・元ページ変更・hash/snapshot 不一致では適用しない、1 commit と
  題の形式、push（NoRemote / Failed は apply を失敗にしない）、救出手順、40 件上限（D12）は不変。
- `crates/task-ops/src/knowledge.rs` 末尾（attempt 2 で別 module から本体へ移した。計画の check が
  `grep 'pub fn commit_curation' crates/task-ops/src/knowledge.rs` で定義の場所を見るため）:
  - `CurationCounts { merged, new, deleted, fixed }`、`curation_commit_subject(date, counts)`。
  - `commit_curation(root, date, counts, task_id, paths) -> Result<String, String>` — 既存 `commit_paths` で
    KB agent 作者の 1 commit。`.gitignore` 対象（`index.json`）と存在しない未追跡 path は除外。変更なしなら HEAD。
  - `push_remote(root) -> PushOutcome`（`NoRemote` / `Pushed { remote, branch }` / `Failed(String)`）— upstream
    の remote か `origin` へ非 force・非対話（`BatchMode=yes`）・`GIT_WRITE_TIMEOUT` 付きで push。
- 試験 `crates/task-ops/src/knowledge/curation_git_tests.rs`（4 件、名前は `curation_commit_` で始まる）。remote は
  tempdir の `git init --bare` のみ（外部ネットワークに出ない）。

## 証拠

- `cargo test -p task-ops --lib curation_commit_` → 4 passed / 0 failed
- `cargo test --workspace` → exit 0、3836 passed / 0 failed / 13 ignored
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0
- attempt 2（移動後）: `cargo test --workspace` → exit 0、3836 passed / 0 failed。`cargo clippy --workspace -- -D warnings` → exit 0。
  計画の check（`grep -q '日次整理の自動適用' …0131-cron-jobs.md && grep -q 'pub fn commit_curation' …knowledge.rs && grep -q 'pub fn push_remote' …knowledge.rs`）→ exit 0
- `cargo fmt --all -- --check` → 差分なし（fmt 適用後）

## 未解決事項

- daemon の apply 経路からの呼び出し（承認撤去・commit・push・event）は WU auto-apply の担当。

## 提案

- なし
