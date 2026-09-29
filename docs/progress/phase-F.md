## Phase F0「並列 WU・途中確認・案件計画・quota 指標の設計」（完了日 2026-09-26）

人の決定（2026-09-25）の 4 点と E6 の事実を設計に落とした。成果物は
`docs/adr/0074-parallel-work-units-checkpoints-milestones-quota.md`（Status: Proposed）。コードは変更していない（docs のみ）。

### 設計の要点（D 番号は ADR-0074）

- **D1 WU 並列**: schema `celeris.execution-plan/2` に `phases`。同じ工程の中で依存の無い WU を並列に走らせ、同じ工程の依存は 1 つまで
  （依存先のブランチから積み上げ）。WU ごとの worktree（`<task>/wu/<key>/`、ブランチ `celeris-wu/<task_id>/<key>`。`celeris/<task_id>/…` は
  git の ref の D/F 衝突で作れない）。工程末尾の system WU `integrate-<phase>` が daemon の決定的な `git merge`（冪等）と検査の再実行を行い、
  衝突は repair WU。`running` の鍵を `(TaskId, Option<WU>)`、WU の lease を `work_units` の列（migration 0027）に。Task の lease は工程の
  保持者で WU の lease の最大値まで延ばすので、既存の `reclaim_expired_leases` → `InfraRequeue` は全部死んだときだけ効く。
  並列数は `max_parallel_work_units`（既定 3、上限 6）と既存の provider / アカウント / 全体の枠。Ready の別 Task の 1 本目を優先。
  最終レビューは Task 単位で 1 回のまま。v1 の計画は 1 工程・並列 1 で今と同じ。`[execution] parallel` の既定は false。
- **D2 途中確認**: `PausePolicy`（none / each_phase / after[key か kind]）を人と CoS が書ける（planner は書けない）。停止点の統合の後で
  `Trigger::PhaseGate`（Running → Blocked、reason `awaiting_human`、attempts 不変）。決定的な途中報告（`PhaseReported` と md）。受信箱の
  `AttentionItem::PhaseCheckpoint`、通知は失敗通知と同じ scan の `PhaseCheckpoint`（`QuestionBlocked` からは除外）。操作は
  `POST /tasks/{id}/execution/phase-gate {continue | replan(note 必須) | withdraw(= Cancel)}`。
- **D3 案件計画**: マイルストーン Task = **案件直下という位置**（`project_id` あり・`parent_id` 無し・execute・対話 / support でない）。
  既存の `milestones`（途中目標の判定の台帳。ADR-0033 / 0038 / 0044 D6）と 1:1 に結ぶ。DAG は既存の `depends_on`、依存先は `reached` で Go。
  秘書が `celeris.project-plan/1` を出し、受信箱の `drafts` に 1 まとまりで出して人が承認（HUMAN GATE）。案件 replan は差分
  （dispatch 前のものだけ）で同じ承認。planner の `children` → 既存の委譲で子 Task（WU の中の再帰はしない）。
- **D4 quota**: run の前後の `RateLimitObservation` から `measured / apportioned / estimated / unknown / free` を決定的に推定
  （重み付きトークン W と、実測の比の和による較正）。`Event::QuotaEstimated`、`ExecutionMetrics.quota` / `cost_usd_complete`、
  `GET /metrics/execution` の `accounts_now`。**観測と記録だけで、quota で dispatch や選択を変えない**（予算管理は別プロジェクト）。
- **D5 WU ごとの lane**: planner に WU ごとの `features`（`TaskFeatureHints`）を書かせ、読めなければ検証エラー。lane は既存の
  `decide_for_work_unit`、WU の lane ≤ max(Task の lane, standard)。planner は standard・24 turn・900 秒、計画のサイズ上限、replan は差分
  （`execution-plan-delta/1`）。
- **D6**: repair に `review_timeout`（daemon が 2 倍の timeout で 1 回再実行 → だめなら repair WU）と Task 内部の `merge_base`（決定的な merge →
  衝突なら repair WU）。`RepairScheduled` で class を残す（`unknown` を無くす）。成果物は daemon の run 後の `artifacts_dir` 走査で登録。

### F1 の受け入れ条件（ADR-0074 §7 F1）

- (a) planner プロンプトが WU ごとの `features` を求める（スナップショット）。
- (b) `TaskFeatureHints` として読めない `features` は計画の検証エラー（1 回再試行 → atomic）。保存済みの v1 計画は読める。
- (c) 機械的な WU（judgment/ambiguity low、verifiability/reversibility high、`checks` あり）が cheap。features 無しは Task の lane。
  Task が standard なら WU の frontier は standard に丸め、`clamped_by` を残す。
- (d) planner run は `standard`（`planner/system-standard`）、24 turn / 900 秒。人の明示 frontier の Task では frontier。
- (e) 計画のサイズ上限の超過は拒否 → 再試行 → atomic。
- (f) replan の差分出力が旧版に当てられ、done の WU を書かずに v(n+1) が採用される。全体形式も受け付ける。差分の件数を reason に残す。
- (g) `command timed out after` の不合格は 2 倍の timeout で 1 回再実行、通れば attempts 不変。通らなければ `review_timeout` の repair WU。
- (h) `merge-base --is-ancestor` の不合格は衝突なしなら決定的な merge → 再レビュー、衝突ありなら `merge_base` の repair WU。
- (i) `RepairScheduled` が残り、`repairs_by_class` に `unknown` が出ない。
- (j) git worktree の Task の `artifacts/report.md` が `ArtifactProduced{declared:false}` になり `GET /tasks/{id}/artifacts` に出る。
- (k) reviewer run の `runs` 索引の欠落（E6 報告 問題 3）が直る。
- 共通: `cargo fmt --all -- --check`、`cargo test --workspace`、`cargo clippy --workspace --all-targets -- -D warnings`。外部ネットワークに出ない。

### 実行したコマンド（F0）

- 読んだもの: `CLAUDE.md`、`docs/SPEC.md`、ADR-0072（全体）、E6 報告、ADR-0069 / 0053 / 0046 / 0043 / 0033 / 0038 / 0016 / 0067、PROGRESS 末尾、
  `crates/task-core/src/{execution_plan.rs, execution_gate.rs, model_policy.rs, execution_metrics.rs, accounts.rs, org.rs}`、
  `crates/task-dispatch/src/{execution_scheduler.rs, dispatcher.rs, undeclared_artifacts.rs, review.rs}`、`crates/task-worker/src/claude_code.rs`
  （`build_execution_plan_prompt`）、`crates/llm-proxy/src/{selection.rs, sources_view.rs}`、`crates/task-ops/src/inbox.rs`、`crates/celeris/src/notify.rs`、
  `gui/app/components/ExecutionSection.tsx`。
- コードは変えていない（docs のみ。差分は `docs/PROGRESS.md` と ADR-0074 だけ）。
- `cargo clippy --workspace -- -D warnings`: **exit 101**。既存コードの `clippy::result_large_err` 2 件
  （`crates/task-worker/src/codex_account.rs:91`、`:130`。`AccountCheck` の Err が 128 バイト超）。NFS 移行で rustup を再構築した
  toolchain（cargo 1.98.1）の clippy が新しく出すもので、本 Phase の差分とは無関係。
- `cargo test --workspace`: NFS 移行後の空の cargo registry に依存を取り直すのに時間がかかり、commit の時点で終わっていない。
  結果は F1 の開始時に取り直す。

### 設計中に確かめた事実（ADR に file:line で記録）

- `WorkUnitSpec.features` は既に型と読み手があり（`execution_plan.rs:118-120`、`model_policy.rs:728-745`）、planner の JSON 例に無いこと
  （`claude_code.rs:747-758`）と読めない値を黙って捨てることが E6-1 の原因。F1 は配線ではなくプロンプトと検証の変更で済む。
- 途中目標（`milestones`）と ADR-0038 の判定・`MilestoneReady` の通知が既にあり、直列の鎖として動いている。案件計画は新しい実体を作らず
  これを DAG に広げる形にした。

### 未解決事項

- ADR-0074 §8 の U-F1〜U-F9（同じ工程の同じファイルの編集、共有 `CARGO_TARGET_DIR` のロック、アカウントの人の手での利用による
  measured の偏り、quota の重みの仮定、codex の rate limit の頻度、CoS の案件直下の Task を draft にする影響、`auto_advance` の置き場、
  途中報告の秘書の要約、`review_timeout` の再実行の無駄）。
- ADR-0072 §7 の U1・U2・U7・U8・U9・U10 は本 ADR では扱わない。

### 提案

- F1 の最初に上記の clippy `result_large_err`（codex_account.rs。`Box` 化）を直し、`cargo test --workspace` の基準値を取り直す。
- F1 → F2 → F3（quota は F1 の後すぐ、途中確認は F2 の schema の後に並行）→ F4 → F5（F1 の後に 1 回、F4 の後に 1 回）。
- F5a を流すまで本番の `[execution] gate` は `shadow` のまま（E6 分析の推奨どおり）。`parallel` の既定は F5b の結果を見て人が決める。

## Phase F1「WU ごとの lane、planner の lane とサイズ、replan の差分、repair の分類、成果物の登録」（完了日 2026-09-26）

ADR-0074 §6 F1 の受け入れ条件 (a)〜(k) を実装した。branch `worktree-agent-abdca7309a6944c0a`。
commit: `ad165d3`（環境修正、着手前に既に他コミットで解消済みだったことを確認）→
`e4e922f`（(a)-(f)）→ `7b3821c`（(g)-(k)）→ 本コミット（テスト追補・ADR/PROGRESS 更新・schema 再生成）。
ADR-0074 の Status を `Proposed` → `Accepted` に変更した。

### 環境（着手前の確認）

- NFS 移行後の toolchain（cargo/rustc 1.98.1）で `crates/task-worker/src/codex_account.rs` に出ていた
  `clippy::result_large_err` は、着手時点で既に別コミット `ad165d3`（`rpc`/`read_account_limits` への
  `#[allow(clippy::result_large_err)]` + 理由コメント）で解消済みだった。
  `cargo clippy --workspace --all-targets -- -D warnings` は着手直後から warning 0 を確認できたため、
  本 Phase での追加修正は無し。

### 受け入れ条件ごとの証拠

**(a) planner プロンプトに WU ごとの `features` の説明と例**
- 実装: `crates/task-worker/src/claude_code.rs` の `work_unit_features_section`（5 軸の定義・例、
  JSON 例に `"features"` 欄を追加）。`build_execution_plan_prompt` から呼ぶ。
- コマンド: `cargo test -p task-worker --lib claude_code::tests::execution_plan_prompt_asks_for_per_unit_features claude_code::tests::build_prompt_selects_the_execution_plan_prompt_when_execution_planner_is_present`
- 結果: 2 passed; 0 failed（5 軸すべての名前・`"features"` 欄・「Do not write \`lane\`」の文言をスナップショットで確認）。

**(b) `features` が `TaskFeatureHints` として読めない計画は検証エラー（1 回再試行 → atomic）。v1 の保存済み計画は読める**
- 実装: `TaskFeatureHints` に `#[serde(deny_unknown_fields)]` を追加。
  `execution_plan::validate` に `PlanValidationError::InvalidFeatures` を追加し、`WorkUnitSpec.features`
  が `Some` なら `TaskFeatureHints` として parse できるか検査する。
  `dispatcher.rs::on_planner_finished` は既存の `give_up_or_retry_planner`（1 回再試行 → atomic）を
  そのまま通る。読み取り時（`decide_for_work_unit`）は今までどおり `.ok()` で寛容に読む（変更なし）。
- コマンド:
  - `cargo test -p task-core --lib execution_plan::tests::features_must_parse_as_task_feature_hints`
    → 1 passed（部分的な features は valid、未知の欄・型違反は `InvalidFeatures`）。
  - `cargo test -p task-dispatch --lib dispatcher::tests::plan_with_unreadable_features_retries_once_then_falls_back_to_atomic`
    → 1 passed（`features.lane` を書いた出力が 1 回再試行の後 atomic に倒れ、planner run が
    ちょうど 2 回であることを確認）。

**(c) 機械的な WU（judgment/ambiguity=low、verifiability/reversibility=high、`checks` あり）が cheap。features 無しは Task の lane。Task が standard なら WU の frontier は standard に丸まり `clamped_by` を残す**
- 実装: `model_policy::decide_for_work_unit` に `task_lane: Option<Tier>` 引数と `WorkUnitLaneCap`
  （`[execution] work_unit_lane_cap = "task" | "none"`、既定 `task`）を追加。
  上限 = `max(task_lane, standard)`、下げる方向（cheap）は制限しない。
- コマンド: `cargo test -p task-core --lib model_policy::tests::work_unit_features_lower_a_mechanical_unit_to_cheap model_policy::tests::work_unit_lane_is_capped_by_the_task_lane model_policy::tests::work_unit_lane_cap_does_not_prevent_routing_cheaper_than_the_task model_policy::tests::work_unit_lane_cap_follows_a_frontier_task model_policy::tests::work_unit_without_features_inherits_the_task_hints model_policy::tests::partial_work_unit_features_are_noted_as_partial`
- 結果: 6 passed; 0 failed。

**(d) planner run が standard（`rule_id = planner/system-standard`）、`max_turns` 24 / `max_wall_secs` 900、`[execution.planner] tier` で変更可。人の明示 `tier:frontier` では frontier**
- 実装: `task_core::PlannerConfig` に `tier: Tier`（既定 `Standard`）を追加、既定 `max_turns` 40→24、
  `max_wall_secs` 1200→900。`crates/celeris/src/config.rs` の `[execution.planner] tier`（TOML の
  `Tier` 型そのまま、`"standard"/"frontier"/"cheap"`）。`dispatcher.rs` の planner dispatch 分岐で
  `task.routing.tier_source == Human && task.worker_hint.tier == Frontier` のときだけ frontier を通す
  （`rule_id = planner/human-frontier`）。
- コマンド: `cargo test -p task-dispatch --lib dispatcher::tests::planner_run_uses_the_standard_lane_by_default dispatcher::tests::planner_run_uses_frontier_when_the_task_explicitly_sets_it`
- 結果: 2 passed; 0 failed。

**(e) 計画のサイズ上限を超えた出力が拒否 → 再試行 → atomic**
- 実装: `ExecutionLimits` に `max_rationale_chars`(1500) / `max_title_chars`(120) /
  `max_objective_chars`(2000) / `max_done_when_items`(8) / `max_done_when_chars`(300) /
  `max_checks`(6) / `max_plan_json_bytes`(24 KiB) を追加。`validate` で全項目を検査。
- コマンド:
  - `cargo test -p task-core --lib execution_plan::tests::plan_size_limits_reject_oversized_rationale execution_plan::tests::plan_size_limits_reject_oversized_work_unit_fields execution_plan::tests::plan_size_limits_reject_the_whole_json_being_too_large`
    → 3 passed。
  - `cargo test -p task-dispatch --lib dispatcher::tests::oversized_plan_retries_once_then_falls_back_to_atomic`
    → 1 passed（`rationale` 1,501 文字の出力が 1 回再試行の後 atomic に倒れる）。

**(f) replan の差分出力（add/modify/remove）が旧版に当てられ、done の WU を書かずに v(n+1) が採用される。全体形式も受け付ける。`ExecutionPlanned.reason` に差分の件数**
- 実装: `task_core::execution_plan` に `celeris.execution-plan-delta/1`（`WorkUnitPatch`・
  `ExecutionPlanDelta`・`apply_delta`）を新設。`docs/protocol/execution-plan-delta.schema.json` を
  新規生成。`dispatcher.rs::parse_planner_output` が出力の `schema` を見て、delta なら
  `active` な計画の `base_version` と突き合わせて `apply_delta` で全体に展開し、それ以外は今までどおり
  全体形式として読む（後方互換）。`task_ops::execution::replan` が `reason` の後ろに
  `(added=N, changed=N, removed=N)` を決定的に追記。replan のプロンプト
  （`replan_context_section`）は差分形式を優先するよう文面を変えた（全体形式でもよいと明記）。
- コマンド:
  - `cargo test -p task-core --lib execution_plan::tests::apply_delta_adds_modifies_and_removes_without_restating_untouched_units execution_plan::tests::apply_delta_rejects_modifying_an_unknown_or_removed_key execution_plan::tests::apply_delta_rejects_adding_a_key_that_still_exists execution_plan::tests::delta_committed_schema_matches_generated`
    → 4 passed。
  - `cargo test -p task-dispatch --lib dispatcher::tests::replan_delta_carries_done_units_without_restating_them`
    → 1 passed（差分だけの出力から v2 が採用され、`reason` に `added=0, changed=2, removed=0` が
    残ることを確認。`changed=2` は `b` の spec 変更 + `c` の `blocked→pending` 遷移。既存の
    `a_failed_work_unit_triggers_a_replan_instead_of_failing_the_task` は全体形式のまま）。
  - `cargo test -p task-ops --lib execution::tests` → 11 passed（既存の replan テスト、
    `reason` の diff 件数付与を含め全 pass）。

**(g) `command timed out after` の不合格で daemon が 2 倍の timeout で 1 回再実行、通れば attempts 不変で合格へ。通らなければ `review_timeout` の repair WU**
- 実装: `crates/task-dispatch/src/review.rs` に `exec_check_with_repair_retries`（`review_task` の
  `Check::Command`・workspace.toml check・`run_work_unit_checks` の 3 箇所で共通化。ADR が指す
  `review.rs:295, 407, 531` の 3 箇所に一致）。timeout なら 2 倍（上限 1,800 秒）で 1 回だけ再実行し、
  結果を差し替える。`execution::RepairClass::ReviewTimeout`（`review_timeout`、max_turns 20、
  wall 1200）を追加、`classify_command` が `reason` が `command timed out after` で始まれば
  最優先でこの class にする。
- コマンド:
  - `cargo test -p task-dispatch --lib review::tests::review_timeout_reruns_once_with_double_timeout_before_repair review::tests::review_timeout_still_times_out_after_the_retry_keeps_the_timeout_reason review::tests::review_timeout_retry_is_capped_at_1800_seconds`
    → 3 passed（偽の `Workspace` で timeout→合格、timeout→timeout（`review_timeout` に分類）、
    上限 1,800 秒で再試行しないことを確認）。
  - `cargo test -p task-dispatch --lib dispatcher::tests::command_timeout_check_reruns_once_with_double_timeout_and_passes`
    → 1 passed（実際の `sleep`/マーカーファイルで daemon の tick を通した end-to-end。
    attempts 不変・repair WU 無し・最終 `ReviewVerdict` が合格であることを確認。実時間で約 1 秒）。

**(h) `merge-base --is-ancestor` の不合格で、衝突なしなら daemon の merge → 再レビュー、衝突ありなら `merge_base` の repair WU**
- 実装: 同じ `exec_check_with_repair_retries` に `merge_base_ancestor_ref`（cmd から `<ref>` を
  字句解析）と `git merge --no-edit <ref>` の 1 回だけの試行を追加。衝突なければ再実行して判定を
  差し替え、衝突（または merge 自体の失敗）なら `git merge --abort` して元の不合格のまま返す。
  `classify_command` が `merge-base` と `--is-ancestor` を両方含む cmd を `RepairClass::MergeBase`
  にする（配送の `[delivery-repair]` は変更していない、別経路のまま）。
- コマンド:
  - `cargo test -p task-dispatch --lib review::tests::merge_base_failure_merges_base_deterministically review::tests::merge_base_conflict_aborts_the_merge_and_keeps_the_failure`
    → 2 passed（偽の `Workspace` で衝突なし→合格、衝突あり→`git merge --abort`→
    `RepairClass::MergeBase` に分類されることを確認）。
  - `cargo test -p task-dispatch --lib dispatcher::tests::merge_base_check_merges_main_deterministically_when_there_is_no_conflict`
    → 1 passed（実際の git リポジトリで、worker が別ファイルを変更している間に `main` が
    別ファイルへ 1 コミット先行 → 最終レビューの `merge-base --is-ancestor main HEAD` が
    daemon の実 `git merge` で直り、repair WU も `RepairScheduled` も出ずに Task が `done` に
    なることを確認）。

**(i) `Event::RepairScheduled{class, …}` が残り、`repairs_by_class` に `unknown` が出ない。planner の repair は `planner`**
- 実装: `Event::RepairScheduled { work_unit_id, key, class, origin }` を新設
  （`execution::RepairOrigin { Review, Integration, Delivery, Planner }`。F1 では `Review`
  （`try_review_repair`）と `Planner`（`task_ops::execution::replan` が `kind = repair` の新規 WU に
  発行）だけを実際に使う）。`execution_metrics::summarize` が `RepairScheduled` を優先して
  `repairs_by_class` を組み立て、無ければ従来の title 接頭辞へフォールバックする。
- コマンド:
  - `cargo test -p task-core --lib execution_metrics::tests::repair_scheduled_event_names_the_class execution_metrics::tests::planner_authored_repair_work_units_are_classified_as_planner execution_metrics::tests::repairs_without_a_recoverable_title_fall_back_to_unknown`
    → 3 passed（新しい経路は `unknown` にならない、旧データは今までどおりフォールバックすることを
    確認）。
  - `cargo test -p task-ops --lib execution::tests::replan_records_repair_scheduled_for_a_planner_authored_repair_unit`
    → 1 passed。
  - `cargo test -p task-dispatch --lib dispatcher::tests::a_format_only_review_failure_is_repaired_without_consuming_attempts`
    → 1 passed（既存の E4 のテストに `RepairScheduled{class:"format", origin:Review}` の確認と
    `execution_metrics::summarize` を通した `repairs_by_class.get("format") == 1`、
    `unknown` が無いことのアサーションを追加）。

**(j) git worktree の Task で run 後に `artifacts/` を走査し、`report.md` 等を `ArtifactProduced{declared:false}` として登録。`GET /tasks/{id}/artifacts` に出る**
- 実装: `undeclared_artifacts::scan_undeclared_artifacts_in_dir`（git worktree の Task 専用。
  リポジトリ全体ではなく run の `artifacts_dir` の中だけを、人が読む拡張子
  （md/html/pdf/csv/png）で走査。`checkpoint.json`/`result.json`/`request.json`/`prompt.txt`/
  `execution-plan*.json`/`project-plan*.json`/`phase-reports/` は除外）。`dispatcher.rs::run_worker`
  の既存フック（ADR-0067 D3）を `worktree.is_some()` のときこちらに分岐。
  `crates/task-api/src/files.rs::artifact_views` は `Event::ArtifactProduced` を宣言の有無に関わらず
  一律で読むため、API 側の変更は不要（確認のみ）。
- コマンド:
  - `cargo test -p task-dispatch --lib undeclared_artifacts::tests::git_worktree_task_registers_report_md_as_an_artifact undeclared_artifacts::tests::git_worktree_task_registers_other_human_readable_extensions undeclared_artifacts::tests::git_worktree_task_skips_already_declared_paths`
    → 3 passed。
  - `cargo test -p task-dispatch --lib dispatcher::tests::git_worktree_task_registers_report_md_as_an_artifact`
    → 1 passed（実際の git worktree を使った daemon の tick を通した end-to-end。
    `artifacts/report.md` が `ArtifactProduced{declared:false, kind:"md"}` として events に残ることを
    確認）。

**(k) reviewer run の `runs` 索引の欠落（`finished_at`/`usage`/`end`）を直す。`replay --check` の fixture**
- 実装: `finish_reviewer_run_index` が `usage` を常に `None` にしていたバグを修正（引数で受け取り
  そのまま `store.run_index_finish` に渡す）。加えて調査の過程で、reviewer run の
  `Event::WorkerFinished.end` も常に `None` だったため `task_ops::replay::rebuild_work_units_and_runs`
  が `status` を常に `harness_error` に誤復元することを発見し（ADR の (k) が `end` も挙げていたため
  本 Phase の範囲内として）、正常完了・実質的な不合格・供給側/インフラ都合の deferred・discarded の
  4 経路それぞれで `RunIndexStatus` と同じ `RunEnd` を event にも書き戻すよう修正した。
- コマンド: `cargo test -p task-dispatch --lib dispatcher::tests::reviewer_run_usage_is_recorded_in_the_runs_index_and_survives_replay_check`
- 結果: 1 passed（`usage`/`finished_at` が `runs` 索引に残ることを確認したうえで、
  `task_ops::replay::rebuild_work_units_and_runs` + `diff_execution`（`celerisctl replay --check` と
  同じ突き合わせ）で reviewer run の再構築結果が store の確定値と食い違わないことを確認。
  fixture は reviewer run のみに絞って比較している。理由は下記「見つけたが直していない別の不具合」）。

### schema 再生成

- `UPDATE_SCHEMA=1 cargo test -p task-core -p task-api -p task-worker --lib schema` → 全 pass。
  差分: `docs/api/v1/event.schema.json`（`TaskFeatureHints.additionalProperties: false`、
  `Event::RepairScheduled` の定義）、`docs/api/v1/api-v1.schema.json`（同じ理由）、
  `docs/protocol/worker-protocol.schema.json`（TaskFeatureHints の変更の波及）。新規:
  `docs/protocol/execution-plan-delta.schema.json`。`docs/protocol/execution-plan.schema.json` は
  変更なし（`ExecutionPlanSpec` 自体の構造は変えていない）。

### ADR-0074 からの逸脱・明確化（詳細は ADR 本文の「Phase F1 実装時の逸脱・明確化」節）

1. `TaskFeatureHints` に `deny_unknown_fields` を追加（ADR は「読めなければエラー」とだけ書いていたが、
   型自体が寛容だと `{"lane":"frontier"}` を「空の hints」として読めてしまうため）。
2. D5.2 の「Task の lane」は、エスカレーション前の `decide_for_task` の結果を使う。
3. replan プロンプトを差分形式優先に変更（全体形式も引き続き受け付ける）。
4. `WorkUnitPatch` は欄を「消す」ことができない（`Option<T>` の限界。空に戻したいなら
   `remove` + `add`）。
5. Task 内部の merge は `--no-ff` を付けない（F2 の工程統合とは別の判断）。
6. `RepairOrigin::Delivery`/`Integration` は型だけ用意し、F1 では発行しない
   （`crates/celeris/src/delivery.rs` は ADR §6 F1 の対象ファイルに無く、今回変更していない）。
7. (k) の調査中に見つけた別の不具合（`dispatcher.rs` の暗黙 WU worker run の `runs` 索引 `seq` が
   `current_run_seq` の契約と食い違って +1 大きい。ADR-0072 Phase E2b 由来）は**本 Phase では
   直していない**。ADR-0074 §6 F1 の受け入れ条件・テスト表に無いスコープ外の欠陥のため。

### 未解決事項・F2/F3 への申し送り

- 上記「逸脱 7」（暗黙 WU の worker run の `seq` が実際より 1 大きい不具合）を次の一手として直す
  ことを提案する。`crates/task-dispatch/src/dispatcher.rs` の
  `current_run_seq(&self.store.events_for(task.id)?) + 1`（`run_index_start` の直前、
  `WorkerStarted` 追記の**あと**に呼んでいる箇所）から `+ 1` を外す（`current_run_seq` の他の
  2 箇所の呼び出しと同じ契約に揃える）。`celerisctl replay --check` を本番の適当なタスクに対して
  実行すれば再現できるはず。
- `RepairOrigin::Delivery` への `RepairScheduled` の発行は F2/F3 の課題として残す
  （配送の repair WU は今のところ title 接頭辞の規約で `unknown` を回避できているため緊急ではない）。
- `WorkUnitPatch` で欄を「消す」操作ができない制約（U 節候補）。F2/F3 で困る場面が出れば
  `serde_with::rust::double_option` の導入を検討する。
- F2 の WU 並列実装では、D1.4 の統合後の検査の repair 分類にも本 Phase の
  `classify_command`（`review_timeout`/`merge_base` を含む）と `RepairScheduled`
  （`origin: Integration`）をそのまま再利用できる設計にしてある。
- F2 の execution-plan/2（`phases`）が入ったら、`work_unit_features_section` の JSON 例と
  `parse_planner_output` の delta スキーマ判定はそのまま両立する（`ExecutionPlanSpec` の
  schema 判定と独立している）。

## Phase F3（quota）「quota 消費の推定と記録」（完了日 2026-09-26）

ADR-0074 D4・§6 F3 の quota 側（(g)〜(k)）を実装した。**途中確認（(a)〜(f)）は対象外**（別の担当）。
branch `worktree-agent-aecde37d6476fb05a`。commit:
`36ee4be`（(g)-(h) 推定式と `Event::QuotaEstimated`）→
`da95565`（dispatcher 配線・(i) metrics/API・(k) 不変性テスト）→
`2924e1a`（(j) GUI）→ 本コミット（ADR/PROGRESS 更新）。

着手前に main（Phase F1 統合済み）を worktree の branch へ fast-forward merge した
（着手時点の branch が Phase F1 の統合より前から分岐していたため。ADR-0074 D4.1/§6 F3 が指す
`sources_view.rs`/`accounts.rs` 等が最新であることを確認してから着手）。

### 環境: shared `CARGO_TARGET_DIR` の衝突

`~/.cargo/config.toml` の `[build] target-dir` はこのマシンの全 worktree で共有される単一の
ディレクトリ（`/var/lib/celeris/build-cache/cargo/agent-platform-dev`）を指しており、並行して
別の worktree（F2 担当と思われる）が同じ target-dir へ `cargo test --workspace` を走らせていたため、
`task-dispatch` のビルドが別 worktree の `task-core`（`WorkUnitSpec.phase` 等、F2 の未着手フィールドを
含む版）と不整合を起こし、`missing field 'phase'` のエラーが再現性を持って発生した
（コード自体に問題は無い。同じソースを再ビルドすると解消する類の一時的な事象ではなく、
2 回連続で同じ失敗が出たことで確認した）。**対処**: 本 Phase の作業中だけ
`CARGO_TARGET_DIR=/var/lib/celeris/build-cache/cargo/agent-platform-f3-quota`
（同じローカル LVM 配下、`/tmp` ではない）を明示して cargo を呼んだ。ADR-0066 の
「repo-key ごとに target-dir を分ける」という発想を、並行するエージェント worktree にも
一時的に適用した形。恒久対応（複数 worktree が同時に動く前提で `~/.cargo/config.toml` の
target-dir を repo-key 化する等）は本 Phase の範囲外として提案に残す。

### (g) run の前後の観測から `Event::QuotaEstimated` を決定的に出す

- 実装: 新規 `crates/task-core/src/quota.rs`。`decide_window`（`measured_used_pct` →
  `apportioned_used_pct` → `estimated_used_pct` → unknown の優先順位。`free_window` は呼び出し側が
  供給元の種別で先に判定）、`weighted_tokens`（`W = in + 0.1·cache_read + 1.25·cache_creation +
  r_out·output`）、`calibrate`（`k = Σq / ΣW`）。`Event::QuotaEstimated` を `model.rs` に追加。
