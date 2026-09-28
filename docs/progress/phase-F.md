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
