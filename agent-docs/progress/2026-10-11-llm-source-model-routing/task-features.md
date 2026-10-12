# task-features: 固定 schema の決定的な抽出・記録・学習データ export

- task: 01M4M8D4AQHBC2KG3H1BPJ7Z0J / unit: task-features（verify leaf）
- status: reverify 完了（2026-10-11）。`task-brief/2`・aux の抽出元を本節で書き直した。`check-architecture-map.py` だけが失敗（下「試験の証拠」、base 由来・本 WU の範囲外）
- 成果: `task-core::route_record`（型・brief・aux・選択肢 builder）＋ `task-core::route_extract`（DB 読み出しと outcome label）＋ `task-core` migration 0069（5 表）＋ `task-ops::routing_export`（`celeris.route-training/1` JSONL）＋ `celerisctl routing export`（旧 dataset v1・`routing evaluate` を消して置き換え）
- unit ごとに [task-features/](task-features/)（schema / store / migrate-fix / extract / dispatch / export-core / export-cli）
- ADR: [2026-10-11-task-routing-features.md](../../adr/2026-10-11-task-routing-features.md)（実装の付記を本日追加）

## 再試行 field audit（2026-10-11）

- [D2・D3(a)・D3(b) 全欄 × subject 照合表](task-features/field-audit.md)。各セルは実装済みの抽出元・コード位置・`route_extract_` fixture 試験、または ADR 文言を根拠にした該当なし。
- 差分修正: task subject の `retries_n` は判断時点内 run 数 − 初回 run（下限 0）、`repairs_n` は所属 task の repair WU 数。task/WU fixture は `route_extract_task_retry_repair_and_budget_fields_use_task_fixture` と `route_extract_work_unit_retry_repair_and_budget_fields_use_work_unit_fixture`。
- D2 の project 名が brief に未出力だったため `tasks.project_id` → `projects.title` を実装し、template を `task-brief/3` に更新。`route_record_schema_pinned` の hash は `sha256:da5704c36693709b084a3652f5d04b5ee248fecf8374770029ae3f4c253a6c0b`。
- 対象検査: `cargo test -p task-core route_extract_ --lib` 成功（12 tests）。この追試の全体 reverify は `reverify3` が担当。
- 前回失敗 check の原因は `route_dispatch_records_feature_reference_and_snapshot` が旧 template `task-brief/2` を期待していたこと。案件名を含める `/3` への移行に合わせ固定期待値を更新した。指定 check `cargo test -p task-core route_ && cargo test -p task-ops routing && cargo test -p task-dispatch cheap_local_first && cargo test -p celerisctl --test routing_cli` は成功（36 + 12 + 9 + 1 tests）。`git diff --check` も成功。

## 再試行 reverify4（2026-10-12、task 01M4P7CTS4）

- 前回 branch 引継ぎ: `celeris/01M4NM332S384YYC4VC5DB7KCY`（816a6a5b）は HEAD の祖先に入っている（merge-prev 済み。抽出・brief task-brief/2・aux・route 表・routing_decided の hash・export・試験 5138 件 pass の成果を作り直していない）。
- 差し戻し（前回 reviewer の唯一の不備）: `features_for_subject` が `write_set` を設定せず、抽出 SQL に `run_write_sets` の読取が無かったため brief が常に「範囲の記述なし」だった。
- 直し（writeset、7cd9444c）: task subject は時点内の `run_write_sets`（task_id 一致）、WU subject は `run_write_sets`（work_unit_id 一致）と `work_unit_write_sets` を、判断時点（`AsOf`）より前の行だけ読む。path は `<repo>:<path>` に正規化・重複除去・整列して brief の「触る範囲」節と aux の `write_set` に入れる。不完全な行（`incomplete`／`unavailable`）は `write_set_gaps` に残す。
- template: `task-brief/3`（項目「触る範囲」の中身が変わるため）。`route_record_schema_pinned` の期待値と ADR 付記の `sha256:da5704c3…` は同じ変更で更新済みで、HEAD の試験で一致している。
- 突き合わせ表: ADR の抽出元表（D2・D3(a)・D3(b)）の全欄 × subject 種別（task／WU）を [task-features/field-audit.md](task-features/field-audit.md) に書いた（field-audit、30402847）。各セルは実装済みの抽出元と fixture 試験名、または ADR 文言による該当なし。未実装のセルは残っていない（`git grep 未実装` で確認）。
- 実データ形の fixture（前の試行あり／なし、判断時点より後の行は読まない）: `route_extract_write_set_{work_unit,task}_prior_attempt_in_brief_and_aux`、`route_extract_write_set_{work_unit,task}_without_prior_attempt_is_missing`、`route_extract_write_set_excludes_sets_recorded_after_decision_time`。fixture の path は `notes/wu.md`（文書参照にしない。doclink-fix）。
- reverify3 の失敗 2 件の診断:
  - `check-doc-links.sh` の exit 1（5 件）: `crates/task-core/src/route_extract.rs` の write set fixture の `docs/wu.md` が、`crates/*.rs` の生きた文書参照として検査された。doclink-fix（144529b3）で `notes/wu.md` に替えて解消。
  - clippy の timeout: host の load average が 70 台だった時の時間切れ。同じ HEAD で前の 2 回は 1.7 秒・10.9 秒で終わっていた。reverify4 の clippy は exit 0（警告 0、1 分 10 秒）で完了し、コードの失敗は見つからなかった。
