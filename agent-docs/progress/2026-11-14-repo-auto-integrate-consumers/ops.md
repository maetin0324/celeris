---
title: task-ops の取り込み判定を repo の欄にする
tasks: [01M7CFRYZH0AW00M3WAHXX8B57]
status: done
updated: 2026-11-14
---
# task-ops の取り込み判定を repo の欄にする

完了日: 2026-11-14

ADR 2026-11-14-repo-auto-integrate の D3〜D5 のうち task-ops の分。基準は `$CELERIS_WU_BASE`。
この WorkUnit の開始時点で、判定関数・prepare・受信箱・reintegrate・試験の書き換えは base（先行 WorkUnit の統合 commit）に入っていた。
この WorkUnit では内容を確認し、検査を実行し、この進捗ファイルを足した。task-ops のコードの追加変更は無い。

## 確認した内容
- 判定関数は `task_ops::delivery::auto_integrates`（pub）の 1 つだけ。`prepare`（no_project の次で `RepoNotEnabled`）、
  受信箱の `skip_needs_attention`（no_commits・repo_not_enabled は出さず、他は判定が真のときだけ出す）、
  `stranded::reintegrate`（判定が偽なら continue）が同じ関数を使う。
- task-ops に案件単位の判定（`projects.auto_integrate`・`project_not_enabled` の分岐）は残っていない
  （`git grep` で task-ops 内の案件の欄の参照が 0 件）。
- 理由 `project_not_enabled` の読み替えは task-core の `DeliverySkipReason::RepoNotEnabled` の serde alias（1 か所）。試験は
  `task-core/src/store/repo_auto_integrate_tests.rs` の `old_project_not_enabled_reason_reads_as_repo_not_enabled`。
- 表 D5 の 4 行の試験: `delivery/tests.rs` の `auto_integrates_follows_every_repo_of_the_task` ほか
  （1 件・有効 → 判定真、1 件・無効 → `RepoNotEnabled`、2 件・全有効 → `MultipleRepos`、2 件・一部無効 → `RepoNotEnabled`）。
  受信箱は `inbox/tests.rs` で `RepoNotEnabled` を出さないこと、判定が偽なら他の理由も出さないことを確かめる。

## 証拠（提出時の HEAD で実行）
- `cargo nextest run -p task-ops` → 678 passed, 1 skipped
- `cargo clippy -p task-ops --all-targets -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 未解決
- 無し（celeris・task-dispatch・文書は他の WorkUnit の担当）。

## 提案
- 無し。