- コマンド: `cargo test -p task-core --lib quota::`
- 結果: **21 passed; 0 failed**。表のテストの主なもの:
  `measured_when_exclusive_and_fresh`、`apportioned_by_weighted_tokens_when_runs_overlap`
  （按分の合計が Δ と一致することも確認）、`estimated_uses_ratio_of_sums_calibration`、
  `unknown_is_never_zero`、`priority_order_prefers_measured_over_apportioned_over_estimated_over_unknown`
  （5 通りの優先順位を 1 つの表で確認）。

### (h) `before` 観測の有効性

- 実装: `quota::before_is_valid(observed_at, now, other_consumption_since)`
  （観測が未来なら常に無効、消費が無ければ古くても有効）。「他の消費」の判定自体は
  `crates/task-dispatch/src/accounts.rs::QuotaActivity`（run の重なりを追跡する状態機械。
  ADR からの逸脱 2 参照）が担う。
- コマンド:
  - `cargo test -p task-core --lib quota::tests::before_is_valid_table` → 1 passed。
  - `cargo test -p task-dispatch --lib accounts::quota_activity_tests:: accounts::quota_calibration_tests::`
    → **9 passed; 0 failed**（単独 run は排他、重なった 2 run はグループを作り最後の run で閉じる、
    グループが閉じた後の新しい run は改めて排他になる、別アカウントはグループを共有しない、
    `begin` を呼んでいない run の `end` は観測なしの排他として安全に扱う、較正のリングバッファが
    件数の上限で古いものを捨てる、等）。

### (i) `ExecutionMetrics.quota`・`cost_usd_complete`・`GET /metrics/execution` の `quota`/`accounts_now`

- 実装:
  - `crates/task-core/src/execution_metrics.rs`: `ExecutionMetrics` に `quota: Vec<QuotaUse>`・
    `quota_unknown_runs: u32`・`cost_usd_complete: bool` を追加。`summarize` は
    `Event::QuotaEstimated` を run_id ごとに「最後の Event が有効」で畳み込んでから集計する
    （apportioned の再送を正しく扱う）。`cost_usd_complete` は token を持つのに `cost_usd` が
    無い run（単価不明のモデル）が 1 件でもあれば `false`。
  - `crates/task-core/src/store.rs`: `ExecutionMetricsTaskRow.has_quota_events`
    を追加（ADR からの逸脱 7）。
  - `crates/task-api/src/stats.rs`: `execution_metrics_summary`/`_from_events` がグループごとに
    `quota_rows`/`cost_usd_complete` を集め、`task_core::merge_quota_use` で合計する。
  - `crates/task-api/src/types.rs`: `ExecutionMetricsGroup.{quota, cost_usd_complete}`、
    `ExecutionMetricsSummary.accounts_now`（新規 `AccountNowView`）、`WorkUnitView.quota`、
    `ExecutionPlanView::with_quota`。
  - `crates/task-api/src/execution.rs`: `GET /metrics/execution` が `LlmSourcesReader`（設定されて
    いれば）から `accounts_now` を埋める。`GET /tasks/{id}/execution` が
    `task_core::group_quota_by_work_unit` で WU ごとの quota を差し込む。
- コマンド:
  - `cargo test -p task-core --lib execution_metrics::` → **18 passed; 0 failed**（
    `cost_usd_complete_is_false_with_an_unpriced_model`、
    `quota_is_aggregated_by_source_account_and_window`、
    `quota_estimated_reemission_for_the_same_run_id_keeps_only_the_last_event`、
    `quota_unknown_runs_counts_runs_that_never_resolved`、
    `group_quota_by_work_unit_splits_atomic_and_work_unit_runs` を含む）。
  - `cargo test -p task-core --lib quota::tests::merge_quota_use_` → 2 passed。
  - `cargo test -p task-api --no-fail-fast` → **353 passed; 0 failed**（既存の `stats`/`execution` のテストが
    型追加後も通ることを確認。この Phase では stats.rs/execution.rs に専用の新規テストは
    追加していない — 集計ロジック自体は `task_core::{summarize_execution_metrics, merge_quota_use}`
    の純粋関数に委ね、task-api 側は配線のみのため）。

### (j) GUI: quota が主、定価 USD は参考

- 実装: `gui/app/lib/task-execution.ts::{quotaSummaryLines, costReferenceLabel}`
  （unknown は「不明」であって `0` ではない。`cost_usd_complete === false` なら「一部のモデルの
  単価が不明なため過小」を明示）。`gui/app/components/ExecutionSection.tsx` の「実行」節に
  quota 消費のブロックを追加（gate の下、WU 表の上。**WU 表の列は変更していない**、F2 の担当）。
- コマンド・結果:
  - `pnpm gen:types && git diff --exit-code app/celeris/types.ts` → 1 回目は差分あり
    （`QuotaUse`/`QuotaWindow`/`QuotaWindowUse`/`AccountNowView`/`ExecutionMetrics.quota` 等を反映）、
    コミット後に**再実行して差分ゼロを確認**。
  - `pnpm typecheck` → exit 0。
  - `pnpm lint` → exit 0（info 2 件、いずれも本 Phase と無関係な既存スクリプトの提案）。
  - `pnpm test` → **1096 passed（72 files）**（`test-execution.test.ts` に quota/cost の表の
    テストを追加）。
  - `pnpm build` → exit 0。
  - `pnpm exec playwright install chromium` の後 `pnpm mobile-audit` →
    `{"ok": true, "total": 0, "by_rule": {}}`、`routes=27 schemes=2 violations=0`
    （`task-overview`/`task-artifacts` 等、Execution 節を含む画面を含む）。

### (k) quota の値で dispatch・lane・アカウント選択が変わらないこと

- 実装: `Dispatcher` に `quota_activity`/`quota_calibration` を追加。run の spawn で
  `quota_begin`（before 観測の記録。`is_planner_dispatch` は対象外）、`finish_worker_result` で
  `resolve_quota_estimate`（after 観測 + 5 通りの決定 + `Event::QuotaEstimated` を
  `WorkerFinished` と同じトランザクションで記録。apportioned でグループが閉じたら他タスクへ
  `store.append_event` で直接書く）。永続化しない早期 return（unknown task・stale な結果の破棄）
  でも `release_quota_if_tracked`（`is_tracked` で `quota_begin` を呼んでいない run — planner run —
  を誤って巻き込まない）で bookkeeping だけ閉じる。
- コマンド: `cargo test -p task-dispatch --lib dispatcher::tests::quota_bookkeeping_does_not_change_account_or_lane_selection`
- 結果: 1 passed（同じ観測値・同じ設定で 2 回走らせ、片方だけ事前に「無関係な別 run がまだ重なって
  いる」quota の状態と較正材料を仕込んでも、選ばれるアカウント（`WorkerStarted.account`）と
  lane（`routing_audit` の最後の判定）が一致することを確認。sanity として、残量の多い方の
  アカウントが選ばれることも確認）。
- 実装上の裏付け: `select_provider`・`decide_lane_for_work_unit`・`accounts::evaluate`/`select_account`
  はどれも `self.quota_activity`/`self.quota_calibration` を読まない（grep で確認）。quota の読み書きは
  `quota_begin`/`resolve_quota_estimate`/`release_quota_if_tracked` の 3 箇所に閉じている。

### schema 再生成

- `UPDATE_SCHEMA=1 cargo test -p task-core -p task-api committed_schema_matches_generated
  store::tests::event_row_schema_matches_committed` → 全 pass。
- 差分: `docs/api/v1/event.schema.json`（`Event::QuotaEstimated`、`QuotaWindow`/`QuotaMethod`/
  `QuotaWindowUse`/`QuotaCalibration`）、`docs/api/v1/api-v1.schema.json`（同上に加え
  `QuotaUse`/`AccountNowView`/`ExecutionMetrics.{quota,quota_unknown_runs,cost_usd_complete}`/
  `ExecutionMetricsGroup.{quota,cost_usd_complete}`/`ExecutionMetricsSummary.accounts_now`/
  `WorkUnitView.quota`）。`docs/protocol/execution-plan.schema.json` /
  `docs/protocol/execution-plan-delta.schema.json` / `docs/protocol/worker-protocol.schema.json` は
  変更なし（この Phase では `WorkUnitSpec`/`ExecutionPlanSpec` を触っていない）。

### ADR-0074 からの逸脱・明確化（詳細は ADR 本文の「Phase F3（quota）実装時の逸脱・明確化」節）

1. `Event::QuotaEstimated.windows` の各要素に窓ごとの `method` を残した（事象全体の代表 `method`
   は別に持つ。5h/7d で判定が食い違いうるため）。
2. `before_is_valid` の「他の消費」判定は、履歴を新設せず `QuotaActivity`（run の重なりの追跡）に
   委ねた。celeris の外の消費（U-F3）は引き続き区別できない。
3. quota の bookkeeping（`QuotaActivity`/`QuotaCalibrationBook`）はプロセス内メモリのみ
   （events を横断する store の問い合わせは新設していない。再起動でやり直しになる）。
4. quota は worker/WU run だけ（planner・reviewer run は対象外。ADR §6 F3 の「触るファイル」の
   絞り込みと、reviewer の完了経路の複雑さを踏まえた判断）。
5. アカウントプールを使わない run は一律 `free`（`account_adapter.is_none()`）。
6. apportioned のグループ境界は「そのアカウントの busy period」（重なりのペアごとではなく、
   連鎖的な重なりを 1 グループとして扱う）。
7. `ExecutionMetricsTaskRow.has_quota_events` を追加（ADR §6 F3 の「触るファイル」一覧に `store.rs`
   は無いが、D4.3 の記述をそのまま実装するのに必要だった。性能への影響は「未解決事項」参照）。

### 未解決事項・提案

- **性能**: worker/WU run のたびに `Event::QuotaEstimated` を出すため、run のあるタスクは
  ほぼ確実に `has_quota_events = true` になり、`GET /metrics/execution` の「索引だけで足りる」
  最適化（ADR-0072 E6）の効きが弱まる。実データでの再計測は行っていない。次の一手として、
  グループ集計の `quota` を `execution_metrics_task_rows` 側にも部分的に持たせる（例えば
  `has_quota_events` だけでなく `quota` 自体を索引化する）ことを検討する。
- **calibration の永続化**: `QuotaCalibrationBook` はプロセス内メモリのみ。dispatcher の再起動が
  頻繁な運用では `estimated` に必要な 3 件の `measured` サンプルが溜まる前に消え、しばらく
  `unknown` に倒れ続ける可能性がある。events から `measured` の履歴を再構築して起動時に温める、
  という改善を提案する（U-F4 の較正の妥当性そのものの検証と合わせて、F5 の実機で判断材料を
  集めてから決める）。
- **planner/reviewer run の quota**: 本 Phase では対象外（逸脱 4）。次の一手として、reviewer の
  完了経路（`on_review_finished`/`finish_reviewer_run_index`）に同じ `quota_begin`/
  `resolve_quota_estimate` の形を配線することを提案する（Phase F1 の逸脱節が挙げた reviewer 側の
  `usage`/`end` の欠落バグの周辺なので、合わせて調査するとよい）。
- **`docs/celeris-api-v1.md` は編集していない**: `GET /tasks/{id}/execution`/
  `GET /metrics/execution` 自体がこの契約ドキュメントに元々未記載（Phase E5/F1 由来の既存の
  ギャップ、本 Phase が作ったものではない）。gui/CLAUDE.md の禁止（`docs/celeris-api-v1.md` の
  書き換え）に従い、本 Phase では追記していない。GUI 側の Phase で
  `GET /tasks/{id}/execution`・`GET /metrics/execution`（quota・accounts_now を含む）の節を
  追加することを提案する。
- **F4（案件計画）との関係**: `accounts_now` は Task 単位ではなく系全体の値なので、案件の DAG
  （F4 D3.5）の各ノードに quota を出すときは `TaskExecutionView.metrics.quota`
  （Task 単位の合計）をそのまま使えばよい。

### コマンド・出力の要点（本チェックポイントの最終ゲート）

- `cargo fmt --all -- --check`: 差分なし。
- `cargo test --workspace --no-fail-fast`（`CARGO_TARGET_DIR` を専用ディレクトリに分離。上記
  「環境」節参照）: **FAILED 0**（`test result:` の合計で 2,365 tests passed。task-core lib 371 /
  task-dispatch lib 315 / task-ops lib 325 / task-worker lib 522 / task-api 全体（lib + 統合テスト）
  353 / llm-proxy・celeris・celerisctl・e2e を含む全クレート）。
- `cargo clippy --workspace --all-targets -- -D warnings`: warning 0。
- GUI: 上記 (j) の各コマンドの結果のとおり（`typecheck`/`lint`/`test`/`build`/`mobile-audit`
  すべて exit 0、違反 0）。

### コマンド・出力の要点（本チェックポイントの最終ゲート）

- `cargo fmt --all -- --check`: 差分なし。
- `cargo test --workspace --no-fail-fast`: **FAILED 0**（81 個の test result ブロック合計 2,324 tests
  passed、failed 0。task-core/task-ops/task-api/task-worker/task-dispatch/llm-proxy/celeris/
  celerisctl/e2e の全クレート。以前 1 度だけ観測した `provider_admin_scenarios::reload_clears_provider_cooldown`
  の高負荷時のタイミング依存の flake（既知。単体では常に pass）は今回の実行では発生しなかった）。
- `cargo clippy --workspace --all-targets -- -D warnings`: warning 0。

## Phase F2「WU の並列実行: schema v2、WU ごとの worktree、統合 WU、鍵 (task, WU)」（(a)(b) と下ごしらえ。(c)〜(l) は末尾の Phase F2b 節で完了 2026-09-26）

ADR-0074 §6 F2 の受け入れ条件 (a)〜(l) のうち、本 checkpoint では **(a) と (b)** を実装した。
branch `worktree-agent-a5ad8285d9e9540a2`（着手前に `git merge main` で F1 の統合を取り込み済み。
merge commit は fast-forward、`aeed716`(F0) → `744744f`(F1 最終) の 5 コミットを取り込んだ）。
(c)〜(l) は次の checkpoint に持ち越す（下記「未解決・次への申し送り」）。

### (a) execution-plan/2 の検証

- 実装: `crates/task-core/src/execution_plan.rs`。
  - `EXECUTION_PLAN_SCHEMA_V2 = "celeris.execution-plan/2"` を新設。
  - `WorkUnitKind::Integrate`（system WU 専用の予約 kind）を追加。
  - `PhaseSpec { key, kind, title }` を新設。
  - `ExecutionPlanSpec` に `phases: Vec<PhaseSpec>`（既定空）・`children: Vec<serde_json::Value>`
    （既定空。F4 まで空だけ許す）を追加。`WorkUnitSpec` に `phase: Option<String>`
    （v1 は常に `None`、v2 は必須）を追加。
  - `validate()` を `spec.schema` で v1/v2 に分岐: v2 は `phases` 1..=`max_phases`（既定 5）、
    工程 key の形式・重複、WU の `phase` が必須かつ既知の工程を指す、依存が「同じ工程か前の工程」
    （後の工程への依存は `DependencyInLaterPhase` で拒否）、同じ工程内の依存は高々 1 つ
    （`TooManyIntraPhaseDependencies`。鎖・木は許すが 2 つ以上の直接の親は拒否）。`children`
    は v1/v2 共通で空以外を拒否（`NonEmptyChildren`）。`kind = integrate` は v1/v2 共通で拒否
    （`ReservedKind`）。v1 は `phases`/`work_units[].phase` が非空・`Some` なら拒否
    （`PhasesNotAllowedInV1`/`WorkUnitPhaseNotAllowedInV1`）。
  - `max_work_units` は v1 専用のまま既定 8、新設 `max_work_units_v2`（既定 10）を v2 に使う。
  - `topo_sort` に `phase_rank`（工程の key → 出現順。v1 は空 map）を足し、`(phase_rank, key)`
    の順で tie-break（v1 は常に rank 0 なので挙動は 1 バイトも変わらない）。
  - `apply_delta`（Phase F1 の差分適用）は `base.phases`/`base.children` をそのまま持ち越す。
- 後方互換の確認: 既存の v1 のテスト・`dispatcher.rs`/`claude_code.rs` の v1 planner プロンプト・
  `celeris/src/delivery.rs` の暗黙 WU 生成は無変更（`WorkUnitSpec`/`ExecutionPlanSpec` の struct
  リテラルへ `phase: None`/`phases: Vec::new()`/`children: Vec::new()` を機械的に追加しただけ）。
- コマンドと結果:
  - `cargo test -p task-core --lib execution_plan::` → **37 passed; 0 failed**
    （既存 17 件 + 新規 20 件。新規テストの一覧: `v1_plan_without_phases_still_validates_exactly_as_before`、
    `v1_rejects_phases_being_set`、`v1_rejects_a_work_unit_with_a_phase_set`、
    `v2_valid_plan_with_parallel_and_stacked_units_passes`、`v2_rejects_no_phases`、
    `v2_rejects_too_many_phases`、`v2_rejects_duplicate_phase_keys`、
    `v2_rejects_a_work_unit_missing_its_phase`、`v2_rejects_a_work_unit_with_an_unknown_phase`、
    `v2_rejects_a_dependency_on_a_later_phase`、`v2_rejects_two_intra_phase_dependencies`
    （ADR §6 F2 のテスト表が指す名前どおり）、`v2_allows_a_tree_of_intra_phase_dependencies`、
    `v2_uses_the_v2_work_unit_limit_not_the_v1_one`、`rejects_children_being_non_empty_in_either_version`、
    `rejects_a_work_unit_with_the_reserved_integrate_kind`、`rejects_a_schema_that_is_neither_v1_nor_v2`、
    `v2_topological_order_respects_phase_order_for_independent_units`）。
  - `UPDATE_SCHEMA=1 cargo test -p task-core --lib event_row_schema_matches_committed` と
    `UPDATE_SCHEMA=1 cargo test --workspace committed_schema_matches_generated` → 全 pass。
    差分: `docs/api/v1/api-v1.schema.json`・`docs/api/v1/event.schema.json`・
    `docs/protocol/execution-plan.schema.json`・`docs/protocol/execution-plan-delta.schema.json`
    （`ExecutionPlanSpec`/`WorkUnitSpec`/`PhaseSpec`/`WorkUnitKind` の新しい形。
    `worker-protocol.schema.json` は変化なし）。

### (b) migration 0027 と旧い DB からの移行テスト

- 実装: `crates/task-core/migrations/0027_parallel_work_units.sql`（ADR §5.2 のとおり、
  `work_units` に `phase`/`lease_run_id`/`lease_expires_at`/`branch`/`base_commit`/`head_commit`/
  `integrated_commit` の 7 列と `idx_work_units_lease`（部分索引）を追加）。
  `crates/task-core/src/store.rs`: `SCHEMA_VERSION` を 26 → 27、`MIGRATION_0027` 定数と
  `migration_sql()` への登録、既存の 6 箇所の `assert_eq!(SCHEMA_VERSION, 26)` を 27 に更新。
  この checkpoint では **Rust の `WorkUnitRow` / 読み書きロジックは変えていない**（(c) 以降で
  `WorkUnitCommitted`/`PhaseIntegrated`/lease 取得の実装と合わせて追加する。ADR への追記参照）。
- コマンド: `cargo test -p task-core --lib migration_27_adds_work_unit_lease_columns`
- 結果: 1 passed（版数 26 の DB を fixture で作り、`SqliteStore::open` で 27 まで migrate → 新しい
  7 列が `PRAGMA table_info` に現れる、旧い行は新しい列が NULL のまま読める、新しい列は
  round-trip で書き込める、`idx_work_units_lease` の部分索引が実際に使える、再度 open しても
  冪等であることを確認）。`cargo test -p task-core --lib migration` → 既存 10 件 + 新規 1 件、
  **11 passed; 0 failed**。

### 最終ゲート（本 checkpoint）

- `cargo fmt --all -- --check`: 差分なし（`cargo fmt --all` を 1 度実行して整形）。
- `cargo test --workspace --no-fail-fast`: **FAILED 0**（exit code 0。task-core の新規 21 テスト
  〈v2 検証 20 + migration 1〉を含め全クレートで failed 0）。
- `cargo clippy --workspace --all-targets -- -D warnings`: warning 0。

### ADR-0074 からの逸脱・明確化

詳細は ADR 本文の「Phase F2 実装時の逸脱・明確化」節（進行中、区切りごとに追記）。要点:
1. `max_work_units` を v1/v2 で分離する専用フィールド `max_work_units_v2` を新設（config 経由の
   上書きはまだ無い。既存の他のサイズ上限フィールドと同じ扱い）。`max_phases` も同様に新設。
2. `children` は F4 の型を先取りせず `Vec<serde_json::Value>`（空だけ許す）にした。
3. `kind = integrate` を検証で拒否する `ReservedKind` を新設（ADR に明示は無いが D1.4 の
   「system WU 専用」という設計意図を検証で保証する）。
4. `topo_sort` に工程順の tie-break を追加（v1 の挙動は不変）。
5. `apply_delta` は `phases`/`children` を base のまま持ち越す（replan は work_units の差分だけ）。
6. migration 0027 は SQL 列の追加のみで、Rust 側の読み書きは (c) 以降に送った。

### checkpoint 2: (c) の下ごしらえ — `runnable_work_units`（純粋関数）

`dispatcher.rs` への配線（`RunKey`・WU の worktree・lease 取得）はまだだが、D1.3 が指す
`next_work_unit(&[WorkUnitRow]) -> NextStep` の一般化 `runnable_work_units(&[WorkUnitRow],
in_flight: usize, limit: usize) -> Vec<String>` を `crates/task-core/src/execution_plan.rs` に
先に実装した（`next_work_unit` は置き換えず並存。`dispatcher.rs` の呼び出し側を切り替えるのは
次の checkpoint）。

- 「現在の工程」は `spec.phase`（WorkUnitSpec に保持済み。専用の `WorkUnitRow.phase` 列はまだ
  追加していない）と `seq` から求める: 有効（`!is_terminal()`）な WU のうち `seq` 最小のものの
  `phase` が「現在の工程」（(a) の `topo_sort` が工程順を保証するため、`seq` の最小値で工程の
  順序が引ける。専用の phase 列を待たずに実装できた）。
- `needs_continuation` を先に、`ready` を `seq` 順で選ぶ。`limit.saturating_sub(in_flight)` 件まで。
- 兄弟に `failed`/`blocked` があれば `ready`（新規）は選ばないが `needs_continuation`（続き）は
  選ぶ（D1.6 の表「走っている WU とその continuation だけは続ける」に合わせた）。
- コマンド: `cargo test -p task-core --lib execution_plan::tests::runnable_work_units`
- 結果: **6 passed**（`runnable_work_units_matches_next_work_unit_for_v1_with_limit_one`
  〈v1 が `next_work_unit` と同じ 1 件を返すことの確認〉、
  `runnable_work_units_respects_the_parallel_limit`（ADR §6 F2 のテスト表の名前どおり）、
  `runnable_work_units_prefers_needs_continuation_over_ready`、
  `runnable_work_units_only_considers_the_current_phase`、
  `runnable_work_units_does_not_start_new_ones_when_a_sibling_failed_but_continues_in_flight`、
  `runnable_work_units_returns_empty_when_everything_is_terminal`）。
- ゲート: `cargo fmt --all -- --check` 差分なし、
  `cargo clippy --workspace --all-targets -- -D warnings` warning 0、
  `cargo test --workspace --no-fail-fast` **FAILED 0**（2,434 個の `test ...` 行、exit code 0）。

### 未解決事項・次の checkpoint（(c) 以降）への申し送り

- (c) 独立 WU の並列実行、(d) 積み上げ、(e) 衝突と repair、(f) 統合後検査、(g) 兄弟 in-flight、
  (h) 再起動照合、(i) Cancel カスケード、(j) 公平性、(k) remote/dir/Shared のフォールバック、
  (l) GUI（工程列・同時 run・ブランチ、`gen:types`/`mobile-audit`）はいずれも未着手。
  ADR §6 F2 の「触るファイル」表のうち `crates/task-dispatch/src/{dispatcher.rs,
  execution_scheduler.rs, checkpoint.rs}`、新規 `crates/task-dispatch/src/integration.rs`、
  `crates/task-worker/src/{protocol.rs, preamble.rs}`、`crates/task-ops/src/{execution.rs,
  replay.rs, view.rs}` のうち WU の worktree・lease・統合に関わる変更、`gui/app/components/
  ExecutionSection.tsx`・`gui/app/lib/task-execution.ts` は本 checkpoint では未着手。
- `WorkUnitRow`（Rust）に `phase`/`lease_run_id`/`lease_expires_at`/`branch`/`base_commit`/
  `head_commit`/`integrated_commit` を足し、`store.rs` の insert/update/select と
  `replay::rebuild_work_units_and_runs` の復元ロジックを実装するのが (c)/(d)/(h) の最初の一歩。
- `RunKey { task, work_unit: Option<String> }`（D1.5）への `dispatcher.rs` の `running:
  HashMap` の切り替えが (c) の中心的な変更になる（現状は `TaskId` が鍵）。範囲が広いため、
  (c) 自体をさらに小さい区切りに分けることを推奨する。
- planner のプロンプトへの v2 `phases` の書き方の追加（`claude_code.rs`）は、v2 を実際に
  planner に出させる `[execution] parallel` の config・dispatcher 側の分岐と合わせて (c)/(d) の
  範囲で行う（今回は v1 のプロンプト・挙動を 1 バイトも変えないことを優先し、着手していない）。

## Phase F2b「WU の並列の本体: 鍵 (task, WU)、WU ごとの worktree、統合 WU、伝播・再起動・Cancel・公平性・GUI」（完了日 2026-09-26）

ADR-0074 §6 F2 の (c)〜(l)。branch `worktree-agent-ac4722421a9ef87da`（着手時に main b53130e へ fast-forward、途中で
coordinator の指示により main b009c88〈F3 quota〉を merge。衝突は `model.rs`・`query.rs`・`dispatcher.rs` の
run 開始部分で、F3 の `quota_begin` を `dispatch_one` に移して解消）。ビルドは coordinator の指示で
`CARGO_TARGET_DIR=/var/lib/celeris/build-cache/cargo/agent-platform-f2b`。

### 区切りと commit
- (c-1) `cdcaa68`: `WorkUnitRow` に phase / lease_run_id / lease_expires_at / branch / base_commit / head_commit /
  integrated_commit、store の読み書き、`acquire_work_unit_lease`・`extend_task_lease`・
  `running_tasks_with_runnable_work_units`、`renew_lease` の WU lease 対応。
- (c-2) `021512e`: `running: HashMap<RunKey, RunEntry>`（`RunKey{task, work_unit}`）、`run_holds_lease`、
  `reclaim_expired_leases` / `abort_stale_runs` / drain / snapshot の鍵対応（v1・atomic は不変）。
- (c-3) `ebdda50`: 新規 `crates/task-dispatch/src/integration.rs`（WU の worktree・ブランチ・commit・冪等な統合）。
- (c-4 前半) `109f4d5` → merge `d85aade` → (c-4)(c-5)(d)〜(k) `e6cac7b`（dispatcher の状態機械の変更が絡み合うため
  1 commit にまとめた）→ replay/API/prompt `b42fccb` → (l) GUI と ADR `b580413` → 本節の commit。

### 受け入れ条件ごとの証拠
- (c) 3 つの独立 WU が並列 3 で同時に走り、各 `celeris-wu/<task>/<key>` で commit、統合 WU が決定的に merge して
  `PhaseIntegrated`。並列 2 なら 2 本ずつ、provider `concurrency = 1` なら 1 本ずつ。v1 は不変。
  - `cargo test -p task-dispatch --lib -- three_independent two_parallel provider_concurrency_one a_parallel_unit a_v1_plan`
    → 5 passed（`three_independent_units_run_in_parallel_and_integrate`: 最大同時 3、WU ブランチの最終 commit の作者
    `celeris`・件名 `wu/<key>: …`、merge の件名 `integrate wu/<key> (phase build)` が seq 順、`WorkUnitCommitted` 3 件、
    `PhaseIntegrated` 1 件、Task の遷移は dispatch 1 回と worker_done、replay の突き合わせで WU/計画の差分 0）。
  - store: `cargo test -p task-core --lib store::tests::acquire_work_unit_lease store::tests::work_unit_parallel store::tests::running_tasks_with`
    ほか → 全 pass。`integration::tests` 5 passed。
- (d) 積み上げ: `stacked_unit_branches_from_its_dependency` → ok（b の base_commit = a の head_commit、b の作業ツリーに
  a.txt がある、`PhaseIntegrated.merged = ["b"]`）。純粋関数 `phase_leaves_are_the_units_nobody_in_the_phase_depends_on` ok。
- (e) 衝突: `integration_conflict_creates_a_merge_repair_and_resumes` → ok（`merge-build-b`〈kind repair、Task の
  worktree〉、`RepairScheduled{class: merge_conflict, origin: integration}`、再開した統合は全部 skipped、a の merge は 1 回）。
  `integration::tests::merge_is_idempotent_after_restart` ok。
- (f) 統合後の検査: `integration_check_failure_is_repaired_when_classified`（`rustfmt` を含む検査 → class `format` の
  `repair-build-1`、修復後に検査 pass で Done）と `integration_check_failure_replans_when_not_classified`
  （分類に当たらない → 統合 WU failed → `replan` の遷移、repair は作らない）→ ok。
- (g) `sibling_failure_waits_for_in_flight_units_before_replan` / `sibling_question_waits_for_in_flight_units_before_blocking`
  → ok（a の失敗・質問の時点で Task は Running のまま、b は完走、b の `WorkerFinished` と同じトランザクションで
  `replan` / `worker_question`）。純粋関数 `execution_scheduler::tests::settle_*` 3 passed。
- (h) `parallel_units_survive_dispatcher_restart`（デーモンを作り直し → Task の lease 失効で `infra_requeue`、WU 2 本が
  `restart_reconcile` で ready → 完走）と `an_interrupted_integration_is_redone_idempotently_after_restart`
  （統合の途中で作り直し → 統合 WU が `restart_reconcile` で pending → やり直し、merge commit は重複せず 2 件、
  `PhaseIntegrated` 1 件）→ ok。
- (i) `cancel_stops_every_work_unit_run` → ok（走っていた 2 run が止まり `running_for_task == 0`、全 WU cancelled、
  WU の worktree とブランチが消える）。
- (j) `ready_task_first_run_beats_a_second_parallel_unit` → ok（全体 2 枠で、並列 WU の 2 本目より Ready の別 Task の
  1 本目が先）。
- (k) `shared_workspace_falls_back_to_serial`（`Shared` → 最大同時 1、`WorkUnitsSerialized` 1 回〈理由に shared〉、
  WU のブランチ無し、統合は no-op の `PhaseIntegrated{merged: [], head: ""}`）と
  `remote_workspace_falls_back_to_serial`（remote・git でない dir → 並列 1 と理由、v1 → 理由なしの並列 1）→ ok。