- 試験数（reverify4、HEAD 144529b3）: `test-parallel.sh` は nextest 5152 passed / 0 failed / 14 ignored、doctest exit 0。`cargo clippy --workspace -- -D warnings` exit 0。`check-doc-links`・`check-adr-numbers`・`progress-index --check` は exit 0。`cargo test -p task-core --lib route_` は 36 passed。詳細は [task-features/reverify4.md](task-features/reverify4.md)。
- 試験の環境: 作業 tmp（94 字）のまま回すと unix socket の path が `SUN_LEN` を超えて credentiald・task-api・celerisctl の socket 試験 158 箇所が落ちる（コードの失敗ではない）。`TMPDIR=/tmp/rv4` で全件通った。
- この検証時点（reverify4）では `changed_*` が残っていた。labels-diff WU で task-ops の遅延計算と task-core の数値受け口を実装し、6欄すべてが抽出済み。詳細は [labels-diff](task-features/labels-diff.md)。

## 実装した schema と template の版

固定の版定数（`crates/task-core/src/route_record.rs`、`route_record_schema_pinned` 試験で固定）:

| 定数 | 値 |
|---|---|
| `TASK_FEATURES_SCHEMA` | `celeris.task-features/1` |
| `ROUTE_OPTIONS_SCHEMA` | `celeris.route-options/1` |
| `ROUTE_DECISION_SCHEMA` | `celeris.route-decision/1` |
| `ROUTE_OUTCOME_SCHEMA` | `celeris.route-outcome/1` |
| `TASK_BRIEF_TEMPLATE` | `task-brief/3` |
| `CANDIDATES_ONE_SHOT_BUILDER` | `candidates-one-shot/1` |
| export の行 | `celeris.route-training/1`（`task-ops::routing_export`） |

`route_record_schema_pinned` は schema / template / builder の文字列と、固定入力の brief・aux・hash を byte 固定する。再試行 field audit 後の hash は `sha256:da5704c36693709b084a3652f5d04b5ee248fecf8374770029ae3f4c253a6c0b`。

### `task-brief/1` から `task-brief/2` へ上げた理由

final review（reviewer 01M4NZVDGH3EQ2RV476X3PJENR）の差し戻し 6 点を直した。brief の文面（節の書式・過去の試行・性質の出し方）が変わるので、ADR の規則で template 版を上げ、固定試験の期待値と ADR 付記の hash を同じ変更で更新した。

