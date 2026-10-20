# 最小 pilot（mlsys-main-2）の不具合: 予算切れ後の再開・B 成果物・decision-log の欠け・runbook

- 日付: 2026-10-20
- 状態: 決定（実装は後続 unit: budget / collect / declog / runbook）
- task: 01M5C8KF8Y3MB6TT064AYV2F23
- 関連: ADR 2026-10-15-decision-log-export、ADR 2026-10-20-grade-failure-class-and-denominators、ADR-0072 D8/D18（continuation）、
  `docs/ops/measure-run-final.md`

## 証拠（/local/celeris/data/measure/main-2/、読むだけ。測定 daemon は release 302e28550103）

B の試行 task `01M5C7M3RTRH2T6FAVD5QTMSAN`（測定 DB の写し `dl-src/measure-db-copy.sqlite3` の events と runs）:

| run | role | 終わり | tokens（input+output+cache） |
|---|---|---|---|
| 01M5C7M3ZN… | worker seq2 | `budget_exhausted`（**turns**。task budget は `{"max_turns":10,"max_wall_secs":1800,"max_retries":2}`、token の上限は無い） | 295,087 |
| 01M5C7NKE6… | worker seq3（`continue: budget_exhausted(turns) の続き（Run #2）`、session resume） | completed | 466,412 |
| 01M5C7QJTC… | reviewer | completed（review_pass → task done） | 289,591 |

- 合計 1,051,090。campaign の per_case は 300,000。`runs/watch-2.json` は `max_overshoot_tokens: 751,090`、`case_budget_cancelled: 1`。
  campaign は Celeris の `GET /tasks/{id}` の `runs[].usage`（run 終了時にだけ入る）しか見ないので、3 run が終わってから上限超過を知った。
- B の worker の作業場所は `state/ws/<task>/repos/scratch`（task-template の `repos:["scratch"]`、`kind=dir` の
  `state/scratch-repo` への symlink）。task の `workspace`（trial workspace）は cwd にならず、worker は `materials/…/task.md` を
  見つけられずに `out/snapshot` を探し回った（events seq 11–21）。成果物は `state/scratch-repo/docs/k4-cmp4-validity-analysis.md`
  にでき、trial workspace（`out/trials/B/…/rep1/workspace`）は seed のまま。grade は `model_failure`。
- `scratch-repo` は案件の 1 つの dir を全 B 試行が共有する（試行間で成果物が混ざる）。
- A も `model_failure`: grade の recompute が `inputs/runs/43706-…/cells/…/cell.json is missing`。case.json の `inputs[].fetch`
  は「`cd inputs && tar -xzf …`」と「`report.md`・`results/cells.csv`」を求めるが、worker に渡る objective
  （`materials/<case>/task.md` と「公開 case 定義は crates/campaign/cases/final/<case>/case.json」）の case.json は trial workspace
  に無い。A も B も出力の契約を読めなかった。
- `runs/dl-export.out`: records 15 件、`--verify-connected` は exit 1。worker 2 run の `grader_verdict` 欠け、reviewer run の
  `proposed_first` と `grader_verdict` 欠け。
- `runs/watch-1-refused.err`: `out has data but no lock.json`。runbook §5 の `sh $L watch … > $OUT/final-run-N.json` は shell が
  先に空 file を作るので、初回 watch の lock 判定がそれを既存データとみなす。
- reviewer run の events に `routing_decided` が無い（seq 64–66 は `worker_started(role=reviewer)`・`harness_selected`・
  `llm_route_selected` だけ）。`llm_route_selected.metadata.candidates` と `route_decision.routing_resolved` はある。
- `campaign_verdict`（export の `grader_verdict`）を書く本番の writer がコードに無い（`decision_log_wire.rs` の試験だけが書く）。

## D1. campaign 試行の token 上限: task 予算として渡し、run の途中で観測して止め、上限後は自動で再開しない

**決定**: 「task の token 予算（`Budget.max_tokens`）＋ run 途中の消費観測で止める ＋ 予算到達後は continuation・retry・reviewer を
起こさない」を採る。残り予算の内側での再開は**する**（上限までは continuation を許す）。上限に達したら task は終端にする。

1. `task_core::Budget` に `max_tokens: Option<u64>`（`#[serde(default, skip_serializing_if = "Option::is_none")]`）を足す。
   数え方は campaign の `token_basis = "all"` と同じ input+output+cache_read+cache_creation（task 木の全 run、reviewer を含む）。
   `POST /tasks` の body（task-api `NewTaskBody`）に `max_tokens` を足し、`Budget` に入れる。schema を再生成する（gui/web の生成型も）。