- planner / worker: `claude_code::tests::the_planner_prompt_explains_v2_phases_only_when_parallel`、
  `a_parallel_work_unit_prompt_names_its_branch_and_forbids_touching_siblings` → ok。
  task-core: `materialize_adds_integration_units_and_only_the_first_phase_is_ready`、`v2_rejects_a_key_reserved_for_integration_units` ok。
- (l) GUI（`cd gui`、`corepack pnpm@11.27.0`）: `gen:types` 後 `git diff --exit-code -- gui/app/celeris/types.ts` 差分 0、
  `typecheck` exit 0、`lint` exit 0（既存の info 2 件のみ）、`test` 1099 passed、`build` exit 0、
  `mobile-audit` → `routes=27 schemes=2 violations=0`（fixture を v2 の計画〈工程 2・ブランチ・running の run・統合 WU〉に更新）。

### 最終ゲート
- `cargo fmt --all -- --check`: 差分なし。
- `cargo clippy --workspace --all-targets -- -D warnings`: warning 0。
- `cargo test --workspace --no-fail-fast`: **FAILED 0**（92 の test result、2,423 passed、exit 0）。1 回目の実行では既知の
  高負荷時の flake `e2e::provider_admin_scenarios::reload_clears_provider_cooldown` が 1 回落ちた（単体で 3 回連続 pass、
  再実行の全体でも pass。本 Phase は provider の cooldown に触れていない）。
- `UPDATE_SCHEMA=1 cargo test --workspace committed_schema_matches_generated` で再生成（`api-v1.schema.json`・
  `event.schema.json`・`worker-protocol.schema.json`）、再生成なしで 3 つの schema テスト pass。

### ADR-0074 との差分（詳細は ADR の「Phase F2b 実装時の逸脱・明確化」26 項）
- 衝突の repair の class は `merge_conflict`（依頼文の `integration` は origin の値にした）。
- `PhaseIntegrated.checks[]` は `{cmd, pass, summary}`（exit code は取れない）。並列 1 の理由は新 Event `WorkUnitsSerialized`。
- in-flight 0 のときの優先順は question → 失敗 → 起こせる WU → 統合。
- 再起動の照合は WU の lease の失効だけを見る（引き継ぎ中の別インスタンスの run を奪わない）。統合のやり直しは Task の
  lease の失効 → 既存の `InfraRequeue` の中で。
- `max_parallel_work_units` は `[execution]` だけ（Task・CoS・profile で狭める経路は未実装）。
- `runs/` は共有、成果物は `wu/<key>/artifacts`、setup は WU の worktree ごと。

### 未解決・申し送り
- Task / CoS / profile の `max_parallel_work_units`（型に欄が無い）。F4/F5 で型を足すときに配線する。
- 統合の repair WU・最終レビューの repair WU は spec が events に無く、replay で行を作り直せない（突き合わせで
  「stored にだけある」差分になる。既存の E4 の repair と同じ限界）。`RepairScheduled` に spec を載せるのが次の一手。
- 何も走っていない（Ready の）v2 Task の Cancel では WU の行は cancelled にならない（従来どおり）。
- F3 途中確認（D2）への申し送り: 工程の統合の成功（`finish_phase_integration`）が `Continue{advance}` / `WorkerDone`
  を選ぶ場所に `pause_after` の判定を差し込む。`PhaseIntegrated` は途中報告の材料になる。
- F4 への申し送り: `ExecutionPlanSpec.children` は空のまま。並列の既定は `[execution] parallel = false`（F5 で人が切り替える）。

## release a770bcb5b7b5 の昇格（F1 + F2 前半 + F3-quota、schema 27）と F2b の統合（2026-09-26 08:27Z）

- verify（ルート満杯の解消後に再実行）全 true（N-1 は schema 27 で想定どおり false）。in-flight 0 で停止→起動の昇格。本番は WU ごとの lane / planner standard / replan 差分 / repair 分類 / 成果物登録 / 計画 v2 schema / quota 記録が有効。`workspace_root` はローカルのまま。
- F2b（WU 並列の本体、(c)〜(l)）を main に統合（ddd6a5d）。統合時に GUI の `WorkUnitKind::integrate` ラベルが F2b 側と私の暫定修正で重複 → 1 つに。ゲート: fmt 0 / test FAILED 0 / clippy 0 / GUI typecheck・lint・test 1099 件・gen:types 差分ゼロ・build。

## Phase F5-1 dogfood: ディスクゲート・codex cache usage・API 文書（2026-09-26）

- `[dispatch] min_free_disk_mb`（既定 5120）を導入。新規 worker/reviewer run の前に `/`、`workspace_root`、`build_cache_dir` の空きを確認し、不足中は新規起動を止めて Phase 116 infra の「ディスク不足」を BadNews 通知に 1 回登録する。空きが戻れば次の tick で自動再開する。既存 run の完了・lease 処理は続ける。
- `celerisctl build-cache prune [--older-than <days>] [--dry-run]` は設定済み `build_cache_dir/cargo/*` の古い実ディレクトリだけを対象とする。既定 30 日、symlink は除外。
- Codex CLI `turn.completed.usage.cached_input_tokens` を `Usage.cache_read_tokens` に取り込む。`ExecutionMetrics.total_cache_read_tokens` は観測値のみ合計する。詳細は ADR-0074 の Phase F5-1 節。
- 実行・計画・再実行 API の型と既定値を `docs/celeris-api-v1.md` に追加した。

### F5-1 配送準備の修復（2026-09-26）

- 取り込み後の release gate は全段成功し、`gate.json.ok = true` とリリースディレクトリが生成された。しかし `prepare.sh` の 3600 秒制限が worktree の掃除中に発火し、verify 前に `result.json.ok = false` となった（`prepare.log`: 09:05:11 開始、10:04:31 release ready、10:05:11 Terminated）。
- clean 475 秒、全 workspace テスト 2416 秒を要した実測に合わせ、release 全体の上限を 7200 秒に変更。verify の 900 秒上限は維持。軽量なモックテストで両上限と成功・失敗時の結果ファイルを確認する。
- 修復 SHA `e07cc0f` の release gate では、`task-worker` 内の孤児子プロセス回収テストが並列実行中の別テストの `git commit` を先に reap し、3 件が `No child processes` で落ちた。回収テストを別 integration-test binary へ移し、`waitpid(-1)` が他の unit test の子を横取りできないようにした。

## F5-1 dogfood の結果（2026-09-26 08:32〜12:26Z、タスク 01M3EDF3JEHRQCG6A2EJDRQMXJ）と 2 つの発見

- 結果: done（壁時計 3h55m）。ただし **atomic で走った**（`has_plan: false`）。原因: 本番の `[execution] gate = "shadow"` では、人が `execution: compound` を明示した Task（`source = human`, `rule_id = human/explicit`）も採用されず記録だけになる（dispatcher の `shadow` が一律に効く）。E6 は `gate = on` だったので compound になった。→ 「人の明示は shadow でも採用する」修正を F3（途中確認）担当に追加指示。F5-1 は **F1 の WU ごとの lane の効果を測れていない**（worker 4 run はすべて `standard/default` / gpt-6-sol、これは atomic の従来動作）。修正が本番に入ってから F5-1 をやり直す。
- 取れたもの: continuation 1 回（E1 の仕組みが本番で 1 回働いた）、retry 0、費用 4.63 USD 相当（`cost_usd_complete = false`）、quota は codex 7 日窓で `measured` 4 run（5 時間窓は unknown）。3 成果（ディスク残量チェック、codex cache usage、API docs）は配送待ち。
- 発見 2: 終端した dogfood の workspace に `repos/agent-platform/target` が 25G あり、同時に `build-cache/cargo/agent-platform-f1d3fe5cc3`（Celeris が渡す `CARGO_TARGET_DIR`）にも 21G あった。worker か reviewer の cargo が Celeris の `CARGO_TARGET_DIR` を受け取らずに worktree 直下に target を作っている（`~/.cargo/config.toml` の `[build] target-dir` より env が勝つはずなので、env が渡っていない経路がある）。ルートディスクは 88% まで戻った → 両方削除して 69%。提案 P-F5-1: worker / reviewer / checks のすべての cargo 実行に `CARGO_TARGET_DIR` を渡す経路を確認し、終端タスクの prune に `repos/*/target` を含める（P-115-4 と同じ）。
- 提案 P-F5-2: ルート LVM（252G）は DB + workspaces + build-cache + 実装エージェント 2 本の target で常に 60〜90%。Proxmox 側で 512G へ拡張するか、build-cache を別ボリュームにする（人の判断）。
- 12:37Z: F5-1 dogfood の配送が作った release `e3465764475c`（main e346576 = F2b の WU 並列本体 + F5-1 の 3 成果: dispatch 前のディスク残量チェックと `celerisctl build-cache prune`、codex cache usage の確認、API docs の追記。gate ok / verify ok / live_ok）を in-flight 0 でライブ昇格（from a770bcb5b7b5）。本番で WU 並列（計画 v2 の `phases`）が使える状態。gate は shadow のまま（人の明示を採用する修正は F3 待ち）。

## Phase F4a「案件レベルの計画（前半）: マイルストーン Task の述語・CoS の案件計画と提案・人の承認」（着手 2026-09-26、完了 2026-09-27）

ADR-0074 D3 の (a)(b)(c) だけを実装する。(d)〜(h)（Go の判定・案件 replan・children・互換テスト・GUI の DAG）は
F4b に送る。作業は worktree の中（main へ merge / push しない）。

### F4a checkpoint 1: (a) `is_milestone_task` と 1:1 の不変条件（完了 2026-09-26）

- **条件**: `task_core::is_milestone_task` の述語（案件直下 = `project_id.is_some() && parent_id.is_none()
  && kind == Execute && conversation.is_none() && support_kind.is_none()`）を実装し、`task_ops::add` が
  案件直下に `milestone_id` 無しで作られる Task について、同じトランザクションで途中目標の行
  （`approved`、title = Task の title）を作って結ぶ（1:1）。既存の案件（途中目標が手で作られたもの、
  `milestone_id` を明示する経路）の挙動は変えない。
- **実装**: `crates/task-core/src/org.rs`（`is_milestone_task`、`crate::report::support_kind` と
  `task.conversation` を見る）、`crates/task-core/src/lib.rs`（re-export）、`crates/task-core/src/store.rs`
  （新しい `TaskStore::create_task_with_milestone`: `create_task` と同じトランザクションで
  `milestones` 行を 1 件追加で作る。呼び出し側があらかじめ新しい `MilestoneId` を `task.milestone_id`
  に入れて渡す）、`crates/task-ops/src/add.rs`（`insert_task` ヘルパーを新設し、`create_task_with_roles` /
  `create_support_task` の挿入経路をこれに統一。`is_milestone_task(&task) && milestone_id.is_none()`
  のときだけ `create_task_with_milestone` を使う）。
- **実行したコマンド・出力の要点**:
  - `cargo test -p task-core -p task-ops --no-fail-fast` → `test result: ok. 328 passed; 0 failed`（新規
    `org::tests::is_milestone_task_requires_project_root_execute_position`、
    `add::tests::creating_a_top_level_execute_task_without_a_milestone_id_auto_creates_an_approved_milestone`、
    `add::tests::creating_a_top_level_task_with_an_explicit_milestone_id_does_not_auto_create_another`、
    `add::tests::a_child_task_under_a_project_does_not_auto_create_a_milestone` を含めすべて green）。
  - `cargo test -p task-api --no-fail-fast` → 全 suite green（`organization.rs` の
    `projects_and_milestones_round_trip_through_the_api` / `project_detail_returns_the_milestones_and_the_work_tree`
    を含め、既存の途中目標が手で作られた経路の挙動が変わっていないことを確認）。
  - `cargo fmt --all -- --check` → 差分ゼロ（`cargo fmt --all` 後）。
  - `cargo clippy -p task-core -p task-ops --all-targets -- -D warnings` → warning 0。
- **既知の限界**: 新しい `TaskStore::create_task_with_milestone` は `SqliteStore` にのみ実装（他に実装者は
  無い）。`docs/protocol/*.schema.json` に影響する型変更は無し（`Milestone` / `Task` の shape は変えていない）。

### F4a checkpoint 2: (b) `celeris.project-plan/1` の schema・検証・CoS の提案（完了 2026-09-26）

- **条件**: `celeris.project-plan/1` の schema（`docs/protocol/project-plan.schema.json`）と検証
  （循環・未知 key・件数上限）を `crates/task-core/src/project_plan.rs` に実装。
  `POST /projects/{id}/plan {mode: "milestones"}` で秘書 run（偽アダプタ）が計画を出し、検証を通ると
  `Event::ProjectPlanProposed`、途中目標（`proposed`）、top-level の draft Task（`depends_on` 付き）が
  でき、受信箱の `drafts` に 1 まとまりで出る。秘書のプロンプト（`claude_code.rs`）に案件計画の書き方。
- **実装**:
  - `crates/task-core/src/project_plan.rs`（新規）: `PROJECT_PLAN_SCHEMA`、`MilestoneSpec` /
    `ProjectPlanSpec`（`deny_unknown_fields`。`acceptance` は既存の `task_core::Criterion` を再利用）、
    `ProjectPlanLimits`（既定 1..=12 件、rationale ≤ 2,000 字）、`validate`（Kahn 法でトポロジカル順・
    循環検出、未知 key、重複 key、件数、文字数上限、acceptance 必須・human に deliverable 必須を検証）、
    `is_milestones_plan_task`（`Task.labels` の固定値 `MILESTONES_PLAN_LABEL` で見分ける。フラグ列は
    増やさない）、`schema_value` / `committed_schema_matches_generated`。
  - `crates/task-core/src/model.rs`: `Event::ProjectPlanProposed{project_id, version, supersedes, plan,
    milestones}` / `Event::ProjectPlanDecided{project_id, version, approved, note}`（(c) 用に型だけ先出し）、
    `ProposedMilestone{key, milestone_id, task_id}`。`docs/api/v1/event.schema.json` を再生成。
  - `crates/task-ops/src/project_plan.rs`: `start_milestones`（`POST /plan {mode: milestones}` の入口。
    `MILESTONES_PLAN_LABEL` を付け、既存の途中目標の文脈は渡さない）、`propose`（検証済み計画を
    トポロジカル順に処理し、`milestone_create`〈proposed〉→ `add::create_task_with_roles`〈draft、
    `milestone_id` 明示なので (a) の自動生成は起きない、`depends_on` は既に作った兄弟 Task を指す〉を
    繰り返し、最後に `Event::ProjectPlanProposed` を plan タスクの events に積む）、
    `record_proposal_failure`（不正が最終的に直らなかったとき、秘書の返事として
    「計画を作れなかった: …」を案件の対話〈`role = node`〉に残す）。
  - `crates/task-dispatch/src/dispatcher.rs`: `plan` コンテキスト（`plan.json` 解析）を
    `is_milestones_plan_task` の Plan タスクには渡さない。`finish_project_plan_run`
    （`(true, TaskKind::Plan, None) if is_milestones_plan_task(&task)` の新しいアーム）が
    `artifacts/project-plan.json` を読み、検証を通れば `task_ops::project_plan::propose` → `ReviewPass`、
    不正なら `ReviewFail`（既存の attempts/max_retries が 1 回再試行・諦めるを決める）。失敗が確定したら
    `record_proposal_failure` を呼ぶ。
  - `crates/task-worker/src/claude_code.rs`: `build_project_plan_prompt`
    （`{artifacts}/project-plan.json` の schema・書き方の指示。milestone は `role` を持たない専用の
    「使える専門家」節）。`build_plan_prompt` の分岐は `is_milestones_plan_task` で切り替え、旧来の
    `plan.json` プロンプトは 1 バイトも変えない。
  - `crates/task-api/src/project_plan.rs`: `ProjectPlanBody.mode`（`decompose`〈既定〉/ `milestones`）。
    `milestones` に `milestone_id` を添えると 422。
  - `crates/task-ops/src/inbox.rs`: `build_drafts` を、案件計画の提案（`is_milestone_task` かつ
    紐づく途中目標が `proposed`）とそれ以外（従来の親でまとめる規則）に分け、提案は案件ごとに 1 つの
    `DraftGroup`（`parent: None`、`plan_summary` に rationale と DAG の行）にまとめる。根
    （誰の子でもなくどの提案にも属さない draft）は従来どおり最後。
- **実行したコマンド・出力の要点**:
  - `cargo test -p task-core project_plan:: -p task-ops project_plan::` ほか個別クレート → 新規テスト
    全 green（`project_plan::tests::{a_valid_plan_is_accepted_and_topologically_ordered,
    rejects_cycles_and_unknown_keys, rejects_wrong_schema_duplicate_keys_and_count_limits,
    rejects_empty_acceptance_and_human_checks_without_a_deliverable,
    is_milestones_plan_task_needs_the_kind_and_the_label, committed_schema_matches_generated}`、
    task-ops 側 `start_milestones_labels_the_plan_task_and_carries_no_milestone_context` /
    `propose_creates_proposed_milestones_and_draft_tasks_with_resolved_depends_on` /
    `record_proposal_failure_posts_a_node_message_to_the_project`）。
  - `cargo test -p task-dispatch milestones_project_plan_task_proposes_milestones_and_top_level_draft_tasks
    invalid_project_plan_fails_without_creating_anything_and_notifies_the_secretary` → ok（偽アダプタで
    `POST /plan {mode: milestones}` 相当の全経路〈run → project-plan.json → 検証 → 提案 → drafts〉と、
    不正な計画が retries 尽きて失敗し秘書に通知される経路の両方を確認）。
  - `cargo test -p task-api --test project_plan` → 8 件 green（新規
    `milestones_mode_labels_the_plan_task_and_ignores_existing_milestones` /
    `milestones_mode_with_a_milestone_id_is_422` を含む）。
  - `cargo test -p task-ops inbox::` → 14 件 green（新規
    `inbox_drafts_group_a_project_plan_proposal_as_one_unit_and_keep_root_last`）。
  - `UPDATE_SCHEMA=1 cargo test -p task-core --no-fail-fast` → 409 passed（`event_row_schema_matches_committed`
    と新規 `project_plan::tests::committed_schema_matches_generated` を含む。`docs/api/v1/event.schema.json`
    と `docs/protocol/project-plan.schema.json` を再生成）。
  - `cargo build --workspace` / `cargo clippy --workspace --all-targets -- -D warnings` → warning 0。
  - `cargo fmt --all -- --check`（`cargo fmt --all` 後）→ 差分ゼロ。
  - `cargo test --workspace --no-fail-fast` → **FAILED 0**（`test result: ok. 524 passed; 0 failed; 1 ignored`
    が task-dispatch の内訳、他クレートも全 0 failed。既存の 1 ignored はこの Phase の変更と無関係）。
- **既知の限界・F4b への申し送り**:
  - `version` は常に 1（replan・複数版の追跡は D3.4、F4b）。`ProjectPlanProposed.supersedes` は常に
    `None`。
  - `project_plan_summary`（inbox の plan_summary の組み立て）は「その案件の Plan タスクを総なめして
    `ProjectPlanProposed.milestones` に一致するものを探す」総当たりで、F4a の想定（同時に 1 件の提案）
    では問題無いが、F4b で複数版が絡むと曖昧になりうる（`plan_task_id` を直接引ける索引が無い）。
  - `POST /projects/{id}/plan {mode: milestones}` は「既に未決の提案がある案件への二重発行」を防いで
    いない（F4b の replan・decide の実装と合わせて対処）。
  - `pause_after` は ADR の `celeris.project-plan/1` 例に載っているが、F3 の `PausePolicy` 型がまだ無い
    ため MilestoneSpec には含めていない（F4b で型が揃ってから追加する。deny_unknown_fields なので
    追加は非破壊）。

### F4a checkpoint 3: (c) 案件計画の `decide`（完了 2026-09-27、commit c118844）

- **条件**: `decide approve` で提案中の計画の全途中目標が `approved`、全 Task が `ready`（1 トランザクション）になり、
  依存の無いものから dispatch される（`depends_on` の既存判定で待つ）。`reject` で途中目標が `redesigned`、Task が
  `cancelled` になり、却下理由が秘書への対話として投げられる。`celerisctl` にも同じ操作。
- **実装**:
  - `crates/task-core/src/store.rs`: `TaskStore::project_plan_decide_apply`（途中目標の状態・Task の遷移・
    `ProjectPlanDecided` を 1 つの IMMEDIATE トランザクションで。`Cancel` のカスケードで既に終端の Task は飛ばす）。
  - `crates/task-ops/src/project_plan.rs`: `decide`（`find_proposal` で版を探す〈無ければ `ProjectPlanProposalNotFound`、
    決定済みなら `ProjectPlanAlreadyDecided`〉、reject は note 必須・秘書の存在を書き込み前に確認、commit 後に
    `conversation::start` で却下理由を秘書へ）。
  - `crates/task-ops/src/gate.rs`: 未決の提案に属する draft の個別 `accept` / `approve` を 422 で拒む。
  - `crates/task-api/src/project_plan.rs`: `POST /api/v1/projects/{id}/project-plan/{version}/decide`（管理系、202）。
    `schema.rs` に `ProjectPlanDecideBody` / `ProjectPlanDecided`、`docs/api/v1/api-v1.schema.json` と
    `gui/app/celeris/types.ts` を再生成。
  - `crates/celerisctl`: `celerisctl projects plan approve|reject <project> [version] [--note] [--config]`（`project` の別名）。
  - `crates/celeris-mcp/src/tools/tasks.rs`: 新しい `OpsError` の分類だけ（MCP からは呼べない）。
- **実行したコマンド・出力の要点**:
  - `cargo test -p task-ops project_plan::` → 14 passed / 0 failed（新規 `approve_readies_the_whole_dag_in_one_transaction`
    〈全 approved / ready、`ready_tasks` に出るのは依存の無い `survey` だけ、`ProjectPlanDecided{approved: true}`〉、
    `decide_reject_requires_a_note_and_cancels_everything_notifying_the_secretary`、
    `decide_on_an_unknown_or_already_decided_version_is_rejected`、
    `project_plan_decide_apply_writes_nothing_when_any_step_fails`〈無い Task を混ぜると途中目標も Task も events も無変更〉）。
  - `cargo test -p task-ops gate::` の新規 `accept_and_approve_refuse_a_draft_that_belongs_to_an_undecided_project_plan_proposal` → ok。
  - `cargo test -p task-api --test project_plan` → 11 passed（新規 `decide_approve_readies_the_dag_reject_needs_a_note_and_replays_are_rejected`
    〈202 / 422 / 404 / 409〉、`decide_reject_redesigns_the_milestone_and_cancels_the_task`、`decide_endpoint_requires_a_token`）。
  - `cargo test -p celerisctl projects` → 4 passed（新規 `run_plan_decide_approves_and_rejects_a_proposal`、
    `plan_approve_and_reject_parse_with_a_default_version`）。

### Phase F4a の全体ゲート（2026-09-27）

- `cargo fmt --all -- --check` → exit 0（差分ゼロ）。
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0（warning 0）。
- `cargo test --workspace --no-fail-fast` → exit 0、合計 **2457 passed / 0 failed / 5 ignored**。
- `UPDATE_SCHEMA=1 cargo test --workspace committed_schema_matches_generated` → 3 suite とも ok、commit 後の `git status` に差分無し。
- `cd gui && corepack pnpm@11.27.0 gen:types` → 差分ゼロ、`typecheck` → exit 0、`test` → Test Files 72 passed / Tests 1099 passed。

### 未解決事項・F4b への申し送り

- (d) reached / Go: 依存先が `done` でも途中目標が reached になるまで依存する Task を dispatch しない判定、`ok` で Go、
  `auto_advance`（F3 の `PausePolicy` が揃ってから。`MilestoneSpec.pause_after` も同時に追加する）。
- (e) 案件 replan（`celeris.project-plan-delta/1`、`ProjectPlanProposed.supersedes`、version n+1）。今の `decide` は
  版ごとに 1 回きりで、差分の適用は無い。未決の提案がある案件への二重の `POST /plan {mode: milestones}` も未対処。
  `find_proposal` / inbox の `project_plan_summary` は plan タスクを総なめする（索引が無い）ので、複数版の前に見直す。
- (f) execution-plan/2 の `children`（委譲の検証、`child:<key>` 依存、部またぎは秘書への質問）。
- (g) 案件計画の無い既存案件の互換テスト（`legacy_project_keeps_linear_milestones`）。
- (h) GUI の案件ページの DAG・提案の承認 / 却下ボタン（API は揃った。受信箱の `DraftGroup` から `decide` を呼ぶ導線が要る。
  F3 が GUI を並行編集中なので F4a では触っていない）。
- reject の秘書への対話は commit 後に作るので、対話の作成だけが失敗すると「決定済みだが対話が無い」になりうる（エラーは返る）。

### 提案

- F4b の (h) で GUI から `decide` を呼ぶ際、受信箱の `DraftGroup` に `project_plan: {project_id, version}` を載せると、
  GUI が plan タスクを探さずに済む（今は `plan_summary` の文字列だけ）。

## Phase F3（途中確認）着手・再開（2026-09-26、branch `worktree-agent-a7a9562a0907d0beb`）

前任者（branch `worktree-agent-a83911fdfc96765ab`、未 commit）が (a)(e) 相当のデータ型骨組み
（`crates/task-core/src/pause.rs`、`model.rs`/`transition.rs` の下書き差分）を残したまま 12 時間 commit 0 件で
停止。本エージェントが引き継ぎ、**区切りごとに commit**する方針で再開する。ビルドは
`CARGO_TARGET_DIR=/var/lib/celeris/build-cache/cargo/agent-platform-f3pause`。

### F3(pause) checkpoint — 区切り 0（完了、commit 待ち）

区切り 0: `[execution] gate = "shadow"` のとき、`ExecutionGateDecision.source = human`
（`rule_id = human/explicit`）の compound を採用して planner run に進む修正（F5-1 dogfood で見つかった不具合、上の
「F5-1 dogfood の結果」節参照）。CoS のヒント（`source = hint`）と規則表（`source = policy`）は shadow では
従来どおり記録のみ。

- **条件**: shadow + human explicit compound → planner run（計画採用・WU 実行）になる。shadow + 規則表の
  compound（人の明示なし）→ 記録のみで atomic のまま実行される。
- **変更**: `crates/task-dispatch/src/dispatcher.rs::dispatch_one` の `is_planner_dispatch` 判定に
  `shadow_human_explicit_compound`（`gate == Shadow && decision.mode == Compound && decision.source == Human`）を
  追加し、`gate == On || shadow_human_explicit_compound` で planner run にする。ADR-0072「Phase F3（途中確認）
  実装時の逸脱・明確化」に 1 行追記。
- **実行したコマンド・出力の要点**:
  - `cargo test -p task-dispatch --lib -- shadow_gate_adopts_a_human_explicit_compound_decision shadow_gate_does_not_adopt_a_rule_based_compound_decision gate_on_compound_task_runs_a_planner_then_the_planned_work_units_in_order`
    → `test result: ok. 3 passed; 0 failed`。
  - `cargo fmt --all -- --check` → 差分なし（exit 0）。
  - `cargo test --workspace --no-fail-fast` → **FAILED 0**。1 回目の実行で `celeris --test instance_handoff`
    のプロセス全体が非 0 で終わったが、その binary だけを単体で再実行（`cargo test -p celeris --test
    instance_handoff`）すると 8 passed / 0 failed。2 回目のフル実行（ログをファイルに保存して確認）では
    FAILED 0（既知の高負荷時 flake、`docs/PROGRESS.md`「evaluator-flaky-tests」と同種。本 Phase の変更とは
    無関係な dispatcher/execution_gate 以外のクレート）。
  - `cargo clippy --workspace --all-targets -- -D warnings` → warning 0（exit 0）。
  - `UPDATE_SCHEMA=1 cargo test --workspace committed_schema_matches_generated` → 3 件 pass、再生成後の
    `git diff` 差分なし。
  - GUI（`cd gui && corepack pnpm@11.27.0`）: `gen:types` は `total_cache_read_tokens`（F5-1 で
    `ExecutionMetrics` に足された欄）が `gui/app/celeris/types.ts` に未反映だった既存のドリフトを検出
    （本 Phase の変更とは無関係。再生成のみで解消）。再生成後 `git diff --exit-code -- gui/app/celeris/types.ts`
    は差分ゼロを確認済み（このコミットで型を追随させた）。`typecheck` exit 0、`lint` exit 0（既存の info 2 件
    のみ）、`test` 1099 passed、`build` exit 0、`mobile-audit` → `routes=27 schemes=2 violations=0`。
- **未着手**: (a)〜(f) の残り全部（`PausePolicy` の型・NewTaskSpec/PATCH/CoS 配線、`PhaseGate`/`PhaseResume`
  遷移、途中報告、受信箱・通知、API・GUI）。前任者の下書き（`pause.rs`・`model.rs`/`transition.rs` の Trigger
  追加）は設計が妥当なので土台として使う予定。

### F3(pause) checkpoint — 区切り 1 (a)(e)（完了、commit 待ち）

`PausePolicy`（none / each_phase / after(phase)）の型と、採用時の解決。前任者の `pause.rs` 下書きのうち
`PauseSource`/`PausePolicy`/`resolve_pause_points` だけを取り込んだ（`PhaseResumeMode`/`PhaseReport` は
区切り 2（(b)）で足す）。

- **条件 (a)**: 人（`NewTaskSpec`/`PATCH /tasks/{id}`）と CoS（`create_task.pause_after`）が
  `pause_after` を書け、計画の採用（新規・replan）のときに `Event::PausePointsResolved` へ解決される。
  planner の出力には欄が無い（`ExecutionPlanSpec` に触れていない。書けば schema 違反になる既存の
  `deny_unknown_fields` のまま）。
  - 型: `crates/task-core/src/pause.rs`（`PauseSource`・`PausePolicy`・`resolve_pause_points`）。
    `crates/task-core/src/model.rs`: `TaskRouting.pause_after`/`pause_after_source`（既定で 1 バイトも
    変わらない）、`Event::PausePointsResolved { plan_id, phases, source }`。
  - 解決: `crates/task-ops/src/execution.rs::adopt_plan`/`replan` が `Task.routing.pause_after` を
    `validated.spec.phases` へ解決し、`ExecutionPlanned`（`extra_events`。`execution_plan_adopt` の
    store trait に `extra_events: Vec<Event>` を新設、`execution_plan_replan` は既存の `extra_events` を
    再利用）と同じトランザクションで書く。
  - 人: `crates/task-ops/src/add.rs::NewTaskSpec.pause_after`（出自 = `spec.provenance.origin` から
    `PauseSource::Human`/`Agent` を決める）。`crates/task-ops/src/edit.rs::TaskEdit.pause_after`（PATCH は
    管理系 = 人だけなので常に `Human`。`routing` が無い旧タスクにも新しく作る）。
  - CoS: `crates/task-core/src/console_action.rs::ConsoleAction::CreateTask.pause_after`
    （`Box<PausePolicy>`。`clippy::large_enum_variant` 対策、`workspace` と同じ理由）→
    `crates/task-ops/src/actions.rs::create_task_action` を経由。
  - 派生の機械的対応: `Event` に新 variant を足したことによる `crates/task-api/src/query.rs::EVENT_TYPES`
    （27→28、`"pause_points_resolved"`）と `event_type_name` の網羅性。`NewTaskSpec`/`TaskRouting` の
    構造体リテラルを持つ約 10 箇所（`crates/{celeris,celerisctl,task-dispatch,task-ops}/**`）に
    `pause_after`/`extra_events` の欄を足した（値は既定 `None`/`Vec::new()`。挙動は変えない）。