| 差し戻し | 直し方 | 版との関係 |
|---|---|---|
| (1) `acceptance` の object を文字列 list として読んで空になった | `Criterion` を `AcceptanceInput`（文と `CheckInput`）として読む。試験の fixture `[{"text":"works"}]` 形も残る | brief の期待する成果物の書式 → `/2` |
| (2) `acceptance_n`・`check_*_n` を誰も設定しない | `Criterion.check` と `WorkUnitCheck` から数えて設定（下の aux 表） | aux の欄が埋まる |
| (3) check の種類が常に `unknown` | `CheckInput` が `command`／`artifact_exists`／`knowledge_page`／`reviewer`／`human` を持つ。WU の check は `command` | brief の検査の書式 → `/2` |
| (4) `retries_n` を wu json から読んで常に null | `work_units.retries` 列を読む | aux |
| (5) lane・harness・budget・repairs が渡らず欠測 | dispatch が `RouteFeatureContext` で lane・harness・9 軸を渡す。budget は WU → task の順。repairs は同 task の `kind='repair'` WU 数 | aux・brief の置かれた位置 |
| (6) 9 軸が前回の判断、events の `ORDER BY seq` 無し | dispatch の今回の判断を優先（`task_features_v69` は context が勝つ）。events は `ORDER BY seq`。過去の試行は run ごとの行（status・model・壁時計・落ちた criterion と exit）、性質は 9 軸を固定の名前と段（低／中／高）で文にする | brief の過去の試行・性質の書式 → `/2` |

また `redact_secrets` は byte 列の 1 回走査に直し（O(n²) を解消）、20 万字の入力の試験 `route_record_redact_large_input_replaces_every_secret` で固定した。

## features の節と aux の欄

brief は `build_task_features`（`crates/task-core/src/route_record.rs`）が LLM を使わず決定的に組み立てる。節の順・見出しは `task-brief/3` が固定。自由文は `redact_secrets`（secret 置換）→ `bounded_secret`（上限 4,000 字。超過は先頭・末尾と `…（N 文字省略、sha256=…）`、`aux.truncated` に残す）で入れる。

### brief の節（D2）

| 節 | 定義 | 抽出元 | 欠測の扱い |
|---|---|---|---|
| 要求 | task title + 判断対象の目的文（WU なら WU objective、葉 task なら task objective） | `tasks.title`、`work_units.json.objective`／`tasks.objective` | 空は「（目的の記述なし）」＋ `aux.missing` に `objective` |
| 期待する成果物 | 受け入れ条件（「文（検査: 種類）」の 1 行ずつ）・checks（`command` は実行ファイル名＋ subcommand まで）・`done_when`・`outputs` | `tasks.json.acceptance`（`Criterion`）、`work_units.json.{checks,done_when,outputs}` | 無ければ「決定的な検査なし」 |
| 触る範囲 | 予定 path・repo・crate・書き込み集合 | `work_units.json.context.paths`、`tasks.repos_json`、`run_write_sets` | 無ければ「範囲の記述なし」 |
| 置かれた位置 | project・genre・kind・harness・lane・depth・phase_index・deps_n | `tasks.project_id` → `projects.title`、`tasks.genre`、`work_units.kind/phase/depends_on_json`、`tasks.parent_id` の連鎖 | 不明は「不明」 |
| 過去の試行 | 前の run 数・失敗数（数の行）、run ごとの 1 行（古い順: status・model・壁時計・落ちた criterion の文と exit）、repair 数の行 | `runs`（WU なら `work_unit_id`、葉 task なら `task_id`。判断時点で絞る）、`events` の `review_verdict` | 前の run が無ければ「初回」。取れない欄は「不明」 |
| 性質（既存の判定） | ADR-0069 の 9 軸を固定の名前と段（低／中／高）で 1 軸 1 行。軸の順は固定 | 今回の判断（dispatch の `RouteFeatureContext.task_features_v69`）。無いときだけ過去の `routing_decided` event | 無ければ節ごと省き aux に `missing` |

### aux の欄（D3(a)、入力側）

`FeaturesAux`（`route_record.rs`）。値が無い欄は `null` のまま `aux.missing` に名前を入れる（0 や既定値で埋めない）。