2. campaign の `CelerisExecutor::body`（`crates/campaign/src/celeris_exec.rs`）が `max_tokens = manifest.budget.per_case.max_tokens`
   を入れる。`check_template` は template の `max_tokens` を拒否する（実行器が埋める欄）。
3. run の途中の観測: claude-code adapter（`crates/task-worker/src/claude_code.rs`）は `assistant` 行の `message.usage` を run 内で
   累積し（`observe_assistant_usage` の隣に足す。sub-agent 行は数えない — 既存の規則と同じ）、新しい非終端の
   `WorkerMessage::UsageProgress { usage }`（累積値。追加のみなので `PROTOCOL_VERSION` は変えない）を出す。
   dispatcher は受けたら runs 索引の `usage_json` を途中値で上書きし（events には書かない。終了時の `worker_finished.usage` が確定値）、
   「終わった run の usage の和 ＋ この run の途中値 ≥ `max_tokens`」なら run を止める（既存の cancel / kill 経路）。
   止めた run の終わりは `RunEnd::BudgetExhausted { kind: BudgetKind::Tokens }`（`BudgetKind` に `Tokens` を足す）。
   途中値は `GET /tasks/{id}` の `runs[].usage` に出るので、campaign の既存の poll（`runner.rs` の case_budget 判定）も実行中に超過を見られる（二重の防御）。
4. 再開の抑止: `worker_finish.rs` の continuation 判定（`max_continuations_per_work_unit` の隣）、retry、`review_spawn.rs` の
   reviewer 起動の前に「task 木の消費 ≥ `max_tokens`」を見る。到達していたら起こさず、task を `failed`
   （`outcome = "token_budget_exhausted: <used>/<max>"`）にする。到達前なら continuation は従来どおり（残り予算の内側で走り、
   3. で止まる）。`blocked`（人に聞く）にはしない — campaign の試行は人の判断を待てず、`trial_timeout` まで待つだけになるため。
5. 超過の幅: claude-code では「上限に達した assistant 1 行ぶん」まで（途中値は 1 応答ごとに届く）。上限の判定は応答の後なので
   0 にはできない。受け入れの「ADR で決めた範囲」は **超過 ≤ 最後の 1 応答の usage**（試験の stub では 1 行の usage 値で上界を置く）。
   codex / opencode の adapter は途中値を出さないので、超過は「1 run ぶん」まで（4. の抑止は効く）。campaign の B/C arm は claude-code だけなのでこれで足りる。

**根拠**: 証拠の超過は「turns で切れた後の自動 continuation（466k）と reviewer（290k）」が主で、どちらも Celeris の判断で起きた。
campaign 側だけで止める案（run 終了ごとに cancel）は run 1 本ぶん（今回 466k）超過し得るうえ、continuation を起こしたのは Celeris
なので原因側で止めるべき。「予算切れ後は一切再開しない」案は turns 切れ（今回の 295k は token 上限の内側）で試行を落とし、
B が A（単発 harness）より不利な上限になる — 残り予算の内側の再開は B の機構そのものなので許す。task の予算として持てば、
measurement 以外の task でも同じ仕組みで token 上限が効く（campaign 専用の分岐を作らない）。

**変える file**: `crates/task-core/src/model.rs`（Budget）・`crates/task-core/src/execution.rs`（BudgetKind::Tokens）・
`crates/task-worker/src/protocol.rs`・`crates/task-worker/src/claude_code.rs`・`crates/task-dispatch/src/dispatcher.rs`
（WorkerMessage 受信）・`crates/task-dispatch/src/dispatcher/worker_finish.rs`・`crates/task-dispatch/src/dispatcher/review_spawn.rs`・
`crates/task-api/src/handlers/tasks.rs`・`crates/campaign/src/celeris_exec.rs`・schema（`UPDATE_SCHEMA=1`、gui/web 生成型）。

**試験**（LLM なし。stub worker と `tokio::time::pause`）:
- `crates/task-dispatch` `token_budget_stops_run_midway`: stub worker が `UsageProgress` を 100k ずつ出す。`max_tokens=250k` で
  3 行目の後に run が止まり `BudgetExhausted{Tokens}`、task は failed、超過 ≤ 100k。
- `crates/task-dispatch` `token_budget_exhausted_no_continuation_or_review`: turns 切れで終わった run の usage が上限以上なら
  continuation も reviewer も起きない。上限未満なら continuation が 1 本起きる（残り予算の内側の再開）。
- `crates/task-worker` `claude_code_emits_cumulative_usage_progress`: fixture の stream から累積値が単調に出て、最終値が `result.usage` と一致。
- `crates/campaign` `celeris_body_sets_per_case_max_tokens` と `check_template_rejects_max_tokens`。
- `crates/campaign` `per_case_overshoot_bounded_and_ledger_matches`: 偽 transport で実行中の `runs[].usage` が途中で増え、
  終端の usage が watch の `tokens_total`・台帳（ledger の observed）・final_accounting の `observed_trial_tokens` と一致、
  `max_overshoot_tokens` ≤ 1 応答ぶん。