- **条件 (e)**: v1/atomic の計画（`phases` が空）では `pause_after` があっても無害（`resolve_pause_points`
  が空集合を返すだけ。`adopt_plan_resolves_no_pause_points_for_v1_plans` で確認）。
- **実行したコマンド・出力の要点**:
  - `cargo test -p task-ops --lib add:: edit:: actions:: execution::` → 全 pass（新規 7 テスト:
    `create_task_carries_pause_after_from_a_human_spec`、
    `create_task_defaults_pause_after_to_none_with_human_source`、
    `create_task_carries_pause_after_from_an_agent_spec_with_agent_source`、
    `edit_writes_pause_after_with_human_source_even_without_prior_routing`、
    `create_task_carries_pause_after_from_cos_with_agent_source`、
    `adopt_plan_resolves_pause_points_from_task_routing_for_v2_plans`、
    `adopt_plan_resolves_no_pause_points_for_v1_plans`。加えて `pause.rs` 自身の 4 テスト:
    `none_never_pauses`・`each_phase_excludes_the_last_phase`・`after_matches_by_key_or_kind`・
    `v1_and_atomic_plans_have_no_phases_so_nothing_resolves`）。
  - `cargo fmt --all -- --check` → 差分なし（1 回 `cargo fmt --all` で自動整形してから確認）。
  - `cargo clippy --workspace --all-targets -- -D warnings` → warning 0。
  - `cargo test --workspace --no-fail-fast` → 1 回目は 138+4 件が `assertion left==right` で FAILED
    （**本 Phase のバグではない**: `df -h /` がルート 99% 使用・空き 4.5G で、`agent-platform-f4a` の
    target が 43G を占めていた。既知の「ルートディスク満杯」障害と同型。自分の
    `agent-platform-f3pause` の target 29G を `rm -rf` して 34G 空きに戻し、`cargo build` からやり直して
    再実行 → **FAILED 0**（82 の test binary、`test result: ok` 82 件）。他の同時稼働エージェントの
    target には触れていない）。
  - `UPDATE_SCHEMA=1 cargo test -p task-api --lib schema::tests::committed_schema_matches_generated` と
    `-p task-core --lib store::tests::event_row_schema_matches_committed` を個別に再生成（`Event`
    variant 追加のため。ADR-0074 の他の Phase と同じく、schema 一致テストは対象を絞って
    `UPDATE_SCHEMA=1` を通す必要がある）。再生成後の 3 schema（`docs/api/v1/api-v1.schema.json`・
    `docs/api/v1/event.schema.json`・`docs/protocol/worker-protocol.schema.json`）はいずれも
    `PausePolicy`/`PauseSource`/新フィールドの追加だけ（299 行の加算、削除ゼロ）。
- **持ち越し**: GUI の `gen:types` はこの区切りでは実行していない（区切り 4 でまとめて GUI 側を作る
  ときに、`api-v1.schema.json` の差分を含めて再生成する）。`PUT /tasks/{id}/execution/pause-after`
  （ADR D2.1 が触れる、計画採用後に pause_after だけを直す専用エンドポイント）は実装していない
  （§6 F3 の受け入れ条件 (a) は `NewTaskSpec`/`PATCH`/CoS とだけ書いており、専用 PUT は明示要求されて
  いないため。必要なら次の一手として追加する）。


### F3(pause) checkpoint — 区切り 2 (b)（完了）

停止点の工程の統合の後で `PhaseGate` → Blocked（reason `awaiting_human`、attempts 不変）、決定的な途中報告
`Event::PhaseReported` と `artifacts/phase-reports/<n>-<phase>.md`（`ArtifactProduced`）。前任者の未 commit の下書き
（8 ファイル）はそのままビルドが通ったので土台にし、受け入れテストを足した。

- **条件 (b)**: `crates/task-dispatch/src/dispatcher.rs::finish_phase_integration` が、次の工程があり、この工程が
  有効な計画の `PausePointsResolved.phases` に含まれるとき `Continue{advance}` の代わりに `Trigger::PhaseGate` を
  適用し、`PhaseIntegrated`・`ArtifactProduced`・`PhaseReported` を同じトランザクションで書く。途中報告は
  `build_phase_report`（checkpoint・統合結果・`git diff --stat`・次の工程・`quota_summary_line`・成果物）で組み、
  LLM は使わない。型は `crates/task-core/src/pause.rs`（`PhaseReport`・`PhaseResumeMode`・`truncate_phase_report`・
  `format_wall_ms`・`quota_summary_line`）、遷移は `crates/task-core/src/transition.rs`（`PhaseGate`/`PhaseResume`）。
  ADR-0074 末尾に「Phase F3（途中確認）実装時の逸脱・明確化」を追加（次の工程の WU は ready になるが Task が
  Blocked なので走らない、など）。
- **実行したコマンド・出力の要点**:
  - `cargo test -p task-dispatch --lib -- pause_after_design_blocks_with_awaiting_human` → 1 passed（Blocked、
    最後の遷移 `awaiting_human`、`worker_done` 無し、attempts 不変、replay・WU 照合の差分 0、`PhaseReported` 1 件、
    `phase-reports/1-design.md` の本文、build の WU は run 0）。
  - `cargo fmt --all -- --check` → exit 0。`cargo clippy --workspace --all-targets -- -D warnings` → exit 0。
  - `UPDATE_SCHEMA=1 cargo test --workspace committed_schema_matches_generated`（＋ `event_row_schema_matches_committed`）
    → pass。`api-v1.schema.json`・`event.schema.json` に `PhaseReported`/`PhaseReport` が加算。
    `gui: corepack pnpm@11.27.0 gen:types` で `types.ts` を追随、`typecheck` exit 0。
  - `cargo test --workspace --no-fail-fast` → exit 0、93 binary、2448 passed、FAILED 0。
  - 環境: 着手時にルートが 100%（空き 0）で `No space left on device`。自分の `agent-platform-f3pause` の target
    だけを消して作り直した（`CARGO_INCREMENTAL=0`・`CARGO_PROFILE_{DEV,TEST}_DEBUG=line-tables-only` で容量を抑える）。


### F3(pause) checkpoint — 区切り 3 (c)(d)（完了）

受信箱の `PhaseCheckpoint`、`NotificationKind::PhaseCheckpoint`、`POST /tasks/{id}/execution/phase-gate`
（continue / replan / withdraw）、`celerisctl execution phase-gate`、CoS のプリアンブルの 1 行。
判定と操作は新しい `crates/task-ops/src/phase_gate.rs`（`is_awaiting_human`・`latest_phase_checkpoint`・
`phase_gate`・`phase_replan_instruction`）に集めた（F4a が並行編集中の `inbox.rs` の drafts 側と
`dispatcher.rs` の案件計画 run には触れていない）。ADR-0074 末尾の逸脱・明確化 5〜11 を追記。

- **条件 (c)**: 受信箱の `attention` に `PhaseCheckpoint`、`questions` には出ない。通知は `PhaseCheckpoint` が
  1 回だけで `QuestionBlocked` は鳴らない。
  - `cargo test -p celeris --lib notify::` → 7 passed（`notify::tests::phase_checkpoint_is_not_a_question`: 途中確認の
    Task は PhaseCheckpoint 1 件・QuestionBlocked 0 件、質問の Task は QuestionBlocked 1 件、2 回目の tick で 0 件）。
    `cargo test -p celeris --test notify` → 25 passed（既存の通知の振る舞いは不変）。
  - `cargo test -p task-ops --lib` → 339 passed（`inbox_phase_checkpoint_is_attention_not_a_question`、
    `phase_gate::tests::*` 6 件を含む）。
- **条件 (d)**: `continue` / `replan`（note 必須）/ `withdraw` → `PhaseResume{Continue}` / `PhaseResume{Replan}` →
  planner / `Cancel`。awaiting_human 以外は 409、`Answer` は 409。
  - `cargo test -p task-dispatch --lib -- phase_gate_ pause_after_design` → 3 passed
    （`phase_gate_continue_resumes_the_next_phase`: reason `phase_continue`、attempts 不変、build が走って done、
    途中報告は 1 回、note が `Answered` に残る、replay 差分 0、2 回目は InvalidState。
    `phase_gate_replan_requires_a_note`: note 無し・空白は Validation で Blocked のまま、`Answer` は InvalidState、
    note ありで `phase_replan` → 起こした理由「人の指示: build を 2 つに分ける」→ role planner の run が起き、
    その前に build の WU は走らない）。
  - `cargo test -p task-api --test execution phase_gate` → 3 passed（401・409〈質問の Task、answer〉・422
    〈`field = "note"`〉・200 continue・2 回目 409・withdraw → cancelled・受信箱の `phase_checkpoint`・
    `GET /tasks/{id}/execution` の `phase = awaiting_human` と `phase_checkpoint`）。
  - `cargo test -p celerisctl --bin celerisctl execution` → 5 passed。CoS のプリアンブル
    （`crates/task-worker/src/preamble.rs::actions_instructions`）に `pause_after` の 3 行、`cargo test -p task-worker` 全 pass。
- **ゲート**: `cargo fmt --all -- --check` exit 0。`cargo clippy --workspace --all-targets -- -D warnings` exit 0。
  `UPDATE_SCHEMA=1 cargo test --workspace committed_schema_matches_generated` pass（`api-v1.schema.json` に
  `PhaseGateRequest`・`PhaseCheckpoint`・`phase_gate`・`awaiting_human`・`phase_checkpoint` が加算）。
  GUI: `gen:types` で `types.ts` を追随し、新しい union 値の表示名（`phase_checkpoint`・`awaiting_human`・
  `phase_gate`）を足して `typecheck` exit 0（画面は区切り 4）。
  `cargo test --workspace --no-fail-fast` → 93 binary、2459 passed、3 failed。3 件はいずれも本変更と無関係な
  高負荷時の flake（`celeris releases::tests::promot*` 2 件は `cargo test -p celeris --lib releases::` 単体で
  21 passed、`e2e --test provider_admin_scenarios` は単体で 2 回とも 3 passed）。


### F3(pause) checkpoint — 区切り 4 (f)（完了）

GUI: タスク詳細の Execution 節に途中報告と 3 つのボタン、受信箱の新しい項目。

- **条件 (f)**:
  - `gui/app/components/ExecutionSection.tsx::PhaseCheckpointPanel`: `execution.phase_checkpoint` があるとき、
    見出し（工程「<title>」まで進みました）・次の工程・途中報告の節（済んだ工程 / この工程の WU / 統合 / 差分 /
    quota / 成果物。空の節は出さない）・途中報告の Markdown へのリンク（`/files/tasks/:id/artifacts/:idx`）・
    「人の指示」欄と 3 つのボタン（続ける / 計画を立て直す（replan） / 取り下げる）。送り先は `/tasks/:id` の
    action の `intent=phase_gate`（`gui/app/celeris/tasks-admin.server.ts::phaseGateTask` →
    `POST /tasks/{id}/execution/phase-gate`。GUI は検証しない。422 の `note` の文言は欄の下に出す）。
    取り下げの前に取り込み（merge / PR）する旨を添えた。表示用の純粋関数は `gui/app/lib/task-execution.ts`。
  - `gui/app/routes/inbox.tsx`: `attention` の `phase_checkpoint`（「工程「<title>」まで進みました（n/m 工程）。
    確認を待っています。次: …」と、タスク詳細へのリンク）。この項目には取り消しボタンを出さない（判断は
    タスク詳細の 3 つのボタンで）。
  - 型の追随で `NotificationKind.phase_checkpoint`・`ExecutionPhase.awaiting_human`・`Action.phase_gate` の表示名。
  - mobile-audit の偽データ（`gui/scripts/lib/celeris-fixture.mjs`）に途中報告つきの Execution 節と受信箱の
    `phase_checkpoint` 項目を足した（1 回目の実行で暗色テーマのリンクの contrast 4.26:1 を検出 → 本文色＋下線に直した）。
- **実行したコマンド・出力の要点**（`cd gui && corepack pnpm@11.27.0 …`）:
  - `gen:types` → `git diff --exit-code -- app/celeris/types.ts` 差分ゼロ。
  - `typecheck` exit 0。`lint` exit 0（既存の info 2 件のみ）。
  - `test` → 72 files / 1106 passed（新規: `task-execution.test.ts` の途中確認 4 件、
    `tasks.manage.action.test.ts` の `phaseGateTask` 3 件）。
  - `build` exit 0。`mobile-audit` → `routes=27 schemes=2 violations=0`。

## Phase F3（途中確認）完了（2026-09-27、branch `worktree-agent-a7a9562a0907d0beb`）

ADR-0074 D2 / §6 F3 (a)〜(f) を区切り 0〜4 で実装した（各区切りの詳細は上の「F3(pause) checkpoint」節）。
commit: 22be6d8（区切り 0）、7b926b8（区切り 1 (a)(e)）、8b39f8b（区切り 2 (b)）、0ee84c1（区切り 3 (c)(d)）、
7845d1a（区切り 4 (f)）、本節の commit（区切り 5 = 全体ゲート）。main への merge / push はしていない。

- **受け入れ条件と証拠**:
  - (a) `PausePolicy` を人（`NewTaskSpec`/`PATCH`）と CoS（`create_task.pause_after`、プリアンブルに 3 行）が書け、
    採用（新規・replan）で `PausePointsResolved`。planner の出力には欄が無い → 区切り 1 の 7 テスト。
  - (b) 停止点の工程の統合の後で `PhaseGate`（Blocked・`awaiting_human`・attempts 不変）、`PhaseReported`、
    `phase-reports/<n>-<phase>.md` → `dispatcher::tests::pause_after_design_blocks_with_awaiting_human`。
  - (c) 受信箱 `PhaseCheckpoint`（questions に出ない）、`NotificationKind::PhaseCheckpoint` が 1 回だけ・
    `QuestionBlocked` は鳴らない → `notify::tests::phase_checkpoint_is_not_a_question`、
    `inbox::tests::inbox_phase_checkpoint_is_attention_not_a_question`。
  - (d) continue / replan（note 必須）/ withdraw → `PhaseResume{Continue}` / `PhaseResume{Replan}` → planner /
    `Cancel`、awaiting_human 以外は 409、`Answer` は 409 →
    `dispatcher::tests::phase_gate_continue_resumes_the_next_phase`、`phase_gate_replan_requires_a_note`、
    `task-api --test execution phase_gate_*` 3 件、`task_ops::phase_gate::tests` 6 件、`celerisctl execution phase-gate`。
  - (e) v1 / atomic で無害 → `adopt_plan_resolves_no_pause_points_for_v1_plans`。
  - (f) GUI の途中報告と 3 つのボタン、受信箱の項目 → 区切り 4。
- **全体ゲート（区切り 5、最終 commit の直前の作業ツリー）**:
  - `cargo fmt --all -- --check` → exit 0。
  - `cargo clippy --workspace --all-targets -- -D warnings` → exit 0。
  - `cargo test --workspace --no-fail-fast` → exit 0、93 binary、2462 passed、FAILED 0。
  - `UPDATE_SCHEMA=1 cargo test --workspace committed_schema_matches_generated` → pass、再生成後 `git status` 差分 0。
  - `cd gui && corepack pnpm@11.27.0 gen:types`（`types.ts` 差分ゼロ）`&& typecheck && lint && test && build &&
    mobile-audit` → すべて exit 0、test 72 files / 1106 passed、`routes=27 schemes=2 violations=0`。
- **未解決事項**:
  1. 途中確認で `replan` を選んだが `max_replans` を使い切っている場合、人の指示は `answers` に残したまま次の工程へ
     進む（`tracing::warn` のみ。ADR-0074 逸脱・明確化 6）。人に見える形（質問に落とす等）にするかは未決。
  2. `PUT /tasks/{id}/execution/pause-after`（計画採用後に pause_after だけ直す専用エンドポイント）は未実装
     （`PATCH /tasks/{id}` の `pause_after` で代用可能。ただし既に解決済みの `PausePointsResolved` は次の replan まで
     変わらない）。
  3. withdraw の reason は既存の `cancel`（ADR D2.4 の `withdrawn` とは異なる。逸脱・明確化 7）。
  4. 全体テストで高負荷時の flake を 3 件観測（`celeris releases::tests::promot*`、`e2e provider_admin_scenarios`）。
     いずれも単体再実行で pass、本変更とは無関係。
  5. 実機（本番 daemon）での途中確認の往復は未確認（F5 の dogfood で `pause_after = each_phase` の Task を 1 件流して
     確かめる）。
- **提案**: F5-2 の dogfood で、`execution: compound` + `pause_after: {"mode":"after","phases":["design"]}` の Task を
  GUI から作り、Discord 通知 1 通 → タスク詳細で途中報告を読む → 「続ける」の往復を 1 回確認する。


## Phase F5-1 dogfood やり直し（2026-09-27、task 01M3HJYF9ZN09GE0ZFT9VT4V8J）

F1 の WU ごとの lane と planner の効果を E6 と比較する dogfood のうち、独立した 3 成果を実施。

- `task-api::query::EVENT_TYPES` に F4a の `project_plan_proposed` / `project_plan_decided` を追加。F3 の
  `phase_reported` / `pause_points_resolved` とあわせ、`types` クエリ語彙テストで4種類を確認する。
- dispatcher の cooldown/backoff 判定にテスト用 UTC・monotonic 時計を導入し、レビュー所有権テストは tick と
  task yield で verdict 保存境界を同期する。provider retry は Adapter 完了通知で同期する。
- `PROGRESS.md` の詳細を `docs/progress/` へ分割し、本ファイルは現在地と目次にする。

全体ゲートの結果・残課題は、この run の成果物ディレクトリにある `report.md` を参照。

## release 90d418e2036a の昇格（F3 途中確認 + F4a 案件計画前半、2026-09-27 13:17Z）と release ビルドのローカル化

- F3-pause（Opus 引き継ぎ）: shadow でも人の明示 compound を採用（区切り 0）、PausePolicy、PhaseGate → Blocked(awaiting_human)、PhaseReported と phase-reports、受信箱の PhaseCheckpoint、continue / replan / withdraw の API・CLI・GUI。F4a（Opus 引き継ぎ）: is_milestone_task と途中目標の 1:1、project-plan/1 の schema と CoS の提案、1 トランザクションの承認 / 却下（API 202 / 422 / 404 / 409、celerisctl projects plan approve|reject）。main 90d418e。ゲート: fmt 0 / test FAILED 0 / clippy 0 / GUI typecheck・lint・test 1106 件・gen:types 差分ゼロ・build・mobile-audit 0。
- release ゲートの繰り返し失敗の原因: NFS 移行後、`~/.local/celeris/releases/.build` と `.cargo-target`（NFS 上）で dispatcher のタイミング依存テストが load 1 でも落ちた（同じテストは単体では通る）。`.build` / `.cargo-target` を `/var/lib/celeris/release-build/` への symlink にしてローカル LVM で回したところ 1 回で通過。verify 全 true（N-1 = e3465764475c も ok）。in-flight 0 でライブ昇格。
- Sonnet の週次制限（9/28 23:00 UTC まで）: 再起動した F3-pause / F4a も途中で停止 → 同じ worktree を Opus に引き継がせて完了。F4b（(d) reached/Go、(e) 案件 replan、(f) children、(g) 互換、(h) 案件ページ DAG）を Opus で開始。
- F5-1 dogfood をやり直し（タスク 01M3HG7VV6A2HRHTNXWPDS9051、explicit compound。内容: EVENT_TYPES の補完、フレークテスト 5 件の決定化、PROGRESS.md の分割）。

## F5-1（やり直し）の発見: planner が Plan Mode で成果物を書けず atomic に倒れた（2026-09-27）

- 事実: gate=shadow でも人の明示 compound は採用されるようになり planner run は起きた（`ExecutionGated source=human`）。しかし planner run（claude-code、標準 lane）は 2 回とも `claude exited without …/artifacts/result.json` で失敗し、`atomic/planner-invalid`（`planner_retry_exhausted`）で atomic に倒れた。stdout を見ると、`--permission-mode plan`（E4b で配線した `[execution.planner].permission_mode` の既定 `"plan"`）により claude-code が Plan Mode に入り、`Write` が plan ファイル以外に書けず、非対話では `ExitPlanMode` も使えないため `execution-plan.json` / `result.json` を書けなかった（2 回目の run は自分でそう報告している）。
- 修正: 既定を `"bypassPermissions"`（アダプタ既定と同じ）に変更（`config.rs`）。planner は成果物を書く役なので Plan Mode は使わない。
- 副次の発見: claude-oauth（Max）の 7 日枠が utilization 0.91（`rate_limit_event`、9/28 23:00 UTC にリセット）。私の実装エージェント（Sonnet）の週次制限と同じ枠。planner run 2 回目は `claude-sonnet-5` で走った（残量による lane の降格）。
- F5-1 は 3 回目のやり直しが要る（release に本修正を含めてから）。それまでの F5-1（やり直し）タスク自身は atomic で done / reviewing まで進んでいる。
## Phase F4b「案件レベルの計画（後半）: reached / Go、案件 replan、planner の children、互換、案件ページの DAG」（着手 2026-09-27、branch `worktree-agent-a46b8a640d3df7cda`）

ADR-0074 D3 の (d)〜(h)。作業は worktree の中（main へ merge / push しない）。ビルドは
`CARGO_TARGET_DIR=/var/lib/celeris/build-cache/cargo/agent-platform-f4b CARGO_INCREMENTAL=0`。

### F4b checkpoint 1: (d) 途中目標の Go（reached まで依存するマイルストーンを dispatch しない）（完了 2026-09-27）

- **条件**: 依存先のマイルストーン Task が `done` でも、その途中目標（案件計画のもの）が `reached` になるまで依存する
  マイルストーン Task は dispatch しない。ADR-0038 の `ok` で `reached` → Go が開く。案件の `auto_advance = true` なら `done` で進む。
- **実装**:
  - migration 0028（`SCHEMA_VERSION` 27 → 28）: `projects.auto_advance INTEGER NOT NULL DEFAULT 0`、`milestones.plan_key TEXT`。
    `Project.auto_advance`（`serde(default)`）、`Milestone.plan_key`（`Option`）。`TaskStore::project_set_auto_advance` /
    `milestone_set_plan_key`。`PATCH /projects/{id}` に `auto_advance`。
  - `task_ops::project_plan::propose` が作る途中目標に `plan_key`（計画の key）を結ぶ。
  - `SqliteStore::ready_tasks`（ADR-0044 D6 の一時停止の判定と同じ場所）: マイルストーン Task 同士の依存で、依存先の途中目標が
    `plan_key` ありで `reached` でなく、案件の `auto_advance = 0` なら ready に出さない（`milestones_awaiting_go_locked`）。
  - `task_ops::milestone_review::decide`: `plan_key` のある途中目標は `decide_planned`（`ok` = `reached` だけ。次の承認・分解 run は
    起こさない。`ng` = `redesigned` + 案件 replan の計画 run）。`record_proposal` と dispatcher の `absorb_milestone_proposal` は
    案件計画の途中目標に触れない。
  - `MilestoneSpec.pause_after`（F3 の `PausePolicy`。F4a の申し送り）。
- **実行したコマンド・出力の要点**:
  - `cargo test -p task-dispatch --lib dependent_milestone_waits_for_reached_not_done` → 1 passed（survey done 後も poc は ready で
    WorkerStarted 0、`ok` 後に poc done。`auto_advance = true` では survey done で poc も done、途中目標は reached でないまま）。
  - `cargo test -p task-core -p task-ops -p task-api -p celeris --no-fail-fast` → 全 suite 0 failed（task-core 423 passed、task-ops 351 passed）。
  - `UPDATE_SCHEMA=1 cargo test -p task-core schema` / `--workspace committed_schema_matches_generated` → ok
    （`event.schema.json`・`project-plan.schema.json`・`api-v1.schema.json` を再生成、`project-plan-delta.schema.json` を新規）。
  - `cargo clippy --workspace --all-targets -- -D warnings` → warning 0。`cargo fmt --all -- --check` → 差分ゼロ。

### F4b checkpoint 2: (e) 案件 replan（差分・同じ承認・二重依頼の防止）（完了 2026-09-27）

- **条件**: 案件 replan の差分（dispatch 前のものだけ変えられる、`cancel` は明示）が F4a と同じ `decide` を通る。承認までは現行の
  計画で動く。同じ案件への二重の計画依頼を防ぐ。
- **実装**:
  - `task_core::project_plan`: `celeris.project-plan-delta/1`（`ProjectPlanDelta{base_version, rationale, add, modify, remove, cancel}`、
    `MilestoneModify`〈書いた欄だけ〉、`docs/protocol/project-plan-delta.schema.json`）、純粋関数 `validate_delta`（schema・
    `base_version` = 現行の承認済み版・空でない・key の実在と重複・`modify`/`remove` は dispatch 前だけ・`cancel` は終端でないもの・
    `add` の key 衝突・当てた後の全体を `validate`）、`MILESTONES_REPLAN_LABEL` / `is_milestones_replan_task`。
  - `Event::ProjectPlanProposed.delta`（`Option`、追加のみ）。差分の提案でも `plan` / `milestones` は**当てた後の全体**を入れる。
  - `task_ops::project_plan`: `plan_state`（plan タスクの events から全版・現行〈最新の承認済み〉・未決・次の版番号）、
    `start_milestones` は動いている計画 run（提案前）か未決の提案があれば `OpsError::ProjectPlanInFlight`（API 409
    `project_plan_in_flight`）、承認済みの計画があれば `start_replan`（replan の印、goal に `base_version` と各節点の状態・変更可否）。
    `propose_delta`（`add` の分だけ proposed / draft を作り、modify・remove・cancel は承認まで何も変えない）、`decide` の差分の分岐
    （承認時に今の状態で検証し直し、古ければ `OpsError::ProjectPlanStale`〈API 409 `project_plan_stale`〉で何も書かない。
    新しい `TaskStore::project_plan_apply` が 途中目標の状態・題名 → Task の書き換え〈draft/ready で lease 無しのものだけ〉→
    `Accept`/`Cancel` → `ProjectPlanDecided` を 1 トランザクションで。却下は `add` の分だけ redesigned / cancelled）。
    `add::build_task_with_roles`（modify の組み立て直しに同じ規則を使う）。
  - `task_ops::milestone_review::decide_planned` の `ng` が `start_replan` を起こす（D3.4 の起点 (a)）。
  - dispatcher `finish_project_plan_run`: replan の run は差分を読み `validate_delta_against_store` → `propose_delta`。
  - `task-worker`: `build_project_replan_prompt`（差分の書き方・規則・schema）。
  - 受信箱: `DraftGroup.project_plan{project_id, version, supersedes}`（F4a の提案）。未決の提案は `plan_state` で引き、`add` の無い
    差分も 1 まとまり（drafts 空）で出る。`plan_summary` に `[add]` の印と modify / remove / cancel の key。
- **実行したコマンド・出力の要点**:
  - `cargo test -p task-ops project_plan::` → 16 passed（新規 `project_replan_delta_cannot_modify_a_started_milestone`〈running の survey の
    modify / remove は "already dispatched" で拒否、cancel の明示は通る、poc の modify と add は v2 の提案で承認まで poc は不変・survey は
    running のまま、未決の間の二重依頼は ProjectPlanInFlight、承認で poc の題名・途中目標の題名が変わり paper が ready、提案後に対象が
    dispatch されると承認は ProjectPlanStale で無変更、却下は現行を変えない〉、`project_replan_cancel_applies_cancel_to_a_running_milestone_on_approval`）。
  - `cargo test -p task-core project_plan::` → 新規 `delta_modify_and_remove_only_touch_undispatched_milestones` /
    `delta_rejects_stale_base_unknown_keys_collisions_and_dangling_dependencies` / `committed_delta_schema_matches_generated` ok。
  - `cargo test -p task-dispatch --lib replan_run_proposes_a_delta_and_rejects_modifying_a_started_milestone` → ok（偽アダプタの replan run）。
  - `cargo test -p task-api --test project_plan` → 12 passed（新規 `a_second_milestones_plan_request_is_409_and_auto_advance_round_trips`）。
  - `cargo test -p task-ops inbox::` → 16 passed。`cargo test -p task-worker --lib replan` → ok（`project_replan_task_gets_the_delta_prompt`）。
  - `cargo test --workspace --no-fail-fast` → **2501 passed / 0 failed / 5 ignored**。`cargo clippy --workspace --all-targets -- -D warnings` → 0。
    `cargo fmt --all -- --check` → 差分ゼロ。`UPDATE_SCHEMA=1 …` で `api-v1.schema.json` 再生成、`gui` の `gen:types` → `types.ts` 再生成、`typecheck` exit 0。
- **既知の限界**: D3.4 の起点 (c)（マイルストーン Task の failed / 取り下げで自動的に replan を起こす）は入れていない（人の依頼 =
  `POST /plan {mode: milestones}` と途中目標の `ng` だけ）。`propose` / `propose_delta` の作成は 1 トランザクションではない（F4a と同じ）。

### F4b checkpoint 3: (f) planner の `children` → 委譲の子 Task、`child:<key>` の依存（完了 2026-09-27）

- **条件**: execution-plan/2 の `children` が既存の委譲の検証を通って子 Task になり、`child:<key>` の依存で WU が待つ。部またぎは
  秘書への質問。planner のプロンプトに children の書き方。