| 欄 | 型 | 抽出元 |
|---|---|---|
| `genre` / `kind` / `harness` / `lane` | カテゴリ | `tasks.genre`、`work_units.kind`（葉 task は `task`）。`harness`・`lane` は dispatch の `RouteFeatureContext`（`lane` = `task.worker_hint.tier`、`harness` = 選ばれた adapter）。context が無いときだけ過去の `routing_decided`（`record.decision.lane`・`record.harness`） |
| `depth` / `phase_index` / `deps_n` | 整数 | `tasks.parent_id` の連鎖、`work_units.phase`＋`execution_plans`、`work_units.depends_on_json`（WU）／`tasks.json.depends_on`（task） |
| `acceptance_n` | 整数 | `tasks.json.acceptance` の件数（`Criterion`） |
| `check_command_n` / `check_artifact_n` / `check_reviewer_n` / `check_human_n` | 整数 | `Criterion.check` の種類（`command`／`artifact_exists`・`knowledge_page`／`reviewer`／`human`）と `work_units.json.checks`（`WorkUnitCheck`、すべて command） |
| `objective_chars` / `objective_truncated` | 整数・真偽 | 目的文の長さ・切ったか |
| `context_paths_n` / `crates_n` / `repos_n` | 整数 | `work_units.json.context.paths`（`crates/<name>/` を数える）、`tasks.repos_json` |
| `prior_runs_n` / `prior_failed_n` | 整数 | `runs`（判断時点で絞る。失敗 = `failed` と `budget_exhausted`） |
| `retries_n` | 整数 | WU は `work_units.retries` 列、task は判断時点内 run 数 − 初回 run（下限 0） |
| `repairs_n` | 整数 | task/WU 共に所属 task の `kind='repair'` の WU 数 |
| `budget_turns` / `budget_wall_s` | 整数 | WU の `json.budget.{max_turns,max_wall_secs}`、無ければ task の `json.budget` |
| `task_features_v69` | カテゴリ 9 個 | 今回の判断の `RouteFeatureContext.task_features_v69`（dispatch の `decision.features`）。無いときだけ過去の `routing_decided` |
| `missing` / `truncated` | 文字列配列 / 切断記録 | 取れなかった欄・切った欄 |

`extract`（`route_extract.rs` の `features_for_subject(store, subject, as_of, context)`）は DB の列と events から `FeaturesInput` を読み、上記の `build_task_features` を呼ぶ。events は `ORDER BY seq` で読み、`routing_decided` は最後のものを使う。`AsOf::Timestamp` は run の `started_at` と event の `ts` を境界にし、`AsOf::EventSeq` は event の `seq` を境界にする（判断時点より後の情報を入れない）。prompt・comment・log・tool 出力・credential は brief に入れない。

### label（D3(b)、`celeris.route-outcome/1`、features には入れない）

`outcome_labels_for_run`（`route_extract.rs`）が run ごとに作る:

| 欄 | 抽出元 | 備考 |
|---|---|---|
| `run_status` | `runs.status` | |
| `checks_passed` / `checks_total` | `review_verdict`（`criterion_idx`・`pass`） | 判定なしなら `null` |
| `review_pass` | `review_verdict` | reviewer・人 |
| `wall_s` | `runs.metrics_json.wall_ms` / 1000 | 無ければ `missing` |
| `cost_usd_list` / `tokens_in` / `tokens_out` | `runs.usage_json` | 無ければ `missing` |
| `quota_used` | `quota_estimated` event の最後の `windows[].used_pct`（run_id 一致） | event または値のある窓が無ければ null＋missing |
| `retries_after` | 同じ subject の後続 worker run 数（task: task_id と work_unit_id NULL、WU: work_unit_id） | started_at が無ければ null＋missing |
| `repaired` | run 後に同じ task に作られた repair WU の有無 | started_at が無ければ null＋missing |
| `changed_lines` / `changed_files_n` / `changed_crates_n` | run の `run_write_sets`（WU は行が無い場合 `work_unit_write_sets`）の complete な base..head を task-ops が `git diff --numstat` | binary は 0 行、path は repo:path で重複除去、crate は `crates/<name>/` の異なり数。取得不能なら null＋missing |

## 記録の時点

- `routing_decided` event の `record.features`（`RouteFeaturesRef { schema, template, hash }`）: dispatch が worker run の source・model を決める 1 回ごとに、`task-core::route_extract::features_for_subject` で snapshot を作り、`insert_route_features`（hash で内容アドレス化・追記専用）に書き、その参照だけ event に入れる（brief 本文は event に入れない）。旧 event は `features` が無くても復元可能（`Option`）。
- `route_features` 表: 本文（`brief`）と `aux_json` を持つ。同じ hash は 1 行（`INSERT OR IGNORE`）。
- 抽出・保存に失敗しても dispatch は続行（警告ログ、参照は無し）。`route_dispatch_records_feature_reference_and_snapshot` が event の hash と `route_features` 行の一致を検証。

