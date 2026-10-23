---
title: main 統合前に、候補 SHA・基底 SHA と必須検査の結果を結び付け、変わったら証跡を失効させる
updated: 2026-10-23
completed: 2026-10-23
tasks: [01M5KWPP519YW28Q6FQRNP5184]
status: done
---
# 必須検査と候補 SHA の対応保証（健全性 5）

ADR: `agent-docs/adr/2026-10-23-required-checks-bound-to-candidate.md`
基準 SHA（着手時の main）: `13d64acd17806d73a1797bffbf831efc9cb5ab6a`

## 棚卸し（統合経路 → 適用される検査 → 証跡の照合箇所）
| 経路 | 必須検査 | 照合箇所 |
|---|---|---|
| atomic / root task → main（delivery） | review の acceptance **＋** `workspace.toml` `check`（今回: acceptance で置換不可） | review が `ReviewTargetSynced`（基底・候補）と `RequiredChecksEvaluated`（結果）を記録 → `celeris/delivery.rs` の `check_candidate`（SHA）＋ `check_required_evidence`（結果）→ `validate_candidate` → `merge --ff-only`。release gate は main 更新後（全体試験はここだけ） |
| WU の run | WU の `checks`（`CELERIS_WU_BASE` 付き） | `dispatcher/work_units.rs`。統合では env 付き check を流さない（ADR 2026-10-22-wu-env-checks-skip-integration） |
| phase 統合（WU → Task ブランチ） | 統合後の tree で非 WU 専用 check ＋ `default_checks` | `phase_integration::integration_checks_for_phase`（試験 `scope_checks.rs`）。基準は env 無しの `git merge-base HEAD main` |
| 親子統合（子 task → 親ブランチ） | 子の最終 review（repo check も必須に） | `integration::integrate` が `MergeCandidate`（ADR-0118 D5）で子ブランチ HEAD を照合、違えば stale |
| 配送の局所修復・IntegrationRepair | 修復後の再 review | 修復後は候補が NULL/別 SHA になり、証跡も無いので `request_rereview` へ戻る |

## 実装
- 必須検査の置換を禁止: `review::review_task` が `workspace.toml` check を常に足す（acceptance の同一 cmd は二重に走らせない）。旧分岐 `task_has_command` は削除。
- `Event::RequiredChecksEvaluated`（task-core）。束縛は既存の `ReviewTargetSynced` の `target_sha`/`reviewed_sha` を再利用（SHA の仕組みは増やしていない）。
- `task-core::required_checks`: `verify_required_checks`（証跡なし・別束縛・不合格・必須 cmd 欠落を拒否）、`reusable_passes`（同一束縛の合格を再利用）。
- review_spawn が再利用集合を渡し、review_verdict が証跡を追記。delivery が ff 前に照合（失敗時は再 review、上限は既存の `MAX_TARGET_RESYNCS`）。
- schema 再生成: `docs/api/v1`・`web/api/generated`。ADR-0043 D4 の記述・`workspace.toml` コメント・`docs/guides/workspace.md` を新規則に更新。

## 受け入れ条件との対応
- 0 棚卸し: 上表。
- 1 拒否の試験: `required_checks::tests`（不在・不合格・欠落・候補/基底/repo 違い・最新優先）、`delivery::tests::merge_requires_passing_evidence_bound_to_the_candidate_and_base`（無し／不合格／別束縛を ff せず Reviewing へ、正常は ff）、`merge_rejects_evidence_missing_a_required_check_of_the_candidate`（acceptance 置換相当）、`review::tests::acceptance_command_does_not_replace_the_repo_checks`、既存 `target_advanced_*`（main の進行・候補の変更）。
- 2 再利用: `review::tests::bound_passes_are_reused_without_running_the_check_again`、`reusable_passes` の試験。reviewed/merge_candidate/target 照合と ff-only は無変更で再利用。
- 3 基準: `scripts/dev/tests/test-affected-list.sh` (d)（CELERIS_WU_BASE なしの統合で commit 済み変更が空 diff にならない）。`scope_checks.rs`（01M5HPMDFY）、Cargo.lock 再生成（01M5GE1BGE）、検査 timeout の孤児（01M5D93NQD）の試験は変更なしで通る（test-affected の対象）。
- 4 全体試験は追加していない。タスク 1 の軽量ゲートは**未接続（タスク 1 側に残す）**: `workspace.toml` の `check` に足せばそのまま必須検査・証跡になるので接続作業は不要（今回 main には未着）。

## 実行コマンドと結果（検証対象: この commit の HEAD）
- `cargo fmt --all -- --check` exit 0 / `cargo clippy --workspace -- -D warnings` exit 0
- `cargo test -p celeris --lib delivery` 37 passed、`cargo test -p task-dispatch --lib review::` 57 passed、`... target_sync` 23 passed、`git_workspace` は新規則に合わせて更新
- `sh scripts/dev/tests/test-affected-list.sh` → ok
- `bash scripts/dev/test-affected.sh`: 1 回目は `celeris::root_done_summary restart_does_not_duplicate` が負荷で 1 回落ち（単独再実行 2 回とも 4 passed、本変更と無関係の既存の時間依存）、2 回目は exit 0（全 crate の nextest ＋ doc test 通過）

### 最終 review の不合格（test-affected exit 100）への対応（WU fix-live-limit-race）
- 原因: `browser_launcher::live_tests::browser_launcher_live_frame_other_session_rejected`（live_tests.rs:687, `ErrorCode::Limit`）の試験の競合。本変更は task-worker に触れていない。拒否された接続（LeaseMismatch・Limit）を client が閉じても、server の接続 thread が `connections`（上限 `max_connections`=4）を減らす前に次の接続を張ると上限超過で Limit になる。live a を開く前（拒否 2 本が残ると a, b と合わせて 4）と raw を張る前（拒否 1 本が残ると 5 本目）の 2 か所に窓があった。
- 修正 dc595f7f: `ServerHandle::wait_connections_at_most`（`#[cfg(test)]`、`conn_exit` Condvar で数えている接続が n 本以下になるまで待つ。30 秒は保険）を足し、試験は live a の前に 2、raw の前に 3 まで戻るのを待つ。sleep・上限・assert は変えていない。production の経路は不変。同じ形（拒否直後に上限近くで新しい接続）の他試験は無い（`live_view_rejects_input` は最大 3 本、`session_and_connection_limits` は Limit を期待する側）。
- `cargo test -p task-worker --lib browser_launcher::` exit 0（54 passed）、`browser_launcher::live_tests::` 5 回連続 10 passed。注: この run の TMPDIR は長く unix socket の SUN_LEN を超えるため、試験は `TMPDIR=/dev/shm/...` で回した（評価器の環境では既存どおり）。
- `bash scripts/dev/test-affected.sh` exit 0（4462 passed / 16 skipped）、`cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0。

## 本番への移行
config・DB の変更なし。昇格時に MergeQueued で止まっている配送は証跡が無いので 1 回だけ再 review になる。

## 残件・提案
- 既に最新 main で解消済みだった項目: SHA の照合・ff-only・phase 統合の基準（ADR-0118、01M5HPMDFY）は既存で、重複実装していない。
- 複数 repo の task で 2 番目以降の repo は定義の網羅を見ない（合否のみ）。repo ごとの定義で review する拡張は別 task。
- タスク 1 の軽量ゲートが入ったら `workspace.toml` `check` に足す（接続はタスク 1 側）。