- **実装**:
  - `task_core::execution_plan`: `ExecutionChildSpec{key, title, objective, acceptance, genre, skills, features, depends_on}`
    （`deny_unknown_fields`、`assignee`/`tier`/`model` なし）で `ExecutionPlanSpec.children` を型付け。v1 では空のまま（`NonEmptyChildren`）、
    v2 では件数（`max_children` 既定 8）・key・子同士の依存と循環・受け入れ条件を検証。WU の `depends_on` の `child:<key>` は既知の子だけ。
    `CHILD_DEP_PREFIX`、`child_label(key)`（子 Task の印 `child-<key>`）、`newly_ready_with(units, external_done)`。
    `docs/protocol/execution-plan.schema.json` 再生成。
  - `task_ops::delegate::plan_children`: 子を `DelegateTask` に写して既存の `plan_delegation`（深さ・件数・木の run 数・`validate_each`・
    作業場所の解決）に通す。1 件でも拒否なら `Err`。部をまたぐかは、担当になるはずのノード（`matching::decide`）の部と親の担当の部を
    比べ、`cross_authorization` が Pending なら `NeedsAuthorization`（`CrossDepartment::question()` の形）、Denied なら `Err`。
  - `TaskStore::execution_plan_adopt_delegating`（計画・WU・events と子 Task〈Created → Accept〉・`Event::Delegated{run_id: <planner run>}`
    を 1 トランザクション）、`task_ops::execution::adopt_plan_with_children`。
  - dispatcher: planner の完了で `plan_children` を通す（`Err` → 既存の retry / give-up、`NeedsAuthorization` → `WorkerQuestion` と
    `approvals` の行〈答えれば planner が同じ子で再試行し、認可済みとして通る〉）。`wu_dispatch_gate` の先頭で `resolve_child_dependencies`
    （子が done なら `pending → ready`〈reason `child_done`〉、failed / cancelled なら `pending → blocked(dependency_failed)` → D17 の replan、
    未完了の子を待つだけなら Skip）。
  - `task-worker`: v2 の planner プロンプトに「#### Child tasks」節（別の deliverable のときだけ、`child:<key>` の待ち方）。
- **実行したコマンド・出力の要点**:
  - `cargo test -p task-dispatch --lib planner_children_become_delegated_child_tasks` → ok（子 1 件が `child-lit` の印・ready → done、
    `Delegated{run_id: planner run}`、WU a は `child_done` で ready、Task done）。
  - `cargo test -p task-ops --lib plan_children` → ok（`plan_children_runs_the_delegation_checks_and_asks_before_crossing_departments`: 同じ部は通る、
    別の部は `cross-department: engineering -> research` の質問、once で通る、denied で拒否、木の深さの上限で拒否）。
  - `cargo test -p task-core execution_plan` → 新規 `children_are_v2_only_and_child_dependencies_must_be_known` ok。
  - `cargo test --workspace --no-fail-fast` → 1 件失敗（v1 の planner プロンプトが schema の説明文に `child:<key>` を含むため、
    テストの否定の判定を節見出しに直した）→ 修正後 `cargo test -p task-worker --lib` ok。`cargo clippy …` 0、`cargo fmt --check` 差分ゼロ、
    `UPDATE_SCHEMA=1 …` で `execution-plan.schema.json` / `event.schema.json` / `api-v1.schema.json`、`gen:types`、`typecheck` exit 0。
- **既知の限界**: replan（既に計画がある Task の planner run）で新しい子を足すことはしない（既存の子の key だけ許す。不正な試行として扱う）。
  `repos` は `ExecutionChildSpec` に入れていない（子は既存の委譲と同じく 親 > 案件の primary を継ぐ）。部またぎの質問の e2e は
  `plan_children` の単体テストで代える。

### F4b checkpoint 4: (g) 案件計画の無い既存の案件の互換（完了 2026-09-27）

- **条件**: 案件計画の無い既存の案件の挙動（直列の途中目標、ADR-0038 の `ok` の旧い意味）が変わらない。
- **実装**: 追加の実装は無し（(d) の Go の判定は `milestones.plan_key` のある途中目標だけ、`ok` の新しい意味も `plan_key` のある途中目標だけ）。
  テストで固定する。
- **実行したコマンド・出力の要点**:
  - `cargo test -p task-ops --lib milestone_review` → 6 passed。新規 `legacy_project_keeps_linear_milestones`（`plan_key` の無い途中目標に属する
    案件直下の Task 同士の依存は、依存先 done で同じ途中目標でも別の途中目標でも ready に出る〈reached を待たない〉、`ok` は reached +
    最新の提案を approved → in_progress にして `kind = plan`〈案件計画の印なし〉の分解 run を起こす、案件計画の版は 0）、
    `planned_milestone_ok_only_reaches_and_ng_starts_a_project_replan`（案件計画の途中目標の `ok` は reached だけ、`ng` は redesigned +
    replan の計画 run〈理由が goal に入る〉）。
  - `cargo clippy -p task-ops --all-targets -- -D warnings` → 0。

### F4b checkpoint 5: (h) GUI の案件ページの DAG（完了 2026-09-27、commit 3c623ce / 4ae0425）

- **条件**: 案件ページに DAG（節点 = マイルストーン Task の状態・進み・quota・止まっている理由）、提案中の計画の承認 / 却下、
  節点ごとの ok / 議論 / ng。モバイル幅は縦の一覧。`gen:types` 差分ゼロ、`mobile-audit` 違反 0。
- **実装**: `task_ops::project_plan::dag_view`（`ProjectPlanDagView{current_version, nodes, pending}`、節点に途中目標・Task の状態、
  WU done/total〈統合 WU 除く〉、子 done/total、quota〈`summarize_execution_metrics`〉、`stop_reason`〈awaiting_human / question /
  failed / awaiting_go / paused〉、提案の節点に `change`〈add / modify / remove / cancel〉）。`GET /projects/{id}` の `project_plan`。
  GUI: `app/components/ProjectPlanDag.tsx`（層ごとに横並び・モバイルは同じ DOM の縦の一覧、節点の「判定」で ADR-0038 の
  `MilestoneReviewPanel` をその場に出す、提案は破線の枠 + 承認 / 却下〈却下は理由必須〉）、`app/lib/project-plan.ts`、
  `decideProjectPlan`（`POST /projects/{id}/project-plan/{version}/decide`）、`startProjectPlan` の `mode`（「計画を見直す」）、
  Flash の文言、mobile-audit の偽 celeris に `project_plan` の例。案件計画の途中目標は「途中目標」一覧から外す。
- **実行したコマンド・出力の要点**:
  - `cargo test -p task-api --test project_plan` → 13 passed（新規 `project_detail_carries_the_plan_dag_and_the_pending_proposal`）。
  - `cd gui && corepack pnpm@11.27.0 gen:types` → `types.ts` 差分ゼロ、`typecheck` exit 0、`lint` エラー 0（既存の info 2 件のみ）、
    `test` → Test Files 73 passed / Tests 1113 passed（新規 `test/unit/project-plan.test.ts` 7 件）、`build` ok、
    `mobile-audit` → `routes=27 schemes=2 violations=0`（project-detail に DAG と提案が出た状態で）。

### Phase F4b の全体ゲート（2026-09-27）

- `cargo fmt --all -- --check` → 差分ゼロ。
- `cargo clippy --workspace --all-targets -- -D warnings` → warning 0。
- `cargo test --workspace --no-fail-fast` → exit 0、合計 **2506 passed / 0 failed / 5 ignored**。
- `UPDATE_SCHEMA=1 cargo test -p task-core schema && UPDATE_SCHEMA=1 cargo test --workspace committed_schema_matches_generated` → FAILED 0、
  再生成後の `git status` に差分なし。
- `cd gui && corepack pnpm@11.27.0 gen:types`（差分ゼロ）`&& typecheck && lint && test && build && mobile-audit` → すべて exit 0、違反 0。

### 未解決事項・F5 への申し送り

- migration 0028（`SCHEMA_VERSION` 28）を含む。本番へ出すと古いバイナリはこの DB を開けない（`SchemaTooNew`）。
  並行している別 branch が 0028 を使っていれば番号の付け替えが要る。
- D3.4 の起点 (c)（マイルストーン Task の failed / 取り下げで自動的に replan）は未実装（人の依頼と `ng` だけ）。
- replan の planner run で新しい children を足すことはしない。children の部またぎの質問は単体テストで確かめただけ（dispatcher の e2e 無し）。
- `propose` / `propose_delta` の作成は 1 トランザクションではない（F4a と同じ）。提案中に daemon が止まると途中まで作った
  proposed の途中目標・draft が残りうる（提案の event が無いので `plan_state` には出ない）。
- 途中目標の状態は dispatch で `in_progress` に上げていない（`approved` のまま → レビュー → `reached`）。GUI は Task の状態を併記する。
- F5 dogfood で確かめたいこと: 秘書の案件計画 / 差分のプロンプトで実際に妥当な DAG・差分が出るか、`awaiting_go` の案件で人の `ok` から
  依存するマイルストーンが自然に動き出すか、planner の children が「別の deliverable のときだけ」書かれるか。

### 提案

- `docs/celeris-api-v1.md` に `GET /projects/{id}` の `project_plan`、`PATCH /projects/{id}` の `auto_advance`、
  `POST /projects/{id}/plan` の 409 `project_plan_in_flight`、decide の 409 `project_plan_stale` を追記する（本 Phase では生成 schema のみ更新）。

<a id="f5-1-review-repair"></a>

## Phase F5-1 dogfood 再レビュー対応（2026-09-27、run 01M3HNR5TKV1YBVGF52TRYNZ5M）

前回レビューで、worktree 2テストが固定回数の sleep に依存し、レビュー所有権テストも
`yield_now` では完了を保証しないと指摘されたため、完了同期を修正した。

- `overlapping_dispatchers_share_review_ownership_until_verdict_is_saved`: review の `JoinHandle` を await し、
  completion が届いても verdict 保存前は別 dispatcher が review を取得できないことを検証する。
  次の tick で verdict が1件だけ保存され、lock が解放されることまで確認する。
- worktree の保持・複数 repo の2テスト: worker の `JoinHandle` → tick → review の `JoinHandle` → tick の
  順に進める。固定回数の sleep による打ち切りを除去した。
- provider requeue: Adapter の通知より後に行われる worker の後処理も含めて await する。
  review も直接 await し、回数制限つき `yield_now` を除去した。cooldown/backoff の2テストは前回の
  テスト時計を維持する。共通 `run_until_idle` と他テストの時計には変更を加えない。
- `EVENT_TYPES` の4種類と `types` 語彙テストは前回の実装を継承する。
- main `c51837427ac5` を取り込み、PROGRESS の追加記録を Phase F へ移動。main の全197節が分割先に
  そのまま残ること、目次のリンクが存在することを検証した。
- `cargo test -p task-dispatch --lib -- --test-threads=32`: 全351件が3回連続成功。

全体ゲートの結果と残した課題は、この run の成果物ディレクトリの `report.md` に記録する。

再レビュー対応の全体ゲート: `cargo fmt --all -- --check`、`cargo test --workspace`（2506 passed / 0 failed /
5 ignored）、`cargo clippy --workspace --all-targets -- -D warnings` はすべて exit 0。

## release c51837427ac5 の昇格（F4b + planner 修正、schema 28、2026-09-27 15:31Z）と F5-1（やり直し）の結果

- F5-1（やり直し）01M3HG7VV6A2HRHTNXWPDS9051: done（atomic に倒れたまま、13:18〜15:31Z）。planner 2 run（Opus 1 分、Sonnet 7 分）は Plan Mode で成果物を書けず失敗。worker 4 run のうち 3 run が `cheap/mechanical-verifiable-reversible` で gpt-6-luna（F1 の規則表が atomic でも Task の features から cheap を選んだ）、retry のエスカレーションで 1 run が frontier（gpt-6-astra）。continuation 1。E6（全 run standard）と比べて lane の分布は変わった。
- 昇格: in-flight 0 で停止→起動、本番 `c51837427ac5`（schema 28）。本番に F4b（reached / Go、案件 replan、children、案件ページ DAG）と planner の permission_mode 修正が入った。
- 3 回目の F5-1 を投入（planner が成果物を書ける版で compound の計画が採用されるかを確認）。

## F5-1 dogfood（3 回目、2026-09-27 15:52〜16:32Z、タスク 01M3HS2E19BRC021ZXMDZANP5B）: compound の計画が初めて本番で採用され、2 つの不具合が見つかった

- 経過: 人の明示 compound → gate（shadow でも採用）→ planner v1（6 WU、`investigate` / `implement` の 2 工程 + 統合 WU）。`investigate` 工程は並列に完走し `integrate-investigate` も done（**WU 並列と統合が本番で動いた**）。`implement` 工程で 3 WU が並列に走り、5 / 9 WU done。
- 不具合 1: WU `impl-quota` の検査が、兄弟 WU と共有する `CARGO_TARGET_DIR`（`build-cache/cargo/<repo-key>`）に残った別ブランチの `task-core` の生成物で偽のコンパイルエラー（E0609）→ retry 2 回 → failed → replan。並列 WU は target を分けなければならない（実装エージェントでも同じ問題が出ていた）。
- 不具合 2: replan の planner は差分（`modify: [impl-quota]` だけ）を出したが「done work unit integrate-investigate must not change on replan」で 2 回拒否 → Task は blocked（question）。daemon が足した統合 WU が差分の適用か不変条件の検査で「変わった」扱いになる。
- lane の分布（F1 の効果、E6 = 全 run standard との比較）: planner 3 run は claude-oauth の残量降格で cheap（claude-sonnet-5）と standard（claude-opus-5-5）、worker 7 run は standard（gpt-6-sol / claude-opus-5-5）と cheap（claude-sonnet-5）。規則は `frontier/judgment-under-uncertainty` に当たった WU も上限（max(Task lane, standard)）で standard に丸まっている。continuation 0、repair 0、replan 2。
- 20:18Z に cancelled（人の操作と思われる）。修正は Phase F5-fix（Opus）へ: WU ごとの `CARGO_TARGET_DIR`（終端で削除）、replan で daemon 由来の WU を不変条件から除外して持ち越す。
- 参考: release `353d32fbe0ea`（F5-1 やり直しの配送: EVENT_TYPES 補完、フレークテスト 5 件の決定化、PROGRESS.md の分割）を 15:52Z に停止→起動で昇格。本番はこれ。
## Phase F5-fix「F5-1 dogfood（3 回目）の 2 不具合の修正」（着手 2026-09-28）

### F5-fix checkpoint 1: 不具合 2（replan の差分が daemon の統合 WU で拒否される）（完了 2026-09-28）

- 原因: `task_ops::execution::replan` は F2b で daemon が足した WU（`kind = integrate`・統合の repair WU）を done の不変条件から外していたが、**dispatcher の planner run の検証**（`replan_done_work_units` の手前、`run_planner` の `validate`）は done の WU をすべて渡していた。planner の差分（`apply_delta` の結果）は計画の spec（daemon の WU を含まない）なので、`integrate-investigate` が「無い＝変わった」扱いになり 2 回拒否 → blocked(question)。
- 修正: `task_core::{is_daemon_added_work_unit, replan_done_work_units}` を足し、dispatcher と `replan` の両方がこれを使う。`replan` の削除ループは統合の repair WU を（その工程が新しい版に残る限り）superseded にしない。`DoneWorkUnitChanged` / `ReservedKind` / `ReservedKey` の文言に「daemon が足した WU（kind = integrate の統合 WU・統合の repair WU）は書かなくてよい」を足す（`DAEMON_ADDED_HINT`）。
- テスト: `task_core::execution_plan::tests::replan_delta_does_not_treat_daemon_added_done_units_as_changed`（fixture v2・2 工程・`integrate-investigate` done・統合 repair done、差分 `modify: [impl-quota]` → `apply_delta` → `validate` が通る。旧挙動の done 集合では拒否され文言に案内が出る）、`task_ops::execution::tests::replan_carries_daemon_added_units_without_the_planner_restating_them`（全体形式）、`task_dispatch::dispatcher::tests::replan_delta_after_an_integrated_phase_keeps_the_daemon_integration_unit`（dispatcher 経由の再現。修正前の done 集合に戻すと FAILED、修正後 ok を確認）。

### F5-fix checkpoint 2: 不具合 1（並列 WU が同じ `CARGO_TARGET_DIR` を共有する）（完了 2026-09-28）

- 自分の worktree で走る v2 の WU の run・その checks は `<build_cache_dir>/cargo/<repo-key>/wu-<work_unit_id>`、Task 単位の run・統合 WU の検査・reviewer の checks は `<repo-key>`（`task_worker::build_cache::work_unit_cargo_target_dir`、`LocalWorkspace::with_env`、`Dispatcher::check_cargo_target_env`）。
- `RunRequest.cargo_target_dir` で `request.json` に実際の値を残す。終端（done / cancelled / superseded）の WU の target は tick ごとの `cleanup_work_unit_build_caches` が rename → 別スレッドで削除。
- ADR-0074: §4 に「並列数 × target の容量」の注意、末尾に「Phase F5-fix 実装時の逸脱・明確化」。
- テスト: `task_dispatch::dispatcher::tests::parallel_work_units_get_their_own_cargo_target_dir_and_it_is_removed_when_done`（並列 2 WU の request.json の `cargo_target_dir` が WU ごとに異なり期待値と一致、WU の checks は各 `wu-<id>`、統合・reviewer の checks は `<repo-key>`、WU done 後に target が消える）、`task_worker::build_cache::tests::work_unit_target_dir_is_nested_under_the_repo_key`。

### Phase F5-fix の全体ゲート（2026-09-28）

- `cargo fmt --all -- --check` → exit 0
- `cargo test --workspace --no-fail-fast` → exit 0、passed 2511 / failed 0 / ignored 5
- `cargo clippy --workspace --all-targets -- -D warnings` → 警告 0
- `UPDATE_SCHEMA=1 cargo test -p task-core schema && UPDATE_SCHEMA=1 cargo test --workspace committed_schema_matches_generated` → ok（`docs/protocol/worker-protocol.schema.json` に `cargo_target_dir` が追加、commit 済み）
- `cd gui && corepack pnpm@11.27.0 gen:types`（差分ゼロ）`&& typecheck && test` → Test Files 73 passed、Tests 1113 passed

### 未解決事項・提案

- 本番での確認（F5-1 dogfood の 4 回目）: 並列 WU の `runs/<run_id>/request.json` の `cargo_target_dir` が WU ごとに違うこと、WU done 後に `<build_cache_dir>/cargo/<repo-key>/wu-*` が消えること、replan の差分が統合 WU で拒否されないことを確かめる。
- 別 Task 同士（同じリポジトリの Task 単位の run）は今も `<repo-key>` を共有する（ADR-0066 D1 のまま）。同じ偽のコンパイルエラーが Task をまたいで出るなら、Task 単位にも分けるかを判断する。

## F5-fix2: WU run の完了が記録されない（dogfood 4 回目）（2026-09-28）

### 症状

- タスク 01M3JXB3DHVBWKWKPW04DTG6SJ（計画 v3 01M3K0X3Z4ZF3XGKEVCP5TQ77B）の `gate` WU（kind = test、checks 5 本）の run が
  `runs/<run>/result.json`（type = done、evidence criterion 0/1、input ~1.59M tokens）を書いて終わったのに、`worker_finished` が無く、
  `runs.status = running`・WU `running` のまま。約 2 時間後に `infra_requeue: lease expired (run_id=phase:…:gate:…)` →
  `restart_reconcile` で ready に戻され、やり直しになった。2 回発生:
  - run 01M3K0X49JB5JP5TQH304ZTRW2: result.json 03:39:03Z → lease 失効 05:30:04Z（seq 984〜986）。
  - run 01M3K7WNJGYAPNBPMBVJXZ96CC: result.json 05:43:02Z → lease 失効 07:34:04Z（seq 1053〜1055。本 fix の調査中に本番で予測どおり
    発生。その後 run 01M3KF2HFMHPJR7YEB5HMT38MQ〈claude、result.json 無しの infra_requeue〉、run 01M3KFDDQRTKPT7881HEC1DDVW が走っている）。

### 根本原因

- **lease の期限が決め手**: 失効時刻は result.json の時刻 + 111 分（03:39:04 → 05:30:04、05:43:03 → 07:34:03）。
  `review_timeout_secs = 600 × (checks 5 × 2 + 1) + lease_grace_secs 60 = 6660 s` で、`spawn_work_unit_checks` が検査の前に延ばす
  lease と一致する（修正前 `crates/task-dispatch/src/dispatcher.rs:4214-4223`）。つまり run の完了は受け取られ、**WU の checks が
  spawn された後に、検査の完了（`Completion::WorkUnitChecks`）が失われた**。WU の scratch target には 05:43:06 の cargo 起動
  （`.rustc_info.json`）以降の書き込みが無く、GUI の vitest の結果（`node_modules/.vite/vitest/*/results.json`）も worker の 05:40:31
  のまま（検査は途中で止まった）。
- **なぜ失われたか**: `on_worker_finished` は run を `running` から外してから（`take_running_by_run_id`、修正前 L4090）
  `spawn_work_unit_checks`（修正前 L4180-4250、spawn は L4236）で検査を `tokio::spawn` するが、その handle をどこにも持たない。
  `Dispatcher::in_flight()`（修正前 L2265-2267（L2266: `self.running.len() + self.reviewing.len()`））と `TickReport.in_flight`（修正前 L2888）は
  検査を数えないので、draining のインスタンスの supervisor（`crates/celeris/src/instance.rs:412` `if in_flight == 0` → drained、exit 0）
  は検査を spawn した直後の tick で exit し、検査ごと完了を失う。2 回とも検査の最中にライブ切替があった（1 回目: 03:39 検査開始 →
  03:42 G1 の昇格、2 回目: 05:38:56 切替で旧 G1 デーモンが draining → 05:43:02 run 完了・検査開始 → その tick で exit）。
  新しい active はこの run を持たず、reconcile は WU の lease（検査が延ばした 111 分）が切れるまで何もしなかった。
  replan v3 そのものは原因ではない（v2 の gate run は検査の途中に切替が無かったので記録された。v3 以降の 2 run がたまたま切替と重なった）。
- 同じ形の潜在不具合: 工程の統合（`integrating`）も `in_flight` に数えていなかった。また `drain_completions`（修正前 L3716 / L3746）は
  `on_worker_finished` / `on_work_unit_checks_finished` のエラーを `?` で tick ごと返し、受信済みの完了を捨てていた（今回の 2 件の直接の
  原因ではないが、同じ「完了が黙って消える」症状になる）。

### 修正（`crates/task-dispatch/src/dispatcher.rs`、schema 変更なし）

- (a) `checking: HashMap<run_id, CheckingEntry{task_id, handle}>` を足し、`spawn_work_unit_checks` が入れ、`Completion::WorkUnitChecks`
  の受信・Task が Running でなくなったとき（`abort_stale_runs`）・`abort_all_runs` で外す。`in_flight()` は
  `running + reviewing + checking + integrating`、`TickReport.in_flight` も `in_flight()`。draining のインスタンスは検査・統合が
  終わって完了を記録するまで exit しない。