## 公開 API の使い方

- `task_core::route_extract::features_for_subject(store, Subject, as_of, context) -> TaskFeatures`
  - WU / 葉 task の `celeris.task-features/1` snapshot を読む。`Subject { kind: "work_unit"|"task", id }`。`context: Option<RouteFeatureContext>` は dispatch が渡す今回の判断の lane・harness・9 軸（評価 manifest など判断でない呼び出しは `None`）。
  - `as_of: None`（現在の DB）か `Some(AsOf::Timestamp(rfc3339))`（判断時点の timestamp）・`Some(AsOf::EventSeq(n))`（判断時点の event seq）。評価 manifest は元の判断時点の `AsOf` を渡して、将来の情報を混ぜずに作る。
- `task_ops::routing_labels::outcome_labels_for_run(store, run_id, work_unit_id) -> RouteOutcome`（task-core の抽出器は `ChangeCount` を受け取る）
  - 1 run の `celeris.route-outcome/1` label。git 操作をしない。DB 由来欄は runs／events／work_units、値が得られない欄だけ `null` ＋ `missing`。
- `task_core::route_record::build_task_features(&FeaturesInput) -> TaskFeatures`
  - 純関数。`FeaturesInput` → 固定 brief ＋ aux ＋ 内容 hash。DB を使わず、評価集合の問題や任意の snapshot に使える。
- `task_core::route_record::candidates_one_shot(question, &[Candidate]) -> RouteOptions`
  - builder `candidates-one-shot/1`。適格候補ごとに 1 つの自然言語選択肢（`option_id` = 文の sha256 先頭 16 hex、`binds` = `{source_id, model_id, harness_id}`）を作る。

## `celerisctl routing export` の使い方と出力例

```
celerisctl routing export --db <path> --out <file.jsonl> [--since <RFC3339>] [--until <RFC3339>] [--stage shadow|enforce|eval] [--text brief|none]
```

- `--db` は明示の SQLite path を読み取り専用（`mode=ro`・`SQLITE_OPEN_URI`）で開く。デフォルトの daemon DB は開かない。
- 1 行 = 判断 1 回（`celeris.route-training/1`）。行の順は `decision_id` 順で決定的。同じ DB と引数で byte 一致。
- `--text none` は `features.brief` を落とし、hash と aux だけを出す（外へ渡すとき用）。既定は `brief`。
- 評価集合の行は `input.context.stage="eval"` で区別。同じ問題の本番の判断と評価の判断は `subject` と features の hash で結合できる。

fixture（`route_features`／`route_options`／`route_decisions`／`route_outcomes` に各 1 行）への出力例（`--text brief` 既定。`python3 -m json.tool` で整形）:

```json
{
  "schema": "celeris.route-training/1",
  "decision_id": "01MZEXAMPLE000000000000000001",
  "subject": { "id": "wu-1", "kind": "work_unit" },
  "features": {
    "aux": { "deps_n": 0, "depth": 2, "genre": "coding", "kind": "implement", "lane": "cheap", "missing": [], "objective_chars": 58, "truncated": [] },
    "brief": "sample brief",
    "hash": "sha256:abc123",
    "schema": "celeris.task-features/1",
    "template": "task-brief/3"
  },
  "options": {
    "builder": "candidates-one-shot/1",
    "options": [ { "binds": { "harness_id": "claude-code", "model_id": "claude-haiku-5-5", "source_id": "claude-oauth" }, "option_id": "o1", "text": "claude-code with claude-haiku-5-5" } ],
    "question": "Which source/model completes this work unit in one run?"
  },
  "input": {
    "candidates": [ { "evidence": { "n": 30, "pass": 21, "pass_lower": 0.53, "pass_upper": 0.84 }, "excluded": null, "label": "claude-haiku-5-5", "model_id": "claude-haiku-5-5", "route_default_rank": 1, "source_id": "claude-oauth" } ],
    "constraints": { "lane": "cheap" },
    "context": { "evidence_version": "evidence/2026-10-11", "stage": "shadow" }
  },
  "output": {
    "action": "execute",
    "propensity": 1,
    "rule": "evidence",
    "scores": [ { "expected_cost_usd": 0.4, "option_id": "o1", "p_success": 0.71, "rank": 1 } ],
    "selected": { "model_id": "claude-haiku-5-5", "source_id": "claude-oauth" }
  },
  "judge": { "deterministic": true, "id": "evidence-policy", "kind": "rule", "version": "routing-policy/1" },
  "outcome": {
    "labels": { "checks_passed": 3, "checks_total": 3, "cost_usd_list": 0.38, "missing": ["changed_lines","changed_files_n","changed_crates_n"], "review_pass": true, "run_status": "completed", "tokens_in": 12000, "tokens_out": 3400, "wall_s": 610.5 },
    "schema": "celeris.route-outcome/1"
  },
  "evidence_version": "evidence/2026-10-11"
}
```

