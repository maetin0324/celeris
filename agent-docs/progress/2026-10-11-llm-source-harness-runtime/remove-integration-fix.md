---
tasks: [01M4MEEKX485VD5SWKX0P30D4Y]
status: done
title: remove 段の統合後の試験失敗修復
updated: 2026-10-11
---
# remove 段の統合後の試験失敗修復

完了日: 2026-10-11（operator）

remove 段の WU（prune-select・prune-policy・api-unprovider・snapshot-unprovider・catalog-unprovider・
config-drop・hint-drop）は単独では通ったが、統合後の phase check で落ちた。原因はすべて hint-drop
（`worker_hint.adapter` → task / role の `harness`）の取りこぼしで、旧い `adapter` 欄を書く経路・例・試験が
`deny_unknown_fields` に当たっていた。

## 根本原因と修正

| 失敗した試験 | 根本原因 | 修正 |
|---|---|---|
| task-ops `followup::tests::worker_only_fields_are_dropped_and_status_is_always_draft` | 試験が消した `adapter` 欄を NewTaskSpec に入れていた | 試験を `harness` 欄（無視されて進行に残る）へ。`followup.rs` の変数名・モジュール文書も `harness` に |
| celeris `knowledge_gc::tests::*`（4 件） | `knowledge_gc.rs` の task spec が `"adapter":"langmem"` を書いていた（本番経路のバグ。知識 GC の task が作れない） | `"harness":"langmem"`（`knowledge_maint` と同じ。試験は `harness_id == "langmem"` を確認） |
| celeris `doc_gardener::tests::managed_scan_schedules_bounded_review_and_survives_state_loss` | `GardenerConfig.adapter` を task spec の `adapter` に写していた（本番経路のバグ） | `[docs_maintenance]` の欄を `harness` に改名し task の `harness` へ写す。旧い `adapter` 欄は `deny_unknown_fields` で読み込みエラー |
| celeris `config::tests::roles_and_delegation_are_parsed_and_mapped` と、同根で落ちていた `rejects_duplicate_roles_…`・`loads_research_example_config`・`loads_web_research_example_config`・`config_examples_load` | 試験と `config/celeris.{research,web-research}.example.toml` の `[[roles]]` が `adapter =` を書いていた | 例と試験を `harness =`（宣言済み harness id `paperqa` / `local-deep-research`）へ。旧い「未知の role adapter」検査の試験は「`adapter` 欄は読み込みエラー」に置き換え、試験名を `rejects_duplicate_roles_removed_role_adapter_and_zero_limits` に |
| celeris::instance_handoff `the_builtin_smoke_role_and_genre_override_a_config_entry_of_the_same_id` | 設定の role に `adapter = "claude-code"` | 欄を消し、上書きの確認は tier / max_turns で行う。組み込みの role の harness は `smoke` を期待 |
| celeris::instance_handoff `normal_mode_does_not_inject_the_smoke_builtins`（と修正途中に出た `verify_mode_never_dispatches_…`） | hint-drop が組み込み `smoke` role の `adapter = "fake"` を機械的に `harness = "fake"`（adapter id）にし、verify の eligibility も `harness_id == "fake"` を見ていた。verify が宣言する harness は `smoke`（kind `fake`）なので、`fake` を要求した task は経路が無く ready のまま | 組み込み role の harness を `SMOKE_ID`、verify の eligibility を `harness_id == "smoke"` に（`apply_verify_smoke` が `smoke` harness を必ず kind `fake` に上書きするので構造上の保証は同じ）。試験の smoke task も `harness = "smoke"`。`docs/ops/selfdeploy.md` の検査 6 の表（旧 `[[providers]]` 形）を新しい宣言の形に、`verify.sh`・`celeris.example.toml` の注釈も更新 |
| task-dispatch `route_select::harness_select_reads_structured_task_fields_only` | 旧試験は「`worker_hint.adapter` は harness 選択で読まない」を確かめていた。hint-drop がその行を `task.harness_id = "acp"` に置き換えたため、意味が反転して `req.harness == None` が落ちた | `worker_hint.adapter` はもう無いので、その行を削除（本文の文面を読まないことと、`harness_id` を読むことの確認は残る） |
| task-worker `protocol::tests::committed_schema_matches_generated` | hint-drop 後に task-worker の schema を再生成していなかった（`WorkerHint.adapter` が残っていた） | `UPDATE_SCHEMA=1 cargo test -p task-worker committed_schema_matches_generated` で `docs/protocol/worker-protocol.schema.json` を再生成（差分は `WorkerHint.adapter` の 6 行削除のみ） |
| task-worker `browser_launcher::live_tests::browser_launcher_live_frame_cdp_screencast_start_ack_stop_on_fake_cdp` | この branch は `live_tests.rs` に触れていない（merge-base 以降の変更は `browser_launcher_run*.rs` の 1 行ずつだけ）。単独実行・全体 2 回とも通過 | 変更なし（負荷時のタイミング flake と判断） |

## 証拠
- `bash scripts/dev/test-parallel.sh`: exit 0。`4896 tests run: 4896 passed, 13 skipped`。
- `cargo clippy --workspace --all-targets -- -D warnings`: exit 0。
- `cargo test -p task-core -p task-api -p task-worker schema_matches`: exit 0（task-api 1、task-core 8、task-worker 1 passed）。
- `cargo nextest run -p celeris --test instance_handoff`: 8 passed。
- `cargo nextest run -p task-worker -E 'test(screencast_start_ack)'`: 1 passed。

## 本番への移行
- `[docs_maintenance] adapter = "…"` は読み込みエラーになった。書いているなら `harness = "<宣言済み harness id>"` に書き換える。
- `[[roles]] adapter = "…"`（hint-drop で削除済み）も同様に `harness = "<harness id>"` へ。

## 未解決
- なし。

## 提案
- `[[roles]].harness` / `[docs_maintenance].harness` は宣言済み harness id かを config 検証で確かめていない（旧い「未知の adapter」検査は消えた）。誤記は dispatch 時に経路無しで ready のまま残るだけなので、`validate` で `harness_declarations` と組み込み id（`smoke`・`langmem` 等）に照合する検査を足すとよい。