## D2. B の成果物: trial workspace を B task の作業場所にする（scratch repo から写さない）

**決定**: B/C の試行 task は**案件のリポジトリを選ばず**、`workspace = {"kind":"local","path":<trial workspace>}` を cwd として走る。
回収（写し）の経路は作らない。

1. `celeris_exec::body` は `repos: []` を明示し、`check_template` は template の `repos` を拒否する（`project_id` は可。repos を
   省くと案件の primary repo が補われる場合があるので、空配列を明示して API 側の補完を止める。実装 unit は `POST /tasks` の
   repos 既定の挙動を確かめ、空配列が補完されないことを試験で固定する）。task の作業場所は `task_workspaces_for` の
   2.（repos 無し・trial workspace は git でない → worktree を作らず path そのもの）になる。
2. 投入後の `lookup`/`check_existing` で `GET /tasks/{id}` の `workspace.path` が trial workspace と一致し、`repos` が空であることを
   確かめる。違えば試行を `harness_error`（executor_errors）にし、採点しない（fail-closed）。
3. `runner.rs` の no_artifact 判定（いま `cid == ConfigId::A` だけ）を B/C にも広げる: 終端が passed で trial workspace が
   seed から変わっていなければ `no_artifact`。成果物が別の場所に出た試行を `model_failure` と誤判定しない。
4. 出力の契約を worker に届ける: snapshot が公開 case 定義（`cases/final/<case>/case.json` のうち公開欄。private_ref は含めない）を
   trial workspace の `materials/<case>/case.json` に置き、objective の参照先をその相対 path にする（A・B 共通）。今回は A も
   B も `report.md`・`results/cells.csv`・`inputs/` への展開を知らなかった。
5. `docs/ops/measure-run-final.md` の task-template 手順（`repos:["scratch"]`・scratch repo の登録）を消す。scratch repo は試行間で
   共有され成果物が混ざるので、旧い仕組みとして残さない。

**根拠**: 写す案（task 終了後に `scratch-repo/docs/*.md` を trial workspace へ）は、(a) scratch repo が全 B 試行で共有の dir なので
どれがどの試行の成果物か決められない、(b) worker は trial workspace の入力（materials・inputs・kb）を見ずに解くことになり A と条件が
揃わない、(c) 写す対象の path 規則を決める必要がある。trial workspace を cwd にすれば A と同じ場所で同じ入力を見て、grader は
既存のまま読める。Celeris が task dir に書く `artifacts/`・`runs/` が trial workspace に混ざる点は、grader が case の宣言 path しか
読まないので採点に影響しない（workspace_sha256_final には入る。記録としてはむしろ正しい）。

**試験**:
- `crates/campaign` `b_done_artifact_lands_in_trial_workspace_and_is_graded`: 偽 Celeris transport が task を done にし、trial
  workspace に case の `report.md`・`results/cells.csv`・展開済み inputs を書く stub。grade は `model_failure` でない（合格の
  fixture 案件で pass）。
- `crates/campaign` `b_passed_without_workspace_change_is_no_artifact`。
- `crates/campaign` `celeris_body_pins_trial_workspace_and_empty_repos`・`check_template_rejects_repos`・
  `lookup_rejects_task_with_foreign_workspace`。
- `crates/task-api`（または task-dispatch）`task_with_empty_repos_runs_in_local_workspace_path`。

## D3. decision-log `--verify-connected` の欠け: (a) は原因を直し、(b) は検査の規則を直す

**(a) `grader_verdict`**: ADR 2026-10-15 は `campaign_verdict` を「独立 checker が問題単位で付ける」と決めたが、writer が無い。
**決定**: 測定 daemon の DB には書き戻さず、`celerisctl campaign export` に `--grades <grades.jsonl>` を足して export 時に結合する。
結合の鍵は trial identity（`manifest_hash`=`campaign_sha256`・`case_id`・`arm`・`repetition`）。grade 行の `failure_class` と
合否から `pass`/`fail` を決め、その試行の**全 run**（worker・continuation・reviewer）の `grader_verdict` に同じ値を入れる
（grader の判定は試行単位で、run 単位の判定は存在しない）。`--verify-connected` は `--grades` を必須にする。grade 行の無い試行は
欠測のまま（0 や既定値で埋めない — 既存の原則）。manifest hash が食い違う grade 行は誤りとして exit 1。
- 根拠: grade は測定が終わってから private grader が出す。測定中の daemon の DB に書くと被測定系に採点結果が入る（独立性を崩す）。
  export は DB の写し（`dl-src`）を読むので、写しと grades の結合で足りる。