`prompt`（組み立て済み worker prompt）・会話・log・credential は brief に入らないため export にも出ない。

## 消した旧い仕組み

- 旧 `celerisctl routing export`（estimator shadow 用の dataset v1）と `celerisctl routing evaluate`（estimator・旧 shadow の候補 policy 比較）を、新 JSONL export に置き換えて同じ変更で消した（[breaking-changes-by-default](../../adr/2026-10-11-breaking-changes-by-default.md)）。`routing show`（config 表示）だけ残す。
- `task-ops::routing_replay`（dataset v1・report v1・estimator 比較の実装）を削除。`crates/celerisctl` と `crates/task-ops/src` での `evaluate_with_estimator` / `DatasetRowV1` / `routing.dataset.v1` の旧参照は 0 件。
- ADR-0069 の 9 軸 `TaskFeatures` は lane の決定に使うため残す（D10）。brief の「性質」節と aux の `task_features_v69` に写す。

## 試験の証拠

reverify（2026-10-11、WU reverify。HEAD = 00ba6d56 の後の作業ツリー）の結果:

| コマンド | 結果 |
|---|---|
| `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` | exit 0。`CELERIS_TEST_SUMMARY`: nextest 5142 passed / 0 failed / 14 ignored、binaries 178（nextest 168・doctest 10）、nextest_exit 0 / doctest_exit 0、nextest 261.4 秒 |
| `cargo test -p task-core --lib route_record_schema_pinned` | 1 passed / 0 failed（`sha256:3574d17f…` の byte 固定） |
| `cargo fmt --all -- --check` | exit 0（差分なし） |
| `cargo clippy --workspace -- -D warnings` | exit 0（`Finished dev profile`、警告 0） |
| `sh scripts/dev/check-doc-links.sh` | exit 0（`check-doc-links: ok`） |
| `sh scripts/dev/check-adr-numbers.sh` | exit 0（`check-adr-numbers: ok (183 files)`） |
| `python3 scripts/dev/check-architecture-map.py` | **exit 1**。`docs/architecture-map.md` の 66・67 行の 3 件が存在しないと判定: `<workspace>/runs/<run_id>/tmp/`（実在しない記法）、`check-test-tmp-leftovers.sh`・`test-parallel.sh`（実体は `scripts/dev/` 下にあるが、表に相対名で書かれている）。この 2 行は base（6534045b）の時点で既にある。base→HEAD の `scripts/`・`docs/` の差分は 0 のため、本 WU の変更が原因ではない。表は refs-repo の WU の担当なので本 WU では直さない。修正は表の path を `scripts/dev/test-parallel.sh` 等の完全な path にし、`<workspace>/…` の行はチェッカーの対象外にする形を提案する |
| `git diff --quiet "$CELERIS_WU_BASE" -- crates/` | clean（crates/ の差分はゼロ） |

個別の試験（`route_extract_*`・`route_record_*`・`routing_export_*`・`routing_cli`・`route_dispatch_records_feature_reference_and_snapshot`）は上の全体 run に含まれ、いずれも 0 failed。`route_extract` の fixture は実データ形（`Criterion` の object、`work_units.retries`、dispatch の context）で aux の各欄が埋まることを確かめる。