- (a') lease の照合: `reclaim_expired_leases` は `checking` の run がある Task の lease を回収せずに延ばす（検査が
  `review_timeout × (2n+1)` より長引く場合。timeout の 2 倍の再試行があるので最悪 3n 倍かかりうる）。`reconcile_parallel_tasks` は
  `checking` の run を「手元の run」とみなす。
- (b) `drain_completions` は確定のエラーで抜けず `record_finalisation_failure` を呼ぶ: `WorkerFinished{outcome: "infra_requeue:
  finalisation failed: <err>", end: harness_error(infra)}` と `runs` の `harness_error` を残し、Task の lease を持つ run は
  `InfraRequeue`（上限超過で `infra failure ×N`）+ WU を ready（reason `finalise_failed`）、工程の lease（v2）の WU の run は Task を
  遷移させずに WU だけを戻し、同じ WU で `max_infra_retries` を超えたら WU を failed（ADR-0072 D12/D17 の replan / 失敗へ）。
- (c) P-F5-3 の result.json の部分: lease（または WU の lease）が切れた run で、このインスタンスの `running`/`checking` に無く、
  `runs/<run_id>/result.json` に終端（done / question / yielded / budget_exhausted / 供給側の失敗でない error）があるものは、
  requeue せず `on_worker_finished` と同じ経路で確定させる（checks があれば走らせ直す。`finalise_from_result_json`、
  `terminal_from_run_dir`）。`reclaim_expired_leases` と `reconcile_parallel_tasks` の両方から呼ぶ。
- ADR-0074 末尾に「Phase F5-fix2 実装時の逸脱・明確化」を追記。

### 証拠

- 再現テスト（`task_dispatch::dispatcher::tests`、本番と同じ形: 1 工程・kind = test の `gate` WU・checks 2 本、1 回目は検査で落ちて
  `work_unit_retry`〈runs = 2、本番の replan v3 後と同じ行の形〉、2 回目の run が criterion 0/1 の evidence と 1.59M tokens の usage で done）:
  - `draining_dispatcher_keeps_work_unit_checks_in_flight_until_the_completion_is_recorded`
  - `lease_expired_work_unit_run_with_a_result_json_is_finalised_instead_of_requeued`
  - `result_json_finalisation_records_the_completion_end_to_end`
  - `a_finalisation_failure_is_recorded_as_an_infra_requeue`（v1 の WU、Task の lease）
  - `a_finalisation_failure_of_a_parallel_work_unit_resets_only_that_unit`（v2、上限超過で WU failed）
- 修正前の挙動に戻して（`in_flight()` を `running + reviewing`、result.json からの確定を無効化）実行 → 上の最初の 3 本が FAILED:
  「WU の checks が走っているのに in_flight() == 0」、`["infra_requeue: lease expired (run_id=phase:…:gate:…)"]`（本番と同じ
  `infra_requeue` → `restart_reconcile` の列）。修正後は 5 本とも ok。
- 全体ゲート:
  - `cargo fmt --all -- --check` → exit 0
  - `cargo clippy --workspace --all-targets -- -D warnings` → exit 0、警告 0
  - `cargo test --workspace --no-fail-fast` → exit 0、passed 2576 / failed 0 / ignored 7
  - GUI・生成型・schema は触っていない（gen-diff 不要）。

### 未解決事項・提案

- P-F5-3 の残り: 新しい active が「draining の旧デーモンが持つ run」を lease 失効より前に引き継ぐこと（run のプロセスが消え
  result.json があるなら、どのインスタンスも持っていないと判断できれば即座に確定できる）はしていない。今回の (a) で旧デーモンは検査が
  終わるまで exit しないので、本番の 2 件の経路は閉じた。旧デーモンがクラッシュした場合は lease 失効（最大 `review_timeout × (2n+1)`）
  まで待ってから (c) で確定する。
- 検査前に延ばす lease の式（`review_timeout × (2n+1) + lease_grace`）は timeout の再試行（最大 3 倍）を含まない。(a') で検査中は
  回収しないので実害は無くしたが、式そのものは変えていない。
- 2 回目の run の `worker_progress` が 05:40:47Z（item_33）で止まり stdout.jsonl は 05:43:01Z（item_38）まで続いていた件は未調査
  （draining 中の旧デーモンの progress の間引き・書き込みの問題の可能性。完了の喪失とは独立）。
- 本番への反映は昇格待ち（本 fix は production に触れていない）。

### 昇格の証跡（2026-09-28）

- release af65cfb6592d（main af65cfb）: gate FMT / TEST / CLIPPY すべて exit 0（workspace の test run は 07:54〜07:57Z）。
  verify `ok=true live_ok=true`、schema 28。2026-09-28T09:24:03Z に live へ昇格（backup `20260928-092356-pre-af65cfb6592d`）。
- 観察: gate を `RUSTC_WRAPPER` を export した環境で走らせると、`cache_server_down_means_no_rustc_wrapper` と
  `runs_fall_back_to_plain_cargo_when_the_server_is_down` の 2 本が落ちる（run の環境が daemon の環境の `RUSTC_WRAPPER` を継ぐため）。
  → 答えは「外す」。**phase-G.md の G3-fix1**（継承した RUSTC_WRAPPER が run に漏れる）で、sccache を配線しないとき run と checks の
  子プロセスから `RUSTC_WRAPPER` / `RUSTC_WORKSPACE_WRAPPER` / `SCCACHE_*` を `env_remove` するようにした（ADR-0075「G3-fix1 実装時の明確化」）。

## F5-fix3: planner が計画の上限を知らない / lease 失効 run の runs 行（2026-09-28）

### 症状

- 不具合 1（タスク 01M3JXB3DHVBWKWKPW04DTG6SJ、events seq 1282〜1351）: reviewer が criterion 5（`docs/progress/phase-F.md` の main との
  競合）で不合格 → `review_fail` → replan の planner run 01M3KHDJ5YWCM3659ZWC50GPX1（claude-opus-5-5）が 08:18:36Z に
  `error(retryable=true): invalid execution plan: work unit sync-main: too many checks: 8 > 6`。再試行の planner run
  01M3KHGNJC6V8M6MCRV6VA2KQC（08:18:46Z〜08:19:42Z）も同じエラー → `blocked`（worker_question）。人が上限を答えて解除した。
- 不具合 2: 同じタスクの `runs` 行 01M3K0X49JB5JP5TQH304ZTRW2（03:28:29Z 開始）と 01M3K7WNJGYAPNBPMBVJXZ96CC（05:30:34Z 開始）が、
  daemon が `worker_finished{outcome: "infra_requeue: lease expired …", end: harness_error(lease_expired)}`（seq 985 / 1054）を
  書いた後も `status = 'running'`・`finished_at = NULL` のまま（WU はその後 done）。

### 根本原因

- 不具合 1:
  - planner のプロンプトは上限のうち WU の数と budget の丸めしか書いていなかった（修正前 `crates/task-worker/src/claude_code.rs:923`
    「Plan at most {max_work_units} WorkUnits」）。`ExecutionPlannerContext`（修正前 `crates/task-worker/src/protocol.rs:525-570`）に
    `max_checks` 等の欄が無く、dispatcher も渡していなかった（修正前 `crates/task-dispatch/src/dispatcher.rs:9597` `let limits =
    task_core::ExecutionLimits::default();` から `max_work_units` / `max_phases` / budget だけを詰める）。検証は
    `crates/task-core/src/execution_plan.rs:1043`（`wu.checks.len() > limits.max_checks`、既定 6 は L399）。
  - 再試行の planner run は前の拒否理由を受け取っていなかった: `give_up_or_retry_planner`（修正前 `dispatcher.rs:5696-5699`）は
    `worker_progress`「計画を採用できませんでした（…）。もう一度だけ試します。」を events に書くだけで、`execution_planner_context` は
    それを読まない。
  - **2 回目が同じ 8 本を出した直接の原因**: 拒否された `artifacts/execution-plan.json` がそのまま残っていた。2 回目の planner は
    08:19:25Z に「The plan from the previous attempt is still there, and it checks out: done WUs are unchanged and it adds one WU,
    sync-main. I'll keep it and write result.json.」と書き、同じファイルを再提出した（seq 1345）。
  - 検証の上限が dispatcher の 3 か所（修正前 `dispatcher.rs:5339 / 5457 / 5468`）とプロンプト用（L9597）でそれぞれ
    `ExecutionLimits::default()` を呼んでいた（出どころが 1 つでない）。
- 不具合 2: `reclaim_expired_leases` の requeue の経路（修正前 `dispatcher.rs:7013-7030`）は `WorkerFinished` を
  `apply_transition_with_events` で書くだけで `run_index_finish` を呼ばない（通常の完了 L4999・planner L5361・確定失敗 L7337 は呼ぶ）。
  同じく `abort_stale_runs`（修正前 L7099〜。cancel 等で止めた run）は `WorkerFinished` も `runs` 行も書かず、drain の打ち切り
  （`abort_all_runs`、L2293〜）で止めた Reviewer run の行も誰も閉じない。

### 修正（schema 変更なし）

- (a) 上限の出どころを 1 つに: `ExecutionConfig.limits: ExecutionLimits`（既定 `ExecutionLimits::default()`、config.toml の欄は無い）。
  planner の出力の検証・`adopt_plan_with_children`・`replan`・planner のプロンプトがすべてこの値を使う。`ExecutionPlannerContext` に
  `max_title_chars` / `max_objective_chars` / `max_done_when_items` / `max_done_when_chars` / `max_checks` / `max_rationale_chars` /
  `max_plan_json_bytes` / `max_children` を追加（0 = 古い request、プロンプト側で既定に倒す。worker-protocol の schema は追加のみ）。
  プロンプトは計画の形の説明の直後（schema の前）に「### Plan limits」節で全部の上限を出す（WU 数、v2 なら phases・children、WU ごとの
  title / objective / done_when / **checks**、rationale、JSON の大きさ、budget の丸め、replan の差分は適用後の計画に対して数えること、
  checks が足りなければ `&&` で 1 本にまとめる例）。初回・replan の両方。
- (b) 再試行: `planner_rejections_since_last_plan`（純粋関数）が直近の `ExecutionPlanned` より後の拒否理由（再試行の進捗と、2 回とも
  拒否されたときの質問の文面。同じ定数から組み立て・取り出す）を集め、`ExecutionPlannerContext.previous_attempt_errors` に入れる。
  プロンプトは「### Your previous plan was REJECTED」節でエラーをそのまま並べ、「その点だけ直せ、再提出するな」と指示する。
  `give_up_or_retry_planner` は拒否した `artifacts/execution-plan.json` を `execution-plan.rejected.json` に移す（次の試行が
  「残っている計画は検証済み」と思い込まない）。
- (c) 決定的な正規化（余った checks を `sh -c "a && b"` に畳む等）は**入れない**（ADR-0074 の F5-fix3 節に理由）。
- 不具合 2: `SqliteStore` が `WorkerFinished` を書く同じトランザクションで、その run の `runs` 行がまだ `running` なら終端にする
  （status は `end` から、無ければ `harness_error`、`finished_at` は event の ts、usage/metrics は event にあれば。
  `crates/task-core/src/store.rs` `close_run_row_for_event_tx`、`append_event_tx` と `apply_transition_tx` の両方）。
  `task_ops::replay::rebuild_work_units_and_runs` と同じ規則なので `replay --check` とも一致する。既に `run_index_finish` で閉じた行
  （checkpoint を持つ）は変えない。lease 失効の requeue・コメントの割り込み・F5-fix2 の確定失敗はこれで閉じる。誰も `WorkerFinished` を
  書かなかった経路は dispatcher の `close_aborted_run` が `WorkerFinished{outcome: "interrupted: …", end: cancelled}` を足す:
  `abort_stale_runs` の worker run（cancel 等）と Reviewer run、drain の打ち切りの Reviewer run（worker run は従来どおり新しい active の
  lease 失効の経路で閉じる）。`interrupted: ` 接頭辞なので GUI の分類は Interrupted（失敗に数えない）、コメントの割り込みも消費しない。
- `runs.status` の利用者の確認: `execution_metrics`（最新 run の status）、WU の continuation 文脈（`Run #n <status>`）、
  `replay --check`、`run_work_unit_keys`（status を見ない）。GUI は `runs` 行を直接読まない（events の `WorkerFinished` から組む）。
  どれも「終わった run が running に見える」誤りが消えるだけで、新しい status 値は増えない。

### 証拠

- 新しいテスト:
  - `task_worker::claude_code::tests::the_planner_prompt_states_every_plan_limit_from_the_context`（既定と違う値で全上限が
    文面に出る、schema 指示の前にある、初回・replan）
  - `task_worker::claude_code::tests::the_retry_planner_prompt_contains_the_previous_validation_error`
  - `task_dispatch::dispatcher::tests::the_retry_planner_run_receives_the_previous_validation_error_and_config_limits`
    （`limits.max_checks = 3` の config で 4 本の checks → 1 回目の context に `max_checks = 3`、2 回目の context に
    `"work unit sync-main: too many checks: 4 > 3"`、拒否ファイルが `execution-plan.rejected.json` に移り 2 回目は古い計画を読まない）
  - `task_dispatch::dispatcher::tests::planner_rejections_are_collected_since_the_last_adopted_plan`
  - `task_dispatch::dispatcher::tests::a_lease_expiry_requeue_closes_the_runs_row`（`harness_error`、`finished_at` あり）
  - `task_dispatch::dispatcher::tests::a_run_aborted_by_cancel_closes_its_runs_row`（`cancelled`）
  - `task_core::store::tests::worker_finished_closes_a_still_running_runs_row`
- 修正を外して（store の `close_run_row_for_event_tx` 呼び出しと `set_aside_rejected_plan` を無効化）実行 →
  `a_lease_expiry_requeue_closes_the_runs_row` と `the_retry_planner_run_receives_…` が FAILED。戻して ok。
- 全体ゲート（`CARGO_TARGET_DIR=/var/lib/celeris/scratch/targets/agent-planner-limits/target`、`RUSTC_WRAPPER` 無し）:
  - `cargo fmt --all -- --check` → exit 0
  - `cargo clippy --workspace --all-targets -- -D warnings` → exit 0
  - `cargo test --workspace --no-fail-fast` → exit 0、passed 2583 / failed 0 / ignored 7
  - worker-protocol の schema は `UPDATE_SCHEMA=1 cargo test -p task-worker` で再生成（欄の追加のみ）。GUI・API の生成型は触っていない。

### 未解決事項・提案

- 本番の 2 行（01M3K0X49JB5JP5TQH304ZTRW2 / 01M3K7WNJGYAPNBPMBVJXZ96CC）は `running` のまま（本 fix は既存の行を直さない）。
  `celerisctl replay --check` の修復、または 1 回限りの `UPDATE runs … WHERE status='running' AND run_id IN (SELECT … FROM events …)` を
  人の判断で。
- 提案: replan の差分（`execution-plan-delta/1`）は phases を足せない（dogfood 4 回目の planner は「差分では工程を足せない」と判断して
  全体形式に書き直した）。差分に `add_phases` を足すか、プロンプトで「新しい工程が要るなら全体形式」を明示する。
- 提案: task-api の人の `PUT /tasks/{id}/execution-plan` と celerisctl は引き続き `ExecutionLimits::default()`（config.toml に欄が無いので
  本番では同じ値）。上限を設定可能にするなら、そのとき `ExecutionConfig.limits` を API にも渡す。
- 本番への反映は昇格待ち（本 fix は production に触れていない。本番 DB は read-only の照会だけ）。

## F5-fix4: codex の sandbox が worktree の git 管理領域に書けない（2026-09-28）

### 症状

- タスク 01M3JXB3DHVBWKWKPW04DTG6SJ の計画 v4 が足した repair WU `sync-main`（`git merge main`、adapter codex / gpt-6-luna /
  `sandbox_mode="workspace-write"`）が 09:33〜09:35Z に 2 回続けて失敗した。run 01M3KND7D33TV57X0PVP8YH3N3 は `work_unit_retry`、
  run 01M3KNSM5W26W01E9HF9K84W1M は質問で `blocked`:「`git merge main` は worktree の登録元
  `/home/rmaeda/workspace/agent-platform/.git/worktrees/agent-platform3/ORIG_HEAD` が read-only のため失敗し、
  `-c core.logAllRefUpdates=false` でも … Celeris 側で writable な worktree を用意してください。」
- 同じタスクの前の WU がコミットできていたのは、codex の on-failure 承認がたまたま昇格実行したから（再現性が無い）。

### 根本原因

- タスクの checkout `/var/lib/celeris/workspaces/01M3JXB3DHVBWKWKPW04DTG6SJ/repos/agent-platform` は
  `/home/rmaeda/workspace/agent-platform` の `git worktree`（`.git` はファイルで `gitdir: …/.git/worktrees/agent-platform3`）。
  `git merge` / `git commit` は per-worktree gitdir（`ORIG_HEAD`・`index`・`HEAD`）と共通の `.git`（objects・refs・packed-refs）に書く。
  どちらも cwd の外。
- codex の `workspace-write` は cwd と `--add-dir` の root しか書けない。celeris は fresh `exec` に `--add-dir <artifacts_dir>` しか
  渡していなかった（修正前 `crates/task-worker/src/codex.rs:420` `command.arg("--add-dir").arg(&req.artifacts_dir);`）。
- ADR-0074 D1.2 の WU worktree（`<task_dir>/wu/<key>/repos/<repo>`）も gitdir は登録元の `.git/worktrees/<name>` にあり、同じ問題を持つ。

### 修正（`crates/task-worker`、schema 変更なし）

- `local_worktree::git_admin_dirs(dir)`（新設）: `git -C <dir> rev-parse --path-format=absolute --show-toplevel --absolute-git-dir
  --git-common-dir`（`GIT_DIR` 等の環境変数は外す）。`dir` が作業ツリーの最上位のときだけ gitdir と common dir を canonical・重複なしで
  返す。git でない・起動できない・サブディレクトリなら空（上位ディレクトリのリポジトリの `.git` を拾って広げない）。
- `codex::git_writable_roots(req)`（新設）: cwd、cwd の親が `repos/` ならその兄弟（シンボリックリンクは除く）、`<workspace>/repos/*`
  を順に `git_admin_dirs` にかける（request にリポジトリの一覧は無いので、ADR-0043 D2 / ADR-0074 D1.2 の配置から拾う）。
- `run_codex_once` の fresh 経路: `--add-dir <artifacts_dir>` の直後に、`sandbox_mode == "workspace-write"` のときだけ上の各 root を
  `--add-dir` で足す（`crates/task-worker/src/codex.rs:468-481`）。read-only の CoS run・`exec resume` は変えない（resume は
  `--add-dir` を拒否する。Phase 68b/68c のコメントを更新）。sandbox mode の既定は変えない。
- テストの fake codex（`worktree_can_write_results_outside_cwd` ほか 1 本）は `--add-dir` の**最初**の値を成果物ディレクトリとして読む
  （繰り返しを受ける）。
- claude-code は `--permission-mode`（OS の sandbox なし）、acp も writable roots の概念が無いので変更なし。

### 証拠

- `--add-dir` が繰り返せること（codex-cli 0.157.0、LLM を呼ばない `--help` 経由）: `codex exec --add-dir /tmp/a --add-dir /tmp/b --help`
  → exit 0。対照の単一値 `codex exec --model x --model y --help` → `error: the argument '--model <MODEL>' cannot be used multiple
  times`、exit 2。
- `codex debug prompt-input -c sandbox_mode="workspace-write" -c sandbox_workspace_write.writable_roots=[<common>]`（LLM を呼ばない）で、
  worktree の cwd の実効 permission profile は `write: <cwd>`, `write: <common>`, `write: :slash_tmp / :tmpdir`、`read: <cwd>/.git`
  （codex は各 root 直下の `.git` を read-only にする。worktree では `.git` はポインタファイルで、実体は `<common>/worktrees/<name>`）。
- 実際の codex の Linux sandbox（`codex sandbox -P probe -c 'permissions.probe.filesystem={…}'`、上と同じ entry、LLM なし）で
  tempdir の worktree に `git merge --no-edit main` + `git commit`:
  - 成果物ディレクトリだけ（修正前と同じ許可）→ `fatal: update_ref failed for ref 'ORIG_HEAD': cannot lock ref 'ORIG_HEAD': Unable to
    create '…/origin/.git/worktrees/code/ORIG_HEAD.lock': Read-only file system`、exit 128（本番と同じ失敗）
  - 成果物 + gitdir + common（修正後の許可）→ `Fast-forward`、続く commit も成功、exit 0
- 本番の checkout に対する（read-only の）`git rev-parse --path-format=absolute --show-toplevel --absolute-git-dir --git-common-dir` →
  toplevel = cwd、`/home/rmaeda/workspace/agent-platform/.git/worktrees/agent-platform3`、`/home/rmaeda/workspace/agent-platform/.git`
  （修正後はこの 2 つが `--add-dir` に載る）。
- 新しいテスト（`crates/task-worker/src/codex.rs`）:
  - `f5_fix4_worktree_cwd_adds_gitdir_and_common_dir`（tempdir に `git worktree add` した 2 リポジトリ + シンボリックリンク 1 つ →
    argv は `--add-dir <artifacts> --add-dir <gitdir> --add-dir <common> --add-dir <兄弟の gitdir> --add-dir <兄弟の common>`、
    リンクは載らない）
  - `f5_fix4_non_git_cwd_adds_only_the_artifacts_dir`
  - `f5_fix4_readonly_cos_run_does_not_add_git_dirs`
  - `f5_fix4_exec_resume_still_has_no_add_dir_in_a_worktree`
  - `f5_fix4_git_admin_dirs_ignores_subdirectories_and_plain_dirs`
- 全体ゲート（`CARGO_TARGET_DIR=/var/lib/celeris/scratch/targets/agent-codex-gitdir/target`、`RUSTC_WRAPPER` 無し）:
  `cargo fmt --all -- --check` → exit 0、`cargo clippy --workspace --all-targets -- -D warnings` → exit 0、
  `cargo test --workspace` → exit 0、passed 2588 / failed 0 / ignored 7（F5-fix3 の 2583 + 新規 5）。

### 未解決事項・提案

- `codex exec --add-dir` が `sandbox_workspace_write.writable_roots` と同じ permission profile の write entry になることは upstream の
  実装と `--help` の文言（"Additional directories that should be writable"）に依る。fresh `exec` 自体は LLM を呼ぶので実機では未確認
  （sandbox の効き目は `codex sandbox` で同じ entry を与えて確認した）。昇格後、本番で最初の codex の merge / commit を含む WU の
  `stdout.jsonl` に `Read-only file system` が出ないことを確認する。
- 登録元 `/home/rmaeda/workspace/agent-platform/.git` 全体が codex に書けるようになる（objects / refs / packed-refs は共通なので
  避けられない）。他のタスクのブランチの ref も書けてしまう。提案: 本当に隔離したいなら、タスクの checkout を登録元の worktree でなく
  `git clone --shared`（または `--reference`）の独立リポジトリにし、統合は fetch で取り込む（ADR-0043 D2 の変更になるので別 ADR）。
- 通常のリポジトリ（`mode = shared` で cwd = 実リポジトリ）では、codex は `<cwd>/.git` を read-only entry にし、本 fix は同じパスを
  write root として足す。どちらが勝つかは codex の entry の優先順位に依る（未確認）。celeris の既定の配置（worktree）には影響しない。
- 本番への反映は昇格待ち（本 fix は production に触れていない。本番のファイルは read-only の参照だけ）。

## F5-1 dogfood（4 回目、2026-09-28 02:26Z〜、タスク 01M3JXB3DHVBWKWKPW04DTG6SJ）: 途中経過

- F5-fix と G1 の効果を本番で確認: `build` 工程の 3 WU（api-docs / milestone-progress / quota-roles）が並列に走り、`request.json` の `cargo_target_dir` は WU ごとに `scratch/targets/task-<id>/wu-<id>/target`（共有 target の混線は再現せず）。`integrate-build` も done。replan v2 / v3（差分 changed=2）が daemon 由来の統合 WU を理由に拒否されなくなった。
- 異常 1 件: `gate` WU の run（03:28 開始）が 05:30 に `infra_requeue: lease expired` で回収され再 dispatch（再実行は 10 分で done）。1 回目の run 自身は「全体ゲート成功（2515 passed）」の result を残しているので、run の終わり際に lease が切れた（G1 昇格 03:42 のライブ切替で旧デーモンが draining のまま 2 時間の run を持っていた経路。旧デーモンが run より先に終了した可能性）。journal が取れず未確定 → 提案 P-F5-3: 「draining 中の旧デーモンが持つ run の lease」を新デーモン側が引き継いで heartbeat する（Phase 116 D5 の拡張）か、旧デーモンの終了条件に「run の lease を渡すまで待つ」を足す。
- 途中の lane 分布: planner 3 run は standard（Opus）、worker 12 run は standard（gpt-6-sol / Opus）7 と cheap（gpt-6-luna）5。E6（全 run standard）から cheap が増えた。

## F5-1 dogfood（4 回目）の最終結果（2026-09-28 02:26〜10:24Z、タスク 01M3JXB3DHVBWKWKPW04DTG6SJ）: failed で終了、成果は人が統合

**結果**: task は 10:24Z に `failed`（`sync-main` WU の checks `cargo test --workspace` が exit 101。G3-fix1 の 2 テスト）。
内容は reviewer（run 01M3KH3JX1E99NWQYD8FACZAAV、gpt-6-sol）が基準 4（3 成果の実装・テスト・文書）を合格、基準 5（main へ取り込める）だけ
`docs/progress/phase-F.md` の競合で不合格 → replan v4 の `sync-main` が main dde19a4 / e50a876 を取り込み競合を解消（merge commit aba1b64、fee4190）
→ その checks が継承した `RUSTC_WRAPPER` で落ちた。成果 branch `celeris/01M3JXB3DHVBWKWKPW04DTG6SJ`（fee4190、17 files +811 −68）は
人（Fable）が main に統合（ebe7008、G3-fix1 と同じ release）。

**指標**（`GET /tasks/{id}/execution` の `metrics`）: WU 12（done 8、superseded 2、failed 1、blocked 1）、run 26（planner 6 / worker 19 / reviewer 1）、
replan 3、repair 1（class planner）、retries 6、continuations 0、input 10.70M / cache read 21.13M / output 0.105M tokens、cost_usd 5.94、
run の壁時計合計 401 分。lane: cheap/gpt-6-luna 10、standard/claude-opus-5-5 8（planner 6 + worker 2）、standard/gpt-6-sol 7、cheap/claude-sonnet-5 1
（Sonnet は週次制限中で 55 秒で失敗）。quota: claude_max_lab seven_day 7.0%（3 run、measured 1 / apportioned 2）、codex は 14 run すべて unknown。

**この dogfood で見つかり、同日に直した不具合（すべて本番で再現→修正→昇格）**
- F5-fix2（release af65cfb6592d、09:24Z 昇格）: WU checks と工程統合が in-flight に数えられず、live handoff 中の旧 daemon が checks 途中で exit → 完了消失。
  gate WU が 111 分ごとに lease 失効→再実行を 2 回繰り返した（1.59M tokens × 2 の無駄）。
- F5-fix3（release e50a8768c0dd、10:32Z 昇格）: planner が計画の上限（checks ≤ 6）を知らず、却下された計画ファイルをそのまま再提出 → blocked。
  人が `POST /tasks/{id}/answer` で上限を伝えて再開。lease 失効で requeue された run の `runs` 行が `running` のまま。
- F5-fix4（同 release）: codex の sandbox が worktree の git 管理領域（登録元 `.git/worktrees/<name>` と common dir）に書けず `git merge main` が失敗
  → blocked。人が「昇格実行で再実行」と answer して再開。
- G3-fix1（この節と同じ release、`docs/progress/phase-G.md`）: daemon から継承した `RUSTC_WRAPPER` が run と checks に漏れ、self-dogfood の
  `cargo test --workspace` が cache-server-down の 2 テストで必ず落ちる → task failed の直接原因。

**人の介入**: answer 2 回（上限の提示、昇格実行の指示）、成果の手動統合 1 回、journald の復旧（root）。

**未解決・提案**
- P-F5-1-4a: replan の planner が `sync-main` のような「main を取り込む」WU を作るのは妥当だが、worktree の登録元が `~/workspace/agent-platform`
  である限り main は人の作業ツリーと同じ `.git` を共有する。F5-fix4 の提案どおり task checkout を clone（`--reference`）にする ADR を別に起こす。
- P-F5-1-4b: 本番 DB に `running` のまま残った 2 行（01M3K0X49JB5JP5TQH304ZTRW2、01M3K7WNJGYAPNBPMBVJXZ96CC）は F5-fix3 では直らない。
  `replay --check` の修復か一回限りの UPDATE を人が選ぶ。
- P-F5-1-4c: task が failed になったとき、成果 branch がゲートを通っていれば「人が統合できる」状態を GUI に出す（今回は人が git で判断した）。
- P-F5-1-4d: Sonnet が制限中でも cheap lane に振られて 55 秒で失敗した。quota の `resets_at` を見て provider を避ける（F1 の quota-aware 規則の拡張）。

**配送**: release `57efebe4fb7d`（main 57efebe = G3-fix1 95c7267 + dogfood 成果の統合 ebe7008 + この記録）。ゲート: fmt / test / clippy 0、
GUI install / typecheck / lint / test 0、gen:types 差分ゼロ（10:43〜10:48Z）。verify ok=true live_ok=true schema 28。2026-09-28 11:02:23Z にライブ昇格
（backup 20260928-110212-pre-57efebe4fb7d）。これで本番は F5-fix2 / fix3 / fix4 / G3-fix1 と dogfood 4 回目の 3 成果を含む。

## F5-fix5: headless の claude-code が background task を残して turn を終える（2026-09-28）

### 症状

- タスク 01M3JXB3DHVBWKWKPW04DTG6SJ の `gate` WU、run 01M3KF2HFMHPJR7YEB5HMT38MQ（claude-code / claude-sonnet-5、07:36:06〜07:37:02Z）。
  `runs/01M3KF2HFMHPJR7YEB5HMT38MQ/stdout.jsonl`:
  - 11〜13 行目: `cargo fmt` を foreground で実行（exit 0）。
  - 15 行目: `timeout 1800 cargo test --workspace > …/logs/test1.log` を Bash の `"run_in_background": true` で起動。16〜18 行目
    `background_tasks_changed` / `task_started`（`is_backgrounded: true`、task `bdk7bdta1`）/ 「You will be notified when it completes」。
  - 19〜49 行目: `ScheduleWakeup`（`prompt` 欠落で失敗）、`sleep 60`（CLI が「standalone sleep」として拒否）、`Monitor` の読み込みなどを
    挟み、「I'll wait for the background test run to complete (a notification will arrive automatically) rather than polling」
    を繰り返して turn を終える。
  - 50 行目: `result` success、`stop_reason: end_turn`、`num_turns: 11`、`duration_ms: 45938`。
  - 51〜53 行目: `background_tasks_changed []`、`task_updated … status: killed`、`task_notification … status: stopped`。
- `artifacts/result.json` は書かれず、`result.json`（run dir）は `claude exited without …/result.json`（retryable）。WU は再実行に回り、
  約 1 分と 1 run を失った。`rate_limit_event` は `allowed`（five_hour 13 %、seven_day 11 %）で quota ではない。

### 根本原因

- `claude -p`（headless）は turn を終えると session を閉じ、残った background task を殺す（51〜53 行目）。完了の通知を受け取る
  次の turn は来ない。
- celeris は claude-code を `claude -p <prompt> --output-format stream-json --verbose --permission-mode … --max-turns N
  --no-session-persistence` で起動していた（修正前 `crates/task-worker/src/claude_code.rs:1421-1468`）。background task を無効にする
  環境変数も、headless であることを伝える system prompt も渡しておらず（同 `:1488` の env の組み立て）、Bash の道具の説明
  （「run_in_background … you will be notified」）がそのままモデルに見えていた。
- 事後の扱いも区別が無かった: `result` success で `result.json` が無い run は一律 `RESULT_JSON_MISSING_MARKER`（修正前 `:1968`）→
  `AdapterError::Other` → InfraRequeue で、background task が殺された事情は記録にも残らない。
- CLI 側の切り替えの確認（LLM・ネットワークなし）: `claude --version` → 2.1.283。バイナリの文字列に
  `function _l(){return mq().backgroundTasksDisabled||a.CLAUDE_CODE_DISABLE_BACKGROUND_TASKS}`、Bash の入力 schema
  `_l()?f3t().omit({run_in_background:!0,…}):…`、Agent の schema `_l()||sZ()?n.omit({run_in_background:!0}):n`、道具の説明文
  `if(_l())return null;return"You can use the \`run_in_background\` parameter…"`。`claude --help` に background を切るフラグは無い
  （`--bg` は session 自体を background で起動する別物）。Bash の timeout 上限は `BASH_MAX_TIMEOUT_MS`（既定 600000）。

### 修正（`crates/task-worker`、schema 変更なし）

- 予防（全 run）:
  - `preamble::HEADLESS_RUN_NOTE`（新設、`crates/task-worker/src/preamble.rs:1117`）: headless であること、turn を終えると run が終わる
    こと、`run_in_background`・Agent の background・Monitor / ScheduleWakeup / Cron に頼らないこと、長い command も foreground で
    `timeout` を明示して走らせること、「通知を待つ」と書いて turn を終えないこと。
  - `run_claude_code` が全 run（worker / planner / reviewer / 対話）に `--append-system-prompt <HEADLESS_RUN_NOTE>` を付ける
    （`claude_code.rs:1556`）。プロンプト本文（`prompt.txt`）は変えない。config.toml の役割ごとの指示文には置かない。
  - 環境変数 `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS=1` と `BASH_MAX_TIMEOUT_MS = max(600000, 壁時計 ms)`（`claude_code.rs:1622`）。
    `env_remove` の後・`config.env` の前に置く（`config.env` の同名が勝つ）。コンテナ実行でも `container::wrap` が env を運ぶ。
- 回復（ADR-0072 D9 の continuation。ADR-0072「Phase F5-fix5 実装時の明確化」）:
  - `BackgroundTasks`（`claude_code.rs:1383`）: stream-json の `system` 行の `task_started`（`is_backgrounded: false` 以外）と、
    `task_notification` / `task_updated`（completed・failed・killed・stopped・cancelled）を追う。`result` を観測した時点の
    未完了の task を `ResultMeta.outstanding_background` に写す（`stop_reason` も読む）。
  - `result` が success（`stop_reason` が `end_turn` か無し）・`result.json` 無し・未完了の background task ありのとき
    （`claude_code.rs:1812`〜）:
    - coding 系の execute run（`is_continuable_execute_run`: Execute / Approval、planner でも対話でもない）→ `Terminal::Yielded`。
      checkpoint は worker の `checkpoint.json` を土台に `next_action`（殺された command を foreground で再実行して続け、result.json を
      書け。前の `next_action` は括弧で残す）と `known_failures`（`headless_background_task: killed when the turn ended: <command>`）を
      足す（`headless_background_checkpoint`）。usage も運ぶ。続きはディスパッチャの既存の continuation（新しい session +
      checkpoint、上限 `max_continuations_per_work_unit`、進捗なし `no_progress_limit`、`CheckpointSaved` / `WorkerFinished{end:
      yielded}`）で、会話の履歴は再送しない。
    - それ以外（レビュー・計画・対話）→ 従来どおり `AdapterError::Other`（InfraRequeue）で、文言に
      `(headless_background_task: background task killed when the headless turn ended: <command>)` を足す。
    - どちらも `headless_background_task: …` で始まる進行（`WorkerProgress`）を 1 件残す（GUI の run の進行に出る）。
  - resume（同じ session に短い追いプロンプト）は採らなかった: 仕事の run は `--no-session-persistence` で resume できる session が
    残らず、残すには D9 (a) の wrap-up（未実装）と同じ変更が要る。D9 (b)「InfraRequeue で同じ session を resume」は ADR で不採用。
    アダプタ内の再起動では continuation の上限・進捗なしも数えられない。新しい `HarnessErrorClass` も足していない（schema・GUI の
    型の変更を避けた。分類は文言・進行・checkpoint の `known_failures` で見える）。

### 証拠

- fixture `crates/task-worker/tests/fixtures/claude-code-headless-background.jsonl`（本番の stdout.jsonl 3・14〜18・25・49〜53 行目の形を
  縮めたもの）を流す fake claude のテスト（`crates/task-worker/src/claude_code.rs`）:
  - `f5_fix5_background_task_killed_at_end_turn_becomes_a_continuation`（`Terminal::Yielded`、`next_action` に foreground と command、
    worker の `completed` を保持、`WorkerCheckpointInput` として読める、usage 2277 output tokens、進行に分類名、run dir の result.json に
    yield）
  - `f5_fix5_a_review_run_keeps_the_missing_result_error_with_the_class`
  - `f5_fix5_a_background_task_finished_before_the_result_is_not_misclassified`
  - `f5_fix5_every_run_gets_the_headless_system_prompt_and_background_off`（argv に `--append-system-prompt <HEADLESS_RUN_NOTE>`、
    子の env が `1|3600000`、`config.env` の上書きが勝ち、短い壁時計は `600000`）
- `crates/task-worker/src/preamble.rs`: `f5_fix5_headless_run_note_forbids_background_tasks_and_waiting_for_notifications`。
- 全体ゲート（`CARGO_TARGET_DIR=/var/lib/celeris/scratch/targets/agent-headless-bg/target`、`RUSTC_WRAPPER` 無し）: `cargo fmt --all -- --check` → exit 0、`cargo clippy --workspace --all-targets -- -D warnings` → exit 0、`cargo test --workspace` → exit 0、passed 2601 / failed 0 / ignored 7（新規 5 本。GUI・生成型は触っていないので gen:types / typecheck は対象外）。

### 未解決事項・提案

- `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS` の効き目はバイナリの実装（2.1.283）で確認しただけで、LLM を呼ぶ実機の run では未確認。
  昇格後、claude-code の run の `stdout.jsonl` の `init` に続く道具呼び出しで `run_in_background` が出ないこと、`task_started` が
  出ないことを確かめる。CLI の更新で名前が変わった場合は system prompt と回復の経路が残る。
- foreground の Bash が `BASH_MAX_TIMEOUT_MS` を超えたときの CLI の振る舞い（background が無効なら kill のはず。
  `CLAUDE_CODE_AUTO_BACKGROUND_TIMEOUT_MS` の自動 background は `_l()` で抑止されるように読める）は未確認。
- 提案 P-F5-fix5-a: `ScheduleWakeup` / `Cron*` / `PushNotification` / `RemoteTrigger` など headless で意味の無い道具を
  `--disallowedTools` で外す（今回は system prompt で禁じるだけ。道具名が CLI の版で変わるので別途）。
- 提案 P-F5-fix5-b: 同じ形が codex / acp に無いかを確かめる（codex の `exec` も turn の終わりで終了する）。
- 提案 P-F5-fix5-c: `headless_background_task` を `HarnessErrorClass` / run の終わり方として構造化し、GUI の run 一覧で数える
  （schema 変更が要るので、再発が観測されたら）。
- 本番への反映は昇格待ち（本 fix は production に触れていない。本番のファイルは read-only の参照だけ）。

**昇格**: release `7667410c23d5`（main 7667410）。ゲート fmt / test / clippy 0、GUI 0、gen:types 差分ゼロ。verify ok=true live_ok=true schema 28。2026-09-28 12:06:01Z にライブ昇格（backup 20260928-120553-pre-7667410c23d5）。

## F5-fix6: 再起動直後に孤児 run を lease 失効を待たずに回収する（2026-09-28）

### 症状（本番 2026-09-28、task 01M3MCA20JSAKGENH571NZES1F、run 01M3MDZF188TRHFZ9554F7H7KN）

- 人が設定の反映（gate=on）のため 17:05:50Z に `systemctl --user restart celeris@1bb7fd6473a4.service` を実行した。
  worker run（claude-code、Opus、16:36:11Z 開始）が走っていた。
- journal: 17:05:50.849Z「SIGTERM; exiting」→ api / llm-proxy / mcp を止め、backup の停止を 5 秒待ち → 17:05:55.852Z
  「instance row removed」→「celeris stopped, exit: Signal」。その間の 17:05:51.45Z に `runs/<run>/result.json` =
  `{"type":"error","message":"worker exited without a result message (exit=143)","retryable":true}` と
  `artifact_produced`（seq 314〜319、未申告成果物の走査）が書かれたが、`worker_finished` は無く、`runs.status` は
  `running`、`tasks.lease_worker_run_id` はその run、`lease_expires_at` 17:20:03Z のまま。
- 新しいデーモン（17:06:06Z 起動、instance 01M3MFP7BYXBC8MXVTBKQZSX5E）は起動時の照合で何もしなかった
  （lease がまだ有効）。lease + `lease_grace_secs` 60 の失効後に F5-fix2 の経路で拾われるまで約 15 分、人からは
  「止まっている」。F5-fix2 の未解決「新しい active が旧の孤児 run を lease 失効の前に引き継がない」そのもの。

### 根本原因（修正前の file:line）

- 止まる側: `crates/celeris/src/lib.rs:2097` の SIGTERM の分岐は tick ループを即座に抜け（`Exit::Signal`）、
  `run()`（L1627〜1660）は listener と背景ジョブを止めて `supervisor.deregister()`（L1659）するだけで、
  `Dispatcher` の完了のチャネルを誰も読まない。systemd（`KillMode=control-group`）は子にも同時に SIGTERM を送るので、
  アダプタはその 0.6 秒後に `result.json` を書いて `Completion` を送ったが、受け取る tick はもう来なかった。
  `abort_all_runs`（drain timeout の強制 abort 用）はこの経路では呼ばれず、呼ばれても worker run の DB は触らない
  （「lease が切れて新しい active が拾う」設計）。つまり「記録しようとして競争に負けた」のではなく、記録する経路が無かった。
- 起きる側: `crates/task-dispatch/src/dispatcher.rs:7042`（`reclaim_expired_leases`）の `if lease.expires_at > now
  { continue; }` と、L9462 / L9481（`reconcile_parallel_tasks`。工程の lease と WU の lease が切れていなければ触らない）。
  lease に持ち主のインスタンスが無く（ADR-0070 D4 で採らなかった）、run の pid も DB に残らないので、
  「持ち主が居ない」を判定する経路が無かった。

### 修正（schema 変更なし。migration 0030 は無い）

- (1) 孤児の回収（`crates/task-dispatch/src/orphan.rs` 新設、`dispatcher.rs` の `reclaim_expired_leases` /
  `reconcile_parallel_tasks` と補助の `holds_task_in_hand` / `lease_holders_gone` / `note_orphan_takeover` /
  `requeue_orphaned_work_unit_run`）。**「持ち主が居ない」**= (i) このインスタンスがその Task を手元に持っていない
  （`running` / `checking` / `integrating` / `reviewing` / `awaiting_children`）かつ (ii) `daemon_instances` の自分以外に、
  `role ∈ {active, draining}`・`drained_at` 無し・heartbeat が `3 × tick + lease_grace` 以内・pid が生きている行が無い。
  そのとき lease の失効を待たずに F5-fix2 と同じ確定をする: result.json に終端があれば `on_worker_finished`
  （error(retryable) → 通常の再試行、done → 検査・レビュー）、無ければ `WorkerFinished{outcome: "interrupted:
  orphan_takeover: …", end: harness_error(infra)}` + `InfraRequeue`（attempts を消費しない、バックオフ無し）、
  WU は reason `orphan_takeover` で ready / needs_continuation。v2 は WU ごと、統合 WU は pending に戻す。回収の前に
  `worker_progress`（`orphan_takeover: …`）と WARN ログ（`reason=orphan_takeover`）を残す。active（`accepting_new_work`）
  で、celeris が `set_orphan_takeover` を渡したときだけ（verify・テスト既定・draining は従来どおり）。
- **ライブ切替の例外**: draining の旧は heartbeat を打ち続けるので (ii) に当たり、新しい active は旧の run を
  横取りしない（ADR-0040 D4 / ADR-0070 D4 のまま）。
- (2) 止まる側（`crates/celeris/src/lib.rs` の `run()`、`Exit::Signal` のときだけ）: `Dispatcher::interrupt_runs_on_shutdown`
  が、届いた完了を `drain_completions` で記録し、残りの worker run をプロセスグループごと止めて
  `WorkerFinished{outcome: "interrupted: daemon shutdown (SIGTERM/SIGINT) …", end: cancelled}` + `InfraRequeue`
  （WU は reason `shutdown`）を書いてから exit する。error 以外の終端の result.json を書き終えた run は触らず、次の
  デーモンの (1) に任せる。レビュー・検査・統合は触らない（次のデーモンの `recover_reviews` と (1)）。
  SIGKILL・OOM・停止中の DB エラーなど (2) が書けなかった場合は (1) が拾う。
- ADR-0070 末尾に「付記: F5-fix6 実装時の明確化」（定義と drain の例外）。

### 証拠

- 新しいテスト（`crates/task-dispatch/src/dispatcher/tests/orphan_takeover.rs`、`crates/task-dispatch/src/orphan.rs`）:
  - `an_orphaned_run_with_an_error_result_is_retried_without_waiting_for_the_lease`（本番の result.json そのまま、
    lease 残り 14 分、旧の行なし → 1 tick で `WorkerFinished`〈exit=143〉と `worker_error` の Running → Ready、`runs` 行が閉じる）
  - `an_orphaned_run_with_a_done_result_is_finalised`（done → 検査 `true` → Done、新しい run は起きない）
  - `a_run_held_by_a_live_draining_instance_is_left_alone`（draining の旧が heartbeat・pid 生存 → 何もしない。
    draining 側・設定なしの dispatcher も回収しない。旧の pid が死んでいれば回収する）
  - `an_orphaned_run_without_a_result_is_requeued_as_interrupted`（v1 計画の WU、`interrupted: orphan_takeover`・
    `harness_error(infra)`・`InfraRequeue`・WU ready〈reason `orphan_takeover`〉・attempts 0・バックオフ無し）
  - `an_orphaned_parallel_work_unit_run_is_reconciled_without_waiting_for_its_lease`（v2 の工程の lease）
  - `a_shutdown_records_the_interrupted_runs_before_exiting`（(2)。`interrupted: daemon shutdown`・`cancelled`・Ready・
    lease なし、2 回目は 0 件）
  - `orphan::tests::the_holder_is_gone_only_when_no_other_live_active_or_draining_instance_exists`、
    `orphan::tests::proc_pid_alive_sees_this_process_and_not_an_unused_pid`
- 修正を外して（`lease_holders_gone` を常に `false`）実行 → 孤児の 5 本（error / done / draining の最後の段 / result なし /
  v2 の WU）が FAILED、戻して 6 本とも ok（shutdown の 1 本は (2) の別経路なので外しても ok）。
- 全体ゲート（`CARGO_TARGET_DIR=/var/lib/celeris/scratch/targets/agent-takeover/target`、`RUSTC_WRAPPER` 無し）:
  - `cargo fmt --all -- --check` → exit 0
  - `cargo clippy --workspace --all-targets -- -D warnings` → exit 0、警告 0
  - `scripts/dev/test-parallel.sh` → exit 0、`CELERIS_TEST_SUMMARY` passed 2627 / failed 0 / ignored 7（nextest 77 バイナリ + doc 10）
  - GUI・生成型・schema・migration は触っていない。

### 未解決事項・提案

- 本番の run 01M3MDZF188TRHFZ9554F7H7KN は、本 fix の調査中に lease 失効（17:20:03Z + 60 s）後の F5-fix2 の経路で
  既に拾われ、planner → WU `merge-and-check` に進んでいる（本 fix は本番に触れていない。DB は read-only の照会だけ）。
- (ii) は保守的: draining の旧が生きている間は、別の落ちたデーモンの孤児も lease 失効まで待つ。lease に `instance_id` を
  持たせれば run ごとに判定できるが、`acquire_lease` の trait の形と schema を変えるので今回もやらない。
- pid の再利用で死んだ旧の pid が別プロセスに使われていると「生きている」と誤認し、lease 失効まで待つ（安全側）。
- 持ち主のデーモンが SIGKILL で消え、systemd の外（開発時の端末など）で子が生き残った場合、その子は stdout の pipe を
  失って次の書き込みで止まる想定だが、孫（`cargo` 等）は残りうる。新しい active はその run を即座に requeue するので、
  同じ worktree に一時的に 2 つの書き手が重なりうる（systemd 配下の本番では cgroup ごと止まるので起きない）。
- (2) で requeue した run は `interrupted:` なので GUI では「中断」（失敗に数えない）。止めた run の続き（continuation）は
  WU の checkpoint があるときだけ（v1 の atomic Task は通常の再 dispatch）。
- `abort_all_runs`（`drain_force_abort = true` の drain timeout）は従来どおり DB を触らないが、そのデーモンは
  `drained_at` を付けて exit するので、新しい active は (1) で即座に拾う。
- 本番への反映は昇格待ち。

## 人の決定と本番 DB の修正（2026-09-28 11:2xZ、記録は 14:4xZ）

- **P-F5-1-4b は実施**: 本番 DB の `runs` で `status='running'` のまま残っていた 7 行を人の指示で UPDATE した。dogfood 4 回目の gate run 2 行
  （01M3K0X49JB5JP5TQH304ZTRW2 → 05:30:04Z、01M3K7WNJGYAPNBPMBVJXZ96CC → 07:34:04Z。lease 失効の requeue 時刻）、それ以前の reviewer 3 行と worker 2 行
  （task はすべて done / cancelled。task の終了時刻）を `status='harness_error'`、`finished_at` を対応する時刻に。事前バックアップ
  `~/.local/celeris/backups/20260928-112219-pre-runs-update.sqlite3`。UPDATE 後 `running` は 0 行。以後は F5-fix3 の store hook が同じ状態を作らない。
- **P-F5-1-4a は取り下げ（人の決定）**: task の checkout は git worktree のまま使う（clone / `--reference` は容量と時間のコストが重い）。codex の書き込みは
  F5-fix4 の `--add-dir`（worktree の gitdir と common dir）で足りる。共有 `.git` の他ブランチへの書き込みは reviewer とブランチ規約で受け止める。
- **P-F5-1-4d は誤りだったので訂正**: 07:36Z の Sonnet run（01M3KF2HFMHPJR7YEB5HMT38MQ）の `rate_limit_event` は `status: allowed`（five_hour 13%、seven_day 11%）で、
  制限には当たっていない。実際の原因は headless の claude-code が `cargo test --workspace` を background task にして turn を終え、セッション終了で
  background task が kill され `result.json` 無しで終わったこと（F5-fix5 で修正・昇格済み）。router の不具合ではない。

## Phase F 最終報告（2026-09-28）

- 報告: [`docs/execution-parallel-report-2026-09-28.md`](../execution-parallel-report-2026-09-28.md)（ADR-0074 の決定と実装・release・昇格の対応、
  受け入れ条件 46 件の判定〈満たす 42 / 部分 2 / 未 2〉、E6 と dogfood 1〜4 回目の比較、見つかった不具合 11 件、quota の観測の範囲、未解決・提案の集約）。
- ADR-0074 の状態は **Partially implemented**（D1.3 の Task / CoS / profile による並列数の絞り込み、D2.1 の `PUT …/pause-after`、
  D3.4 起点 (c) の自動 replan、D3.8 の CoS の案件直下 Task の draft 化〈未確認〉、D6.2 の配送の `RepairScheduled`）。
- 本番で一度も通っていないもの: 案件計画（`project_plan_proposed` 0 件）と途中確認（`awaiting_human` 0 件）。F5b の残り。
- 訂正: 本ファイルの「Phase F5-1 dogfood やり直し」節の `01M3HJYF9ZN09GE0ZFT9VT4V8J` は Task ではなく run の id（Task は 01M3HG7VV6A2HRHTNXWPDS9051）。
  4 回目の節の「codex は 14 run すべて unknown」は、正確には 5 時間窓が 14 run すべて unknown・7 日窓は measured だが 13 run が前後同値（0.0pt）。
- 次の一歩（提案）: `[execution] parallel` を採用の関門にするか決めて本番の設定と揃え、181939898ec3 の上で F5b の形（案件計画 + `pause_after` + 並列）の
  dogfood を 1 回流し、codex の quota が取れない件と合わせて `parallel` / `gate` の既定を人が判断する（報告 §8）。

**人の決定（2026-09-28 15:2xZ）**: P-F-1 は「`[execution] parallel = true` にして実態に合わせる」。本番 `~/.config/celeris/config.toml` の `[execution]` に
`parallel = true` を足し、idle のときに `systemctl --user restart celeris@<current>` で反映する（config の reload 機構は無い。`parallel` は F2b 以降の全 release が
知っている key なので N-1 は壊れない）。費用指標（決定 4: quota 消費）は、codex の quota が measured で取れるようになりデータが揃ってから改めて判断する。

## Phase F6: 既存の Task / 案件を後から分解の経路に入れる、案件の編集（2026-09-28）

### 症状・動機

- 人の依頼（2026-09-28）: (1)「すでにタスクや案件として起票されてしまっているものに関して、後から分解の経路（ExecutionPlan）に入れられるようにしたい」
  — 人・CoS（ChatGPT RDC 経由を含む）が起票し、gate で atomic になった（または gate=shadow の下で CoS のヒントだけの compound が記録だけになり
  atomic で走った）Task を、人の操作で planner の経路に入れたい。止まった案件（`benchfs` 01M35WRV77A2JPYGERQGXF6V7K「BenchFS 国際会議フルペーパー化」。
  最後の仕事は 2026-09-24 の Phase 1 framing の調査・承認・報告のまとめ）を GUI から案件計画（CoS がマイルストーン Task の DAG を提案 → 人が承認）に
  入れたい。(2)「案件の名前や説明を後から GUI 上で変更できるようにしたい」。
- 本番で見つかった不具合（coordinator 経由、2026-09-28）: `POST /tasks/{id}/retry` が元の gate の判定を写す。01M3MBV3AKXZGEG5RXR60XC62J は
  15:58Z に gate=shadow で判定（`compound/long-and-broad`、`source: hint`、`shadow: true`）され atomic で走り、人が中止 → 17:06Z に gate=on で再起動 →
  retry。複製 01M3MFS5T52FXA63W4V10XGC4S には自分の `execution_gated` が無く、`GET /tasks/{id}/execution` の `gate` は元の `shadow: true / source: hint`
  のまま。それでも daemon は写された判定（mode compound）を gate=on で採用して planner run を起こし、計画 v1（7 WU）を実行中。GUI は古い判定を出していた。
- 調べて分かったこと: 案件計画の API（`POST /projects/{id}/plan {mode: "milestones"}` → CoS の計画 run → `ProjectPlanProposed` → `decide`）は既存の案件でも
  そのまま動く（下の結合テスト）。本番で `project_plan_proposed` が 0 件だった理由は GUI に**初回の**案件計画を起こす入口が無かったこと（「計画を見直す」は
  承認済みの計画がある案件にしか出ず、「この方針で進める」は `mode` を送っていなかった）。benchfs には途中目標が 0 件（`milestones` の行なし）。

### 決定（ADR-0072「Phase F6 実装時の決定」P1〜P7）

- P1 `POST /tasks/{id}/execution/decompose {mode, note?}`（管理系）: 人の明示（`execution_hint = {mode, explicit: true}`）を書き、前の判定を消し、
  `execution_hint_set{source}` を残す。次の dispatch で gate が `human/explicit` として判定し直す（D13「1 回だけ」の例外）。`draft`/`ready`/`blocked` のみ。
  `running`/`reviewing` は 409（run を止めない。止めるなら中止 → retry）、終端は 409（retry の `execution`）、計画を持つ Task の `compound` は replan の依頼
  （`max_replans` の範囲、依頼は次の `Transitioned{to: running}` で消費）、計画を持つ Task の `atomic` は 409、gate の対象外は 422。
- P2 GUI の「この方針で進める」に「進め方」（仕事に分解する / 案件計画を提案させる）を足した。提案は既存の DAG 節の破線 + 承認 / 却下に出る。
- P3 `PATCH /projects/{id}` に `title`・`request`（= 説明、依頼文）。空は 422、200 / 20,000 文字まで。新しい `description` 欄は作らない。slug（K-1）も同じフォームに。
- P4 案件の編集の監査は管理系の構造化ログ `op = "project_updated" fields=[…]`（案件の events 列は無い。作るなら migration。提案に残す）。
- P5 retry は `routing.execution` を写さない（`execution_hint` は写す）。複製先は今の設定で判定し直し、自分の `execution_gated` を残す。`RetryBody.execution` で
  複製先に人の明示を書ける。
- P6 MCP `task_decompose { id, mode, note? }` は `tasks:interact`（run を止めない・複製しない・承認しない・取り消せる。ChatGPT RDC の推奨 scope のまま使える）。
  `task_retry { id, execution? }` は `tasks:control` のまま。
- P7 GUI のタスク詳細に「実行の形（atomic / compound）」節: gate の判定の出どころ（規則表 / CoS のヒント + 規則表 / 人の明示、shadow）と `execution_hint`、
  確認ダイアログ付きのボタン。
- migration なし（`SCHEMA_VERSION` 28 のまま）。新しい Event `execution_hint_set`（`docs/api/v1/event.schema.json`）。

### 使い方

- GUI（タスク）: `/tasks/<id>` の「実行」節の下「実行の形」→「計画を作らせる（compound に切り替え）」（確認 → 次の run が planner run）。終端のタスクは
  「計画を作らせてやり直す」（新しいタスクへ移る）。計画のあるタスクは「計画を見直させる（replan を依頼）」。
- GUI（案件）: `/projects/<id>` の「依頼」節の「名前・説明を編集」（名前・説明・slug）。「この方針で進める」で「案件計画を提案させる」を選んで送ると、CoS の
  計画 run が提案を出し、「案件計画」節に破線の提案と「承認 / 却下」が出る。
- API: `curl -X POST -H "Authorization: Bearer $TOKEN" -d '{"mode":"compound","note":"工程に分けて"}' $CELERIS/api/v1/tasks/<id>/execution/decompose`、
  `-d '{"execution":"compound"}' …/tasks/<id>/retry`、`-X PATCH -d '{"title":"…","request":"…"}' …/projects/<id>`、
  `-d '{"mode":"milestones","note":"…"}' …/projects/<id>/plan` → `-d '{"decision":"approve"}' …/projects/<id>/project-plan/1/decide`。
- MCP: `tools/call {"name":"task_decompose","arguments":{"id":"<id>","mode":"compound"}}`（scope `tasks:interact`）。

### 証拠

- Rust の新しいテスト:
  - `task_ops::regate::tests`（4 件: 明示の書き込み・判定の消去・source/note の記録、running/終端の 409、対象外・長い note の 422、replan の依頼の消費）。
  - `task_ops::retry::tests::{retry_drops_the_copied_gate_decision_and_keeps_an_explicit_hint, retry_with_execution_sets_an_explicit_hint_on_the_copy_with_its_source}`。
  - `task_dispatch::dispatcher::tests::regate_of_an_atomic_task_starts_a_planner_run_and_yields_a_plan`（gate=shadow、偽アダプタ。hint_set → 新しい
    `execution_gated{human/explicit}` → planner run 1 本 → 計画 2 WU → done）、`regate_of_a_planned_task_is_a_replan_request`（版 2、planner の理由に note と
    `mcp:chatgpt`）、`retry_after_switching_gate_to_on_regates_the_copy`（shadow の判定を持つ Task を中止 → gate=on で retry → 複製に `execution_gated` が
    1 件・`shadow: false`・`source: hint`、done）、`milestones_plan_on_an_existing_stalled_project_reaches_the_human_gate_and_is_approved`（done / failed の
    Task がある案件 → `start_milestones` → 偽 CoS の project-plan/1 → `ProjectPlanProposed v1`・`dag_view.pending`（2 節点）→ `decide approve` →
    途中目標 approved・「調査」ready・`current_version = 1`・既存の Task はそのまま）。
  - `task-api` `tests/execution.rs`（decompose の 200 / 409 / 422 / 400 / 401 / 404、retry の `execution` と 422）、`tests/project_plan.rs`
    `patch_project_edits_title_and_request_and_validates_them`（trim、題名だけ + slug、空・長すぎの 422、知らない欄の 400、401、404）。
  - `celeris-mcp` `task_decompose_requires_tasks_interact_and_records_the_mcp_source`（`tasks:read` だけは -32601、`source: mcp:chatgpt`、終端は -32602）。
- GUI の新しいテスト: `test/unit/execution-mode.test.ts`（判定の 1 行 1 件、表示の判定 5 件、描画 3 件、decompose の写し 2 件、retry の `execution` 1 件）、`test/unit/projects.detail.test.ts`
  （`patchProjectText` 2 件、`startProjectPlan` の `mode` 2 件）。
- 関門（すべて exit 0）: `cargo fmt --all -- --check`、`scripts/dev/test-parallel.sh`（nextest 77 バイナリ + doc 10、passed 2634 / failed 0 / ignored 7）、
  `cargo clippy --workspace --all-targets -- -D warnings`、`corepack pnpm@11.27.0 -C gui typecheck`、`lint`（info 2 件は既存の scripts/）、`test`
  （75 ファイル / 1144 件）、`gen:types` の後 `git diff --exit-code gui/app/celeris/types.ts` 差分なし。

### 未解決事項・提案

- 本番の反映は未（release・昇格は人）。昇格後に確かめること: (a) benchfs で「案件計画を提案させる」→ `project_plan_proposed` → 承認、(b) 01M3MBV3… の
  やり直しのような retry で複製に `execution_gated` が付くこと。**既に走っている 01M3MFS5T52FXA63W4V10XGC4S の `routing.execution` は古い判定のまま**
  （計画は採用済みなので動作には影響しない。GUI の gate 表示だけが古い。DB は直していない）。
- 案件の events 列（`project_updated` を GUI の履歴に出す）は migration が要るので見送った（P4）。要るなら次の schema 変更と一緒に。
- `blocked` の Task に compound を書いても、回答で `ready` に戻るまで planner は起きない（P1）。人が「今すぐ計画から」と言うなら、回答と同時に使う必要がある。
- CoS（対話 run）の道具には decompose を入れていない（CoS の `create_task.execution` は従来どおりヒント +2）。CoS が既存タスクの再分解を人に提案する経路は
  別に考える。
### F6-fix: mobile-audit の違反 7 件（tap-target 2、perf 5）（2026-09-28）

main f066c84 の release gate が `pnpm-mobile-audit` で落ちた（`routes=27 schemes=2 violations=7 perf_worst=task-overview 598.4KB`）。

- **tap-target（project-detail の light / dark）**: F6 で足した「進め方」の radio を包む `<label>` が `flex items-start gap-2 text-sm`
  だけで高さ 40px（319.0x40.0 < 44x44）。ADR-0055 D1-2 の既存の作り（`~/components/ui/form.ts` の `chipLabelClass`、`min-h-11`。
  `/projects`・`/board`・`/reports` のチェックボックスと同じ）に揃えた（`w-full`、radio は `shrink-0`）。
- **perf（task-overview / timeline / changes / files / artifacts、light）**: F6 の「実行の形」カード（`ExecutionModeControl`・`~/lib/execution-mode.ts`・
  確認文言・`TaskDecomposeFlash`）が `tasks.$id` の初回チャンクに静的 import で載り、初回 JS が 537.2KB > 532KB。予算（ADR-0055）は変えずに:
  `ExecutionModeControl` を `React.lazy` + `Suspense`（fallback なし）にし、`TaskDecomposeFlash` を `Flash.tsx` から `ExecutionModeControl.tsx` へ移した
  （初回チャンクの `Flash` に残らないように）。これだけだと 531.6KB（余裕 0.4KB）だったので、判断待ちがある review タスクでしか出ない
  `HumanReviewPanel` も同じく `React.lazy`（fallback は Skeleton）にした。
- 数字（mobile-audit の `report.json`、`load` までの script の Content-Length 合計）: task 系 5 画面の初回 JS **537.2KB → 525.0KB**（予算 532KB、余裕 7.0KB、
  26 チャンクのまま）。`tasks._id-*.js` 82,771 → 71,929 bytes、遅延チャンク `ExecutionModeControl-*.js` 6,267 / `HumanReviewPanel-*.js` 6,524 bytes。
- 関門（すべて exit 0）: `corepack pnpm@11.27.0 -C gui typecheck`、`lint`（info 2 件は既存の scripts/check-resume-recovery.mjs）、`test`（75 ファイル / 1147 件）、
  `build`、`MOBILE_AUDIT_SKIP_BUILD=1 pnpm mobile-audit`（`routes=27 schemes=2 violations=0 perf_worst=task-overview 586.5KB`）、`E2E_SKIP_BUILD=1 pnpm e2e:mock`（ok）。
- 再現時（修正前）の audit は上記 7 件に加えて `knowledge-skill-edit` の LCP 8648ms（load1 ≈ 12 の高負荷下の揺れ。修正後の実行では出ていない）も出した。
- 気付き（未対応）: build が `INEFFECTIVE_DYNAMIC_IMPORT` を出している（`task-changes.tsx` / `task-files.tsx` は兄弟ルートから静的 import されているので
  Phase 77 の `React.lazy` が別チャンクに分かれていない）。今回の予算超過とは別件。

## F7: 認可要求の自動クローズ（2026-09-28）

**人の報告**: 「認可待ちのところに dogfood 時のすでに不要な認可待ちが溜まっている。消すのと、認可元のタスクを手動でキャンセルした
場合に自動で消えるように修正して。」

**事実（本番、読み取りのみ）**: `GET /approvals?pending=true` に 1 件（01M3HVC62B3WQGF8CKKACAR627）。認可元のタスク
01M3HS2E19BRC021ZXMDZANP5B（dogfood 3 回目）は 2026-09-27 に人が取り消し済み。ナビのバッジ（`DaemonSnapshot.approvals_pending`）は 1。
`POST /approvals/{id}/decide {decision:"denied"}` は **409 invalid_transition**（「status=Cancelled … only blocked tasks accept an answer」）を
返したが、決定は書かれて一覧とバッジからは消えた（半端な副作用）。本番の残りはこの手動の決定で既に 0 件。

**原因**:
- `approvals` 行は run が `Question` で終わったときに追記するだけ（`crates/task-dispatch/src/approvals.rs` `record_question_approval`）で、
  タスクが終端になっても閉じる処理がどこにも無かった（終端化の伝播は `crates/task-core/src/store.rs:2645`〈変更前〉の
  `cascade_after_transition_tx` だけで、`approvals` を見ない）。
- `crates/task-ops/src/approval.rs:64`（変更前）で `approval_decide`（決定を書く）→ `:76` で `gate::answer`（タスクに答える）の順。
  後者が `InvalidState` で失敗しても前者は戻らない。

**修正**（ADR-0033 追記 2026-09-28）:
- `Decision::Withdrawn`（`"withdrawn"`）を追加。celeris だけが書く（人が送ると 422）。DB は既存の TEXT 列（**migration 無し**）。
- `SqliteStore::apply_transition_tx`: 非終端 → 終端（done / failed / cancelled）の遷移と同じトランザクションで、そのタスクの未決の行を
  `withdrawn`（`answer = "task <status>: 認可元のタスクが終わったため、celeris が自動で取り下げました"`）にし、
  `Event::ApprovalsWithdrawn {approval_ids, task_status, reason:"task_terminal"}` を追記（`crates/task-core/src/approval.rs`
  `withdraw_pending_for_task_tx`）。連鎖（子・後続・案件/途中目標の中止）も同じ関数を通るので同じく閉じる。
- 照合: `ApprovalStore::approval_withdraw_stale`（未決で、タスクが既に終端の行を閉じる。`reason:"reconcile"`）をディスパッチャの tick で呼ぶ
  （`dispatcher.rs` の `tick` に 1 呼び出しだけ）。`record_question_approval` は追記の直前にタスクを読み直し、終端なら追記しない。
- `task_ops::approval::decide`: **書く前に**タスクの状態で分ける。`blocked`（途中確認ではない）→ 従来どおり答える。終端・タスク無し →
  決定だけ記録して 200 + `note`（`transition` 無し）。それ以外 → 409、何も書かない。
- 部をまたぐ委譲の判定（`conversation::cross_authorization`）は `withdrawn` を「未決」と読む（`denied` の「もう聞かない」にしない）。
- 他の表面の確認: 質問（`blocked` の worker_question）・途中確認（`blocked` + `awaiting_human`）・`DaemonSnapshot.awaiting_human`
  （メモリ上の集合、`recover_reviews` が reviewing 以外を落とす）・受信箱の `approvals`（`kind=approval` かつ `ready`）はどれも生きている
  タスクの状態から毎回導くので、終端のタスクについて残る永続の表現は `approvals` 行だけ。本番の 30 件の `awaiting_human` の遷移イベントは
  履歴で、どの画面の件数にも入らない。
- GUI: `withdrawn` のラベル「取り下げ（元のタスクが終了）」（`~/lib/labels.ts`）、決めたものの履歴で `warning` の色
  （`decidedApprovalBadgeTone`）、Console の認可ブロックもラベルで表示、決定の結果の `note` を Flash に出す。
- API 文書 `docs/gui/api.md` §3.56 / §3.57 / events の `types` 語彙（→ `scripts/sync-gui-docs.sh` で `gui/docs/celeris-api-v1.md` に反映）。
  スキーマ `docs/api/v1/{api-v1,event}.schema.json`、`gui/app/celeris/types.ts` を再生成。

**証拠**:
- 試験（新規）: task-core `approval::tests`（取り消しで閉じる + イベント 1 件・人の決定は触らない・他タスクは残る / 閉じるものが無ければイベント無し /
  後続〈dependency_failed〉と `kind=approval` の子の連鎖 / running→failed で閉じ blocked→ready では閉じない / 照合は終端のタスクだけ・冪等）、
  task-dispatch `approvals::tests`（終端のタスクの質問は追記しない / 照合が残りを閉じる）、task-api `tests/approvals.rs`
  （`POST /tasks/{id}/cancel` の後に `pending=true`・`approvals_pending`・受信箱の questions から消え `pending=false` に `withdrawn` で残る、
  `types=approvals_withdrawn` / 終端のタスクの認可に `denied` → 200 + note・`Answered` 無し / `ready` のタスクの認可 → 409 で未決のまま・
  `withdrawn` を送ると 422）、task-ops `conversation`（`withdrawn` は pending と読む）、GUI `test/unit/approvals.test.ts`（3 件）。
  既存の `dispatcher::tests::child_failure_question_is_not_repeated_…` は「未決 1 件」→「全 1 件で `withdrawn`」に直した（親が done になるため）。
- `cargo fmt --all -- --check` → 0。`scripts/dev/test-parallel.sh` → 0（passed 2630 / failed 0 / ignored 7、87 binaries）。
  `cargo clippy --workspace --all-targets -- -D warnings` → 0。GUI `typecheck` 0 / `lint` 0 / `test` 0（74 files, 1131 tests）/
  `gen:types` の再生成で差分ゼロ（コミットした types.ts と一致）/ `scripts/sync-gui-docs.sh --check` 0。
- 1 回目の test-parallel（load 17）で `e2e::api_scenarios::api_enforces_token_host_and_workspace_boundaries_…` が replay の MISMATCH
  （status replayed=Running stored=Ready）で落ちた。単独 1 回・`--stress-count 8` で全部通り、2 回目の全体実行でも通った。replay は `Created` /
  `Transitioned` しか読まず、F7 の変更は遷移のイベントの並びを変えないので、高負荷時のタイミング依存（既知の型）と判断した。

**未解決・提案**:
- 既存の install の残りは次の tick で `reconcile` として閉じる（本番は既に 0 件なので、昇格しても何も起きない想定）。
- 取り下げた行を一覧から消す（隠す）かは GUI 側の判断。今は「決めたものの履歴」に `取り下げ` として残る。

## release 6fe28711bab6 の昇格（2026-09-28 19:18:07Z）と browser task の詰まり

- release `6fe28711bab6`（main 6fe2871 = F5-fix6 742f977 + F6 f066c84 + F7 35992b5 + run ログ表示 task の配送 3c08f81 + F6-fix 8e9aee5 + EVENT_TYPES の要素数修正）。
  ゲート: cargo-test 82 s（nextest）、GUI audit 0 件。verify ok / live_ok、schema 29。ライブ昇格（backup 20260928-191751-pre-6fe28711bab6）。
- 最初のゲートは `pnpm-mobile-audit` で 7 件（F6 の tap-target 2 + 初期 JS 予算超過 5）→ F6-fix。2 回目は F6 と F7 の統合で `EVENT_TYPES: [&str; 32]` に
  33 要素（`execution_hint_set` と `approvals_withdrawn`）が入りコンパイル失敗 → 1 行修正。
- browser task 01M3MFS5T52FXA63W4V10XGC4S（gate=on で retry した compound の本番確認）: plan v1 の 7 WU が done → review で基準 0（task 直下の report.md 無し）
  不合格 → replan v2（remerge / reship 追加）→ remerge が task ブランチに直接 commit（7725ed6）したため `celeris-wu/<task>/remerge` が無く、
  reship の worktree 準備が「dependency branch of remerge does not exist yet」で 2 秒ごとに失敗し続け、event も無く task が `ready` に見えた（18:58〜19:19Z）。
  人が branch を作って解消。修正は F5-fix7 として委譲（依存 WU のブランチ解決の規則と、準備失敗を blocked + event で見せる）。

## F5-fix8: pegasus の ssh master を長く保ち、無駄な再接続をやめ、切断を数えて知らせる（ADR-0078、2026-09-28）

タスク 01M3MQQ9CQT3Q1NQGKPCT4H0BR（WorkUnit `fix`）。ADR-0078 D1〜D5 の実装（未昇格）。

**変更**（ADR の決定ごと）:
- D1: master の argv（`start_publickey` / `start_totp`）に `-o ControlPersist=yes`（`persist_args`、`-M -N` の前）。`[[clusters]] control_persist`
  （既定 `"yes"`、`"yes"` か正の秒数だけ。他は `Config::validate` のエラー）。`scripts/cluster-login.sh` にも `ControlPersist=yes` と keepalive。
- D2: `crates/celeris/src/control_path.rs`。起動時（verify 以外）に背景スレッドで `ssh -G <host>` を読み、ControlPath の `none`・90 バイト超・
  親ディレクトリの所有/0700・`XDG_RUNTIME_DIR` 下で Linger 無し・`controlmaster no` を warn、人の `controlpersist` を info で 1 回ずつ。
- D3: 鍵認証の再接続は totp / manual で切断 1 回につき 1 回、publickey は 6 秒から倍々で 5 分まで。実通信 probe は別スレッドで走らせ tick を
  塞がない。probe の失敗で `-O exit` しない（片付けフックを削除）。3 回連続で lost。
- D4: `Dispatcher::set_cluster_connected` を `cluster_connected` の唯一の書き込み口にし、true→false で `ClusterLoginNeeded`（本文に切れた時刻・
  接続していた時間・推定の理由）を outage ごとに 1 件。publickey は再接続 3 回失敗で `ClusterUnavailable` の報告 1 件。GUI の「クラスタ」画面に
  直近 24 時間の回数と「最後に切れた時刻（理由）」。
- D5: log の固定文言 `cluster ssh master connected`（method, attempt）/ `cluster ssh master lost`（cause, uptime_secs, last_tick_gap_ms）/
  `cluster key-auth reconnect attempt`（ok, backoff_secs）/ `cluster totp prompt relayed`。記録は新しい表 `cluster_connection_log`
  （migration 0030、**schema 29 → 30**）。`GET /api/v1/clusters` の各クラスタに `stats.last_24h`（DB から）と `stats.since_start`（スナップショット）。
  Event にしなかった理由は ADR-0078 §7。

**証拠**:
- 試験（新規）: task-worker `cluster_login::tests`（`ControlPersist=yes` が両経路で `-M` より前 / 設定の秒数・空は付けない /
  偽 ssh で `a_control_persist_yes_master_survives_a_daemon_restart_gap`〈`-O check` が 1.5 秒途切れても master が残り、新 daemon は借りて
  master を張り直さない〉と対照の `without_control_persist_yes_the_master_dies_in_the_restart_gap`〈消えるまで読み切ってから判定〉）、
  celeris `config`（control_persist の既定と検証）・`control_path`（7 件）・`tests/cluster_login_script.rs`（偽 ssh で script の argv）、
  task-dispatch `dispatcher::tests`（probe 1〜2 回の失敗は接続中・3 回で lost 1 件 / 遅い probe が refresh を塞がない / totp で 100 tick 回しても
  鍵認証 1 回・通知 1 件、繋ぎ直した後の切断でもう 1 回 / publickey のバックオフ 6→…→300 と 3 回失敗の報告印 / 遷移ごとに lost 1 件・通知 1 件・
  master_exited も同じ経路 / dispatcher を作り直しても既存 master を `borrowed` で借り connector を呼ばず、DB の回数は同じ）、
  task-core `store::tests::cluster_connection_log_records_and_lists_since`、GUI `clusters.test.ts`（`clusterConnectionSummary` 2 件）。
- `cargo test --workspace --no-fail-fast` → exit 0（passed 2674 / failed 0 / ignored 7、99 binaries）。
  `cargo clippy --workspace -- -D warnings` → 0、`--all-targets` も 0。`cargo fmt --all -- --check` → 0。
  `UPDATE_SCHEMA=1 cargo test -p task-api --lib schema` で `docs/api/v1/api-v1.schema.json` を再生成、GUI `gen:types` で `types.ts` を再生成。
  GUI `typecheck` 0 / `test` 0（75 files, 1149 tests）/ biome（変更ファイル）0。
- pegasus への新規ログインは無し（テストは偽 ssh とフックだけ）。

**未解決・提案**:
- 本番での確認は人の操作が要る（ADR-0078 §6-6。TOTP 1 回）: 昇格後に GUI で pegasus に 1 回接続 → `systemctl --user restart celeris@<sha>` 1 回と
  次のライブ切替 1 回を経ても `GET /api/v1/clusters` の pegasus が `connected=true`、`stats.last_24h.connects_totp` が増えず、
  journal に `cluster ssh master lost` が出ないこと。確認の ssh は `ssh -O check pegasus` だけ。
- schema 30 に上がるので、昇格後に旧 release へ戻すと `SchemaTooNew` で起動しない（戻すときは backup から）。
- 人の `~/.ssh/config` の `ControlPersist 10` → `8h`、`ServerAliveInterval 30` の提案は ADR-0078 §4（任意。celeris の master には不要）。
- 次は release WorkUnit（`scripts/selfdeploy/release.sh` / `verify.sh`）。
## F5-fix7: 依存 WU のブランチが無いと WU の準備が無音で失敗し続ける（2026-09-28）

### 症状（本番 2026-09-28 18:58〜19:19Z、task 01M3MFS5T52FXA63W4V10XGC4S、plan v2 01M3MMTCWK98A11E6HAJ1YS0CN）

- review_fail の後の replan（plan v2）が、工程 `remerge` に WU `remerge`（kind `repair`、main を Task ブランチに merge して衝突を解く）と
  `reship`（kind `release`、depends_on: remerge）、`integrate-remerge` を足した。
- `remerge`（codex、run 01M3MMVP19EJCN7J9WVJXM9TF7）は 18:58:35Z に done、`work_unit_committed {key: remerge, branch:
  "celeris/01M3MFS5T52FXA63W4V10XGC4S", commit: 7725ed6167cb…}`。`work_units` の行は `branch` 空・`base_commit` 空・
  `head_commit` 7725ed6167。`reship` は `dependency_ready` で ready、Task は `advance` で ready。
- 以後 20 分間、tick ごと（約 2 s）に WARN `cannot prepare the work unit worktree; not dispatching this tick … work_unit=reship
  error="dependency branch of remerge does not exist yet"`。イベントは 1 件も無く、Task は ready のまま（GUI の inbox に出ない）。
  `celeris-wu/<task>/*` は adopt-prior・design-gap・mvp-audit・release だけ。コーディネータが `celeris-wu/<task>/remerge` を
  7725ed6 に手で作ると、reship は直ちに dispatch された。

### 根本原因（修正前の file:line、`crates/task-dispatch/src/dispatcher.rs`、HEAD 6fe2871）

- L8395 `if !mode.worktrees || wu.kind == task_core::WorkUnitKind::Repair { return Ok(None); }`: kind `repair` の WU は（統合の
  repair WU に限らず、planner が書いた repair WU も）WU の worktree を切らず、Task の worktree で走る。完了時の commit は
  `work_unit_trees`（L8504 付近）が `wu.branch == None` のため Task の worktree を返し、Task ブランチに入る。WU ブランチは作られない。
- L8412〜8441: 同じ工程の依存先（`intra_dep`）があると、基点は `refs/heads/celeris-wu/<task>/<dep>` だけを見て、無ければ
  `"dependency branch of {dep} does not exist yet"`。依存先の行の `branch` / `head_commit` / `integrated_commit` / `base_commit` を
  見ない。依存先は既に done なので、この ref は二度と現れない（"yet" ではない）。
- L10621: `prepare_work_unit_workspace` の `Err` はすべて WARN を出して `return Ok(false)`。回数も分類もイベントも無く、
  WU・Task の状態を変えないので、次の tick が同じことを永遠に繰り返す。

### 修正（schema 変更なし）

- `crates/task-dispatch/src/integration.rs` `dependency_base`（新設）: 依存先の基点の規則（ADR-0074「Phase F5-fix7 実装時の明確化」2.）。
  WU ブランチ → （`branch == None` なら）`head_commit` → `integrated_commit` → `base_commit` → Task ブランチの HEAD、
  （`branch` があるのに ref が無ければ）`head_commit` → `base_commit`、どれも無ければ `Err`。ref は作らない（同 1.）。
- `dispatcher.rs` `prepare_work_unit_workspace`: 依存先の行を渡して `dependency_base` を使う。エラーは `WuPrepareError
  {message, permanent}`（依存先が解決できない・Task ブランチが無い = permanent、それ以外 = transient）。
- `dispatcher.rs` `on_work_unit_prepare_failed`（新設）と dispatch の入口: transient は連続 `MAX_WU_PREPARE_ATTEMPTS = 5` 回目まで
  `wu_prepare_backoff`（2/4/8/16 s、上限 60 s。`wu_prepare_failures`、プロセス内メモリ）で待ってやり直す。permanent は 1 回目、
  transient は 5 回目で WU を `blocked(question)`（`WorkUnitTransitioned{reason: "prepare_failed"}`）にし、`worker_progress`
  （`prepare_failed: work unit <key>: <error>`）を残す。Task が Ready なら `approvals` に 1 件・`QuestionRaised`・`Trigger::Unroutable`
  （`ready → blocked`）。並列の 2 本目以降（Task は Running）は WU だけ blocked にし、兄弟の終了時の `settle_phase` が Task を止める。
  回答（`Trigger::Answer`）で WU は D18 の `answer` の経路で ready に戻る。
- `crates/task-ops/src/replay.rs`: `prepare_failed` の blocked を `Question` と読む（WU の行を events から作り直せる）。
- ADR-0074 末尾に「Phase F5-fix7 実装時の明確化」（1. WU ブランチを作らない理由、2. 基点の規則、3. 失敗の扱い）。

### 証拠

- 新しいテスト（`crates/task-dispatch/src/dispatcher/tests/work_unit_dependency_base.rs`）:
  - `a_dependent_of_a_unit_that_committed_on_the_task_branch_dispatches`（本番の形: repair `remerge` が Task の worktree で
    `remerge.txt` を commit → `WorkUnitCommitted.branch == celeris/<task>`、`remerge.branch == None`、`celeris-wu/<task>/remerge`
    は作られない。`reship` は `remerge.txt` が見える worktree で走り `base_commit == remerge.head_commit`、Task は Done、
    `prepare_failed` 0 件、replay の差分 0）
  - `a_dependent_of_a_unit_that_made_no_commit_bases_on_the_task_branch`（repair が何も変えずに done → reship の基点は Task
    ブランチ（main）、Done）
  - `an_unresolvable_dependency_blocks_the_unit_once_and_asks_a_human`（依存先 a が done・`branch` あり・ref 無し・`head_commit`
    がリポジトリに無い → 最初の tick で b が `blocked(question)`・`prepare_failed` 1 件・`QuestionRaised`（"dependency branch of a"）・
    `worker_progress`・Task Blocked、その後 20 tick 回しても `prepare_failed` は 1 件のまま、replay の差分 0。人が
    `celeris-wu/<task>/a` を作って `Answer` → b が ready に戻り、a のブランチから切られて走り Done）
  - `transient_prepare_failures_retry_with_backoff_then_block`（git の錠の transient: 1〜4 回目はイベントなし・Task Ready・
    `retry_at` が未来、5 回目で `prepare_failed`・Task Blocked。バックオフ 2 s / 16 s / 60 s 上限）
  - `integration::tests::dependency_base_falls_back_to_the_recorded_commits_and_the_task_branch`（WU ブランチ優先・repair の
    head・commit の無い repair = Task ブランチの HEAD・記録した head が無い repair = Task ブランチの HEAD・統合 WU =
    `integrated_commit`・ref の消えた WU = head → base・解決できなければ `Err`）
- 修正を外して確認: `dependency_base` を「WU ブランチが無ければ `Err`」に戻す → 依存の 2 本（task ブランチに commit した依存先・
  commit の無い依存先）が FAILED。`on_work_unit_prepare_failed` を「何もしない」（修正前の WARN して戻るだけ）にする →
  `an_unresolvable_dependency_blocks_the_unit_once_and_asks_a_human` と transient の 1 本が FAILED。戻して 4 本とも ok。
- 全体ゲート（`CARGO_TARGET_DIR=/var/lib/celeris/scratch/targets/agent-depbranch/target`、`RUSTC_WRAPPER` 無し）: 下の「ゲート」。

### 未解決事項・提案

- 本番の task 01M3MFS5T52FXA63W4V10XGC4S は、コーディネータが手で作った `celeris-wu/<task>/remerge` のまま進んでいる
  （本 fix は本番に触れていない。DB は read-only の照会だけ）。この ref は 1. の規則とは食い違う（`branch == None` の WU に
  ref がある）が、害は無い（統合の merge 対象は行の `branch` から決まり、remerge は葉でもない）。
- 並列の 2 本目以降で blocked にした WU の質問の本文は、兄弟の終了時に既定の「WorkUnit <key> が質問しています」になる
  （理由は `worker_progress` にある）。`deferred_work_unit_trigger` が `prepare_failed` の進捗を読めば本文に出せる（今回はやらない）。
- transient の回数はプロセス内メモリなので、再起動で 0 から数え直す（上限は再起動ごとに 5 回。無限にはならない）。
- `Transitioned.reason` は `unroutable`（`Trigger::Unroutable` を借りたため）。専用の trigger を足すかは transition の表の変更なので
  提案に留める。
- planner が kind `repair` を「main を merge する」WU に使うと、その WU は Task の worktree で走る（同じ工程で依存の無い repair WU が
  2 本あれば Task の worktree を共有して並列に走りうる。未確認）。planner のプロンプトで repair の用途を明示するか、repair を Task の worktree に倒すのを統合・
  最終レビューの repair（`RepairScheduled` のある WU）に限るかは次の判断に回す。
- 本番への反映は昇格待ち。

## gate=on での ExecutionPlan 経路の本番確認（browser capability task 01M3MFS5T52FXA63W4V10XGC4S、2026-09-28 17:07〜20:11Z）: done

- 人が gate=on / parallel=true で再起動（17:06Z）後、cancel した atomic task を `retry` で複製（17:07Z）。planner（Opus、99 s）が plan v1（adopt / harden / ship の 3 工程、WU 7 =
  adopt-prior / design-gap / mvp-audit / release + 統合 3）を作り、cancel 前の run が worktree に残した ADR-0078・docs・MVP コードを adopt-prior で回収。
- WU 7/7 done → 最終 review で基準 0（task 直下の report.md 無し）と main との衝突を指摘され不合格 → replan v2（remerge / reship 追加）→ remerge が main を取り込み
  → reship の準備が依存ブランチ不在で無音停止（F5-fix7 の発端、人が branch 作成）→ reship done → 統合 → 最終 review 全 11 基準合格 → 20:11Z done。
- 指標: WU 10/10、run 10（planner 2 / worker 6 / reviewer 2）、replan 1、cost_usd 6.45、壁時計 3h04m（うち無音停止 20 分）。人の介入 1 回（branch 作成）。
- 配送: task ブランチが main 06e9a03 を統合済みで fast-forward 可能 → main は 09a6b0c（browser capability Phase 1 + F5-fix7）。task が作った release
  `5fcb7eebbe9a`（gate ok: cargo test 2678 / GUI 1173 / mobile-audit 0、verify ok / live_ok）は main 09a6b0c と tree が同一なので、それを 20:13:05Z にライブ昇格
  （backup 20260928-201255-pre-5fcb7eebbe9a）。これで F5-fix7 も本番に入った。
- 見つかった不具合: F5-fix7（依存 WU のブランチ解決・準備失敗の blocked 化）、retry が gate 判定を複製する（F6 で修正済み）。

## F5-fix9: 空の replan で task が ready のまま止まる（2026-09-29）

（注: 同じ「F5-fix8」の名前は上の「pegasus の ssh master」節〈ADR-0078〉にも使われている。本節は空の replan の修正。）

### 症状（本番 2026-09-29、task 01M3MZKB3DFYJNBH015MJGQ0BT「ブラウザ capability Phase 2」、/2、gate=on、`max_replans = 3`）

- 02:26:45Z 最終レビュー不合格（criterion 3・5・6・7・8。main a525af2 がタスクブランチの祖先でない等）→ `ready`。
- replan の planner 1 回目（01M3NFSFBGQBQ2RPFQ7PJZ5142）は `too many work_units: 11 > 10` と容量超過で拒否。2 回目
  （01M3NFWQKAW2PNGQYDHGZ8B98T、02:29:47Z）は「done の WU だけで上限いっぱいなので WU は足せない。指摘はもう解消している（HEAD d5ec0cd）」
  として空の差分を出し、`execution_planned {version: 4, reason: "replan (planner run) (added=0, changed=0, removed=0)"}` →
  `running → ready (planned)`。
- plan v4（01M3NFYBEQ1YQXCVN80VRFWWEH）の `plan_id` の `work_units` 行は 0（done の WU 10 は v1 12 / v2 1 / v3 2 の行のまま持ち越し。
  これは正常）。有効な WU 15 行（統合 WU を含む）はすべて done。
- 以後 30 分、Task について event もログも 0 件（02:39Z の停止→起動の昇格を挟んでも同じ）。
- 03:00:41Z 人が `POST /tasks/{id}/execution/decompose {mode: compound, note}` → `execution_hint_set {replan: true}`。その後（read-only
  の照会、03:07Z 時点）: **新しい event は 0 件**、Task は `ready` のまま。daemon のログには 03:00:41Z から tick ごと（約 2 s）に
  WARN `a human replan was requested but max_replans is exhausted; continuing with the current plan`（task_id 同じ、03:07Z までに
  191 行）が出るだけで、planner も review も起きない。

### 根本原因（修正前の file:line、`crates/task-dispatch/src/dispatcher.rs`、HEAD 74faf16）

- L9019（/2・/3）`PhaseSettle::AllDone | PhaseSettle::Failure(_) => self.replan_gate(task_id)`、L8884（/1）`NextStep::AllDone =>
  self.replan_gate(task_id)`: 仕事の残っていない計画の `ready` の Task は、**採用の直後でも**必ず replan に回る。コメントの前提
  （「この状態は review 不合格の後だけ」）が、replan で何も足さなかった版の採用で崩れる。
- L12258〜12269 `replan_gate`: 版 4 = replan 3 回 ≥ `max_replans` 3 → `raise_node_replan_limit`（L11653。木の節点でなければ L11661 で
  何もしない）→ `Ok(WuDispatchGate::Skip)`。L13034 `WuDispatchGate::Skip => return Ok(false)`。毎 tick 同じ判定で、event もログも無い。
- 人の依頼の後は L8764〜8768 で `replan_gate` が同じく `Skip` → WARN を出して「今の計画のまま進む」が、進む先が上の `AllDone →
  replan_gate → Skip`。
- /1 で有効な WU が 0 のときは `task_core::next_work_unit`（`crates/task-core/src/execution_plan.rs` L2675）が `Stuck` を返し、L8905 の
  WARN → `Skip` で同じく止まる。
- 見逃した理由: R3b の生存確認（L11218 `check_tree_liveness`）は L11220 で木が無効なら何もせず、木の節点しか見ない。さらに
  `crates/task-core/src/tree.rs` L1748 は unit がすべて終わった節点を常に「走れる: completion（次の tick で最終レビュー）」と分類して
  いたが、dispatcher は実際には最終レビューに進めていなかった。
- 仮説との対応: (1) 正（空の差分が採用され、done の持ち越しだけの版になる）。(2) 正（`ready` から進む経路は unit の完了か replan だけで、
  完了済みの計画を最終レビューに出す経路が無い）。(3) は採らない（下の規則 2.）。

### 規則（ADR-0074 末尾「F5-fix8 実装時の明確化: 空の replan と完了済み計画の進行」）

1. 仕事の残っていない計画（有効な WU がすべて done、または有効な WU が 0）の `ready` の Task は、その版の採用の後に最終レビューの判定が
   まだ無ければ、次の tick で run を起こさず最終レビューに出す（新 trigger `PlanComplete`: `ready → reviewing`、`reason =
   "plan_complete"`、attempts 不変）。判定の後なら従来どおり replan。
2. 空の差分は拒否しない（done の WU が上限を埋めていると planner は WU を足せない。当否は最終レビューが決める。繰り返しは attempts と
   `max_replans` で有限）。
3. 生存確認を、有効な計画を持つ `ready` の Task すべて（木が無効でも）に広げる。unit がすべて終わった節点は 未審査 = 走れる / 審査後で
   replan の余地あり = 走れる / 余地なし = 理由なし `replans_exhausted`。通知 key は木でなければ `stall:<id>:<seq>`。

### 修正（schema・migration の変更なし）

- `crates/task-core/src/transition.rs`: `Trigger::PlanComplete`（表のテストを 4×8×22 に）。
- `crates/task-core/src/execution_plan.rs`: `plan_work_finished`（有効な WU がすべて done / 0 件）と `plan_awaits_final_review`
  （採用の後に最終レビューの判定が無いか。純粋関数）。
- `crates/task-dispatch/src/dispatcher.rs`: `wu_dispatch_gate` の scheduler の前に `plan_work_finished` → `finished_plan_gate`
  （`FinalReview` か `replan_gate`）。新しい gate `WuDispatchGate::FinalReview` → `start_final_review_from_ready`（`PlanComplete` +
  `spawn_review`。主題は今の版の `rationale` + 完了した WU の要約）。`check_tree_liveness` は木が無効でも計画を持つ `ready` の Task を見る。
- `crates/task-core/src/tree.rs` / `crates/task-ops/src/tree.rs`: `NodeLivenessFacts.plan_reviewed`、`LivenessUnitFacts.waits_on_child_dep`
  （`child:` の依存を待つ pending は名指しの待ち）、`planner_pending` は人・途中確認の replan を `max_replans` の余地があるときだけ数える。

### 証拠

- 新しいテスト（`crates/task-dispatch/src/dispatcher/tests/finished_plan.rs`）:
  - `empty_replan_after_review_fail_goes_to_final_review`（事故の再現: v1 の 3 WU done → review_fail → planner の空の差分で v2 採用
    〈`added=0, changed=0, removed=0`〉、`max_replans = 1` で使い切り → `plan_complete` → 2 回目の審査で `done`。worker run 3・planner 1、
    replay の差分 0）
  - `a_plan_with_no_work_left_proceeds_to_review_without_a_run`（/2 の計画の WU〈統合 WU 含む〉がすべて done / すべて cancelled → run 0 本で
    `plan_complete` → `done`、replay の差分 0）
  - `a_stalled_non_tree_plan_is_detected`（木は無効、全 done・審査の後・`max_replans = 0` → 偽の時計で 599 s までは無し、630 s で
    `StallDetected{replans_exhausted}` と通知 `stall:<id>:` を 1 回だけ、以後繰り返さない。run 0 本）
  - `liveness_does_not_flag_runnable_non_tree_plans`、`task_core::tree` の `running_and_runnable_nodes_are_live` に全 done / unit 0 /
    審査後の 3 ケースを追加。
- 修正を外して確認: `wu_dispatch_gate` の `plan_work_finished` の分岐を無効にすると、上の 1 本目と 2 本目が FAILED（`ready` のまま）。
  戻して 4 本とも ok。
- 既存テストの変更: `tree_approval::tree_disabled_is_unchanged` の `liveness_checked_at.is_none()` の assert を外した（木が無効でも計画を
  持つ Task の生存確認は走るため。`StallDetected` が出ないことの assert は残した）。
- 全体ゲート（`CARGO_TARGET_DIR=/var/lib/celeris/scratch/targets/agent-emptyplan/target`、`RUSTC_WRAPPER` 無し）: 完了報告に記載。

### 未解決事項・提案

- 本番の task 01M3MZKB3DFYJNBH015MJGQ0BT は、この修正の昇格後の最初の tick で `plan_complete` → 最終レビューに進むはず（v4 の採用の後に
  判定が無いため。人の replan の依頼は `max_replans` を使い切っているので WARN のまま、今の計画で進む）。本 fix は本番に触れていない
  （DB は read-only の照会だけ）。人の note（main の取り込み・release/verify）を反映させたければ、レビューの不合格の後に `max_replans` を
  上げる（木の節点なら `limit:max_replans` の決定）か、別 task にする必要がある。
- **done の WU が `max_work_units`（10）に数えられる**ため、done が 10 に達した /2 の計画の replan は WU を 1 つも足せない（1 回目の
  planner が 11 > 10 で拒否された原因）。done の持ち越しを上限から外すか、replan では上限を「新しく足す WU の数」に数えるかは提案に留める。
- 人の明示の replan の依頼（`execution_hint_set{replan: true}`）が `max_replans` の使い切りで黙って（WARN だけで）落ちる。依頼を受けた
  時点で API が 409 を返すか、`worker_progress` を 1 件残すかは次の判断に回す。

**昇格と本番確認**: release `a8ed75460c78`（main a8ed754 = R3b + F5-fix9）を 2026-09-29 03:28:42Z にライブ昇格（backup 20260929-032832-pre-a8ed75460c78、verify ok / live_ok、schema 31）。昇格の 4 秒前まで `ready` で止まっていた Phase 2 task 01M3MZKB3DFYJNBH015MJGQ0BT は 03:28:38Z（新 daemon の最初の tick）に `transitioned ready→reviewing (plan_complete)` で最終 review（reviewer run 01M3NKA4F1P2Z1R51PTJ3NY6A2）に進んだ。


## browser capability Phase 2 task（01M3MZKB3DFYJNBH015MJGQ0BT、人が GUI で起票、2026-09-28 21:46〜09-29 03:34Z）: 内容合格・祖先条件だけ不合格 → 人が統合

- compound（gate=on、hint）で plan v1〜v4、WU 15/15 done、run 24（planner 7 / worker 15 / reviewer 2）、replan 3、cost_usd 26.4。
- replan 連鎖の原因: (1) main の schema が 31 に進んでいるのに本番が 29 のままで task の release が live_ok=false（02:39Z に本番を 31 へ昇格して解消）、
  (2) 私が R2a〜R4a を main に統合し続けたため「main が task ブランチの祖先」の基準（8）が毎回崩れた、(3) `max_replans` 超過後に `ready` で無音停止（F5-fix9）。
- 最終 review（03:34Z）は基準 0〜7 合格、基準 8 のみ不合格 → failed。成果ブランチ d5ec0cd に main（85cd482 → 6a976d2）を取り込み、
  `transition.rs` の衝突（BrowserWait/Resume/Fail と PlanGate/PlanComplete）を解消して ce5d768 とし、人（Fable）が main に fast-forward で統合。
  migration 0032（browser_waits）/ 0033（browser_task_policies）で schema 33 → 昇格は停止→起動。ゲート: 2898 passed、GUI 1213、mobile-audit 0。
- 教訓（ADR-0079 が構造的に解決する点）: 根 task の review 中に main を動かすと祖先条件で落ちる。R1c で子 task は親ブランチ基準になったが、根は main 基準のまま。
  運用上は「根 task の最終 review 中は main への統合を控える」か、review 基準を「fast-forward 可能」から「衝突なく merge 可能」に緩める（提案 P-R-1）。