- file: `crates/task-core/src/decision_log_export.rs`（結合と verify）・`crates/celerisctl/src/commands/campaign.rs`
  （`--grades`）・`docs/ops/measure-run-final.md` §export の手順・launcher の export 段。
- 試験: `crates/task-core` `export_joins_grader_verdict_to_every_run_of_trial`・`verify_connected_requires_grades`・
  `grade_with_foreign_manifest_hash_is_rejected`。

**(b) reviewer run の `proposed_first`**: reviewer run には `routing_decided` が無く、したがって `decision_id` も推奨候補（`proposed_first`）も
存在しない。**決定**: 記録の投影は変えず、検査の規則を直す。`task-core` の `verify_connected` は `decision_id` を持たない run
（reviewer run 等、routing 判断を経ていない run）を `proposed_first` の必須検査から除外する。`decision_id` を持つ run は従来どおり
`proposed_first` を必須とし、`grader_verdict`・`exec_model` など他の必須欄は全 run に求め続ける。
- 根拠: 「実行 model は提案の先頭か」は routing 判断がある run にだけ意味を持つ。判断が無い run に `proposed_first` を捏造する
  （`llm_route_selected` の候補から射影する等）と、存在しなかった提案を測定に混ぜる。欠けは仕様上の欠けであって記録の漏れではない。
- file: `crates/task-core/src/decision_log_export.rs`（`verify_connected` の `unrouted` 判定）。
- 試験: `crates/celerisctl/tests/campaign_connected.rs` の `run_without_routing_decision_may_lack_proposed_first`。

## D4. runbook と lock 判定: 出力先を OUT の外にする（lock 判定は変えない）

**決定**: runbook の運用者が捕る stdout（`watch` の `final-run-N.json`・`usage-N.json`・`exp-db` 等）は `$OUT` の外、
`RUNS=$(dirname "$OUT")/runs`（pilot が実際に使った配置）に置く。lock の「OUT にデータがあって lock.json が無ければ拒否」は
変えない。拒否のメッセージには OUT にある既存 entry 名（先頭数件）を出して、原因が分かるようにする。
- 根拠: OUT は lock tree が所有する場所で、知らない file（空でも）を数えない例外を作ると「他人のデータを新しい lock に取り込まない」
  不変条件に穴が開く（file 名の許可リストは運用ごとに増える）。redirect 先を変えるのは runbook の 1 行で済む。
- file: `docs/ops/measure-run-final.md`（§5 の watch・usage・export 例。`$OUT/final-run-N.json` → `$RUNS/watch-N.json`）、
  同じ形の他の runbook（`docs/ops/measure-run-*.md` を grep）、`crates/campaign/src/lock.rs`（メッセージ）。
- 試験: `crates/campaign` `runbook_does_not_redirect_into_out`（`docs/ops/measure-run-*.md` に `> $OUT/`・`>"$OUT/`・
  `> "$OUT/` の redirect が無い）と `lock_refusal_names_existing_entries`（空の `final-run-1.json` だけがある OUT で拒否され、
  メッセージにその名が出る）。

## D5. 参考: failed / harness_error の run でも取れた usage を記録する

**決定**: adapter が終端で usage を持っていれば（claude-code の `last_result.usage`、無ければ D1 の累積値）、`Error` の終端と
`WorkerMessage::Error` にも `usage: Option<Usage>`（追加のみ）を載せ、`worker_finished.usage` と runs 索引に入れる。reviewer run
（`review.rs` の失敗経路）も同じ。値が無ければ欠測のまま（0 で埋めない）。
- 根拠: 失敗した run の消費が欠けると campaign の `tokens_missing` と台帳の照合が崩れ、超過の観測（D1）が過小になる。
- file: `crates/task-worker/src/adapter.rs`（Terminal::Error）・`crates/task-worker/src/protocol.rs`・`claude_code.rs`・
  `crates/task-dispatch/src/dispatcher/worker_finish.rs`・`crates/task-dispatch/src/review.rs`。
- 試験: `crates/task-worker` `claude_code_error_carries_observed_usage`・`crates/task-dispatch` `failed_reviewer_run_records_usage`。

## 本番・測定 config への影響（後続の close unit が進捗の「本番への移行」に書く）

- 次の campaign の task-template から `repos` を外す（D2。`check_template` が拒否する）。scratch repo の登録手順は不要。
- `per_case.max_tokens`（人が 1,500,000 まで許可）は manifest の値がそのまま task の `max_tokens` になる（D1）。
- export は `--grades $OUT/grades.jsonl` を付ける（D3）。watch 等の出力は `$RUNS` へ（D4）。
- 測定 daemon は D1・D2・D5 を含む release に替える（release sha12 は env-lock に入る）。