reverify2（2026-10-11、WU reverify2。task subject の `deps_n` 修正後。`CELERIS_WU_BASE` = 4fa839f4、crates/ と docs/・scripts/ の差分ゼロ）の結果:

| コマンド | 結果 |
|---|---|
| `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` | exit 0。`CELERIS_TEST_SUMMARY`: nextest 5145 passed / 0 failed / 14 ignored、binaries 178（nextest 168・doctest 10）、nextest_exit 0 / doctest_exit 0、nextest 99.5 秒。`tmp_leftovers`: 4（警告のみ、exit に影響しない） |
| `cargo fmt --all -- --check` | exit 0（差分なし） |
| `cargo clippy --workspace -- -D warnings` | exit 0（`Finished dev profile`、警告 0） |
| `sh scripts/dev/check-doc-links.sh` | exit 0（`check-doc-links: ok`） |
| `sh scripts/dev/check-adr-numbers.sh` | exit 0（`check-adr-numbers: ok (183 files)`） |
| `python3 scripts/dev/check-architecture-map.py` | exit 1。`docs/architecture-map.md` の 3 件（`<workspace>/runs/<run_id>/tmp/`、`check-test-tmp-leftovers.sh`、`test-parallel.sh`）が存在しないと判定。reverify（上）と同じ既存の不一致で、本 WU の差分（docs/・scripts/ とも base から 0）とは無関係 |
| `cargo test -p task-core --lib route_extract_` | 5 passed / 0 failed（task subject の `deps_n` 試験 3 本を含む） |
| `cargo test -p task-core --lib route_record_schema_pinned` | 1 passed / 0 failed |

注記: `TMPDIR` を run の作業 tmp（長い path）にしたまま走らせると、nextest が 108 件落ちた（`path must be shorter than SUN_LEN` の unix socket bind など。browser・credentiald・celerisctl の socket 試験）。`TMPDIR=/tmp` の短い path で全件通った。これは試験環境の path 長の問題で、コードの不備ではない。

`deps_n` の task subject 修正の確認: `route_extract_task_uses_task_json_dependencies_and_populates_task_fields`（task の `tasks.json.depends_on` 2 件 → 2）、`route_extract_task_empty_dependencies_are_zero`（空配列 → 0、missing に入らない）、`route_extract_work_unit_dependencies_still_use_work_unit_column`（WU の `depends_on_json` 3 件 → 3）が通ることを上の `route_extract_` 実行で確認した。

## 残る実装項目

- D3(b) の6 label はすべて抽出済み。selector 側では run 終端の outcome 記録へ接続する。

## ADR の未解決 3 点の決着

- **secret 置換の規則をどの既存実装から取るか**: `task-worker` に自由文向けの汎用 token/key redactor は無いため、`task-core::route_record::redact_secrets` に Bearer、`sk-`/`sk_live_`/`ghp_`/`gho_`/`github_pat_`/`xox*` 等の token 形、`api_key=`/`token=`/`secret=`/`password=` の key=value 形を写した。自由文は record 化前にこの関数を通す（task-worker への依存を作らない）。`route_record_secret_replacement_covers_token_and_key_shapes` で固定。
- **brief の各節の字数上限の具体値**: 目的文は 4,000 字（`OBJECTIVE_MAX_CHARS`、ADR D2 の仮値のまま。超過は先頭・末尾＋省略マーカーで `task-brief/2` に残す）。title / check / 各 list 項目は 1,000 字。評価集合の文の長さ分布を見て要調整なら `task-brief/2` の版を上げる。
- **`changed_lines` を数える git 操作の置き場所**: 決着 — export／label 抽出時の遅延計算を task-ops に置き、task-core には `ChangeCount` の数値だけを渡す。run の complete な `run_write_sets` を読み、WU は該当 run の行が無い場合 `work_unit_write_sets` を使う。各 repo の base..head に `git diff --numstat` を実行し、追加＋削除を合計（binary の `-` は 0）、変更ファイルは repo:path で重複除去、crate は `crates/<name>/` の異なり数とする。行・SHA・repo・git のいずれかが取得不能なら3欄とも null＋missing。`routing_outcome_git_numstat_counts_lines_files_crates` と task-core の値受け渡し試験で固定する。
