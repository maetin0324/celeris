# task-features: 固定 schema の決定的な抽出・記録・学習データ export

- task: 01M4M8D4AQHBC2KG3H1BPJ7Z0J / unit: task-features（verify leaf）
- status: done (2026-10-11)
- 成果: `task-core::route_record`（型・brief・aux・選択肢 builder）＋ `task-core::route_extract`（DB 読み出しと outcome label）＋ `task-core` migration 0069（5 表）＋ `task-ops::routing_export`（`celeris.route-training/1` JSONL）＋ `celerisctl routing export`（旧 dataset v1・`routing evaluate` を消して置き換え）
- unit ごとに [task-features/](task-features/)（schema / store / migrate-fix / extract / dispatch / export-core / export-cli）
- ADR: [2026-10-11-task-routing-features.md](../../adr/2026-10-11-task-routing-features.md)（実装の付記を本日追加）

## 実装した schema と template の版

固定の版定数（`crates/task-core/src/route_record.rs`、`route_record_schema_pinned` 試験で固定）:

| 定数 | 値 |
|---|---|
| `TASK_FEATURES_SCHEMA` | `celeris.task-features/1` |
| `ROUTE_OPTIONS_SCHEMA` | `celeris.route-options/1` |
| `ROUTE_DECISION_SCHEMA` | `celeris.route-decision/1` |
| `ROUTE_OUTCOME_SCHEMA` | `celeris.route-outcome/1` |
| `TASK_BRIEF_TEMPLATE` | `task-brief/1` |
| `CANDIDATES_ONE_SHOT_BUILDER` | `candidates-one-shot/1` |
| export の行 | `celeris.route-training/1`（`task-ops::routing_export`） |

`route_record_schema_pinned` は schema / template / builder の文字列と、固定入力の brief・aux・hash（`sha256:93bb770a…`）を byte 固定する。版の文字列を変えると落ちる。

## features の節と aux の欄

brief は `build_task_features`（`crates/task-core/src/route_record.rs`）が LLM を使わず決定的に組み立てる。節の順・見出しは `task-brief/1` が固定。自由文は `redact_secrets`（secret 置換）→ `bounded_secret`（上限 4,000 字。超過は先頭・末尾と `…（N 文字省略、sha256=…）`、`aux.truncated` に残す）で入れる。

### brief の節（D2）

| 節 | 定義 | 抽出元 | 欠測の扱い |
|---|---|---|---|
| 要求 | task title + 判断対象の目的文（WU なら WU objective、葉 task なら task objective） | `tasks.title`、`work_units.json.objective`／`tasks.objective` | 空は「（目的の記述なし）」＋ `aux.missing` に `objective` |
| 期待する成果物 | 受け入れ条件・checks（`command` 種は実行ファイル名＋ subcommand まで）・`done_when`・`outputs` | `tasks.json.acceptance`、`work_units.json.{checks,done_when,outputs}` | 無ければ「決定的な検査なし」 |
| 触る範囲 | 予定 path・repo・crate・書き込み集合 | `work_units.json.context.paths`、`tasks.repos_json`、`run_write_sets` | 無ければ「範囲の記述なし」 |
| 置かれた位置 | genre・kind・harness・lane・depth・phase_index・deps_n | `tasks.genre`、`work_units.kind/phase/depends_on_json`、`tasks.parent_id` の連鎖 | 不明は「不明」 |
| 過去の試行 | 前の run 数・失敗数・初回か | `runs`（同じ `work_unit_id`） | 初回は「初回」 |
| 性質（既存の判定） | ADR-0069 の 9 軸 `TaskFeatures` を「判断の重さ: …」で文に | `routing_decided.record.decision.features` | 無ければ「不明」 |

### aux の欄（D3(a)、入力側）

`FeaturesAux`（`route_record.rs`）。値が無い欄は `null` のまま `aux.missing` に名前を入れる（0 や既定値で埋めない）。

| 欄 | 型 | 抽出元 |
|---|---|---|
| `genre` / `kind` / `harness` / `lane` | カテゴリ | `tasks.genre`、`work_units.kind`、判断時の harness 選択、`routing_decided.record.decision.lane` |
| `depth` / `phase_index` / `deps_n` | 整数 | `tasks.parent_id` の連鎖、`work_units.phase`＋`execution_plans`、`work_units.depends_on_json`／`tasks.json.depends_on` |
| `acceptance_n` / `check_command_n` / `check_artifact_n` / `check_reviewer_n` / `check_human_n` | 整数 | `tasks.json.acceptance`、`work_units.json.checks` |
| `objective_chars` / `objective_truncated` | 整数・真偽 | 目的文の長さ・切ったか |
| `context_paths_n` / `crates_n` / `repos_n` | 整数 | `work_units.json.context.paths`（`crates/<name>/` を数える）、`tasks.repos_json` |
| `prior_runs_n` / `prior_failed_n` / `retries_n` / `repairs_n` | 整数 | `runs`、`work_units.retries`、同じ task の `kind='repair'` WU |
| `budget_turns` / `budget_wall_s` | 整数 | `work_units.json.budget`／`tasks.json.budget` |
| `task_features_v69` | カテゴリ 9 個 | `routing_decided.record.decision.features` |
| `missing` / `truncated` | 文字列配列 / 切断記録 | 取れなかった欄・切った欄 |

`extract`（`route_extract.rs`）は DB の列と events から `FeaturesInput` を読み、上記の `build_task_features` を呼ぶ。`AsOf::Timestamp` は run の `started_at` と event の `ts` を境界にし、`AsOf::EventSeq` は event の `seq` を境界にする（判断時点より後の情報を入れない）。prompt・comment・log・tool 出力・credential は brief に入れない。

### label（D3(b)、`celeris.route-outcome/1`、features には入れない）

`outcome_labels_for_run`（`route_extract.rs`）が run ごとに作る:

| 欄 | 抽出元 | 備考 |
|---|---|---|
| `run_status` | `runs.status` | |
| `checks_passed` / `checks_total` | `review_verdict`（`criterion_idx`・`pass`） | 判定なしなら `null` |
| `review_pass` | `review_verdict` | reviewer・人 |
| `wall_s` | `runs.metrics_json.wall_ms` / 1000 | 無ければ `missing` |
| `cost_usd_list` / `tokens_in` / `tokens_out` | `runs.usage_json` | 無ければ `missing` |
| `quota_used` / `changed_lines` / `changed_files_n` / `changed_crates_n` / `retries_after` / `repaired` | — | 計測・git 未実装のため `null` ＋ `missing`（下「未実装」参照） |

## 記録の時点

- `routing_decided` event の `record.features`（`RouteFeaturesRef { schema, template, hash }`）: dispatch が worker run の source・model を決める 1 回ごとに、`task-core::route_extract::features_for_subject` で snapshot を作り、`insert_route_features`（hash で内容アドレス化・追記専用）に書き、その参照だけ event に入れる（brief 本文は event に入れない）。旧 event は `features` が無くても復元可能（`Option`）。
- `route_features` 表: 本文（`brief`）と `aux_json` を持つ。同じ hash は 1 行（`INSERT OR IGNORE`）。
- 抽出・保存に失敗しても dispatch は続行（警告ログ、参照は無し）。`route_dispatch_records_feature_reference_and_snapshot` が event の hash と `route_features` 行の一致を検証。

## 公開 API の使い方

- `task_core::route_extract::features_for_subject(store, Subject, as_of) -> TaskFeatures`
  - WU / 葉 task の `celeris.task-features/1` snapshot を読む。`Subject { kind: "work_unit"|"task", id }`。
  - `as_of: None`（現在の DB）か `Some(AsOf::Timestamp(rfc3339))`（判断時点の timestamp）・`Some(AsOf::EventSeq(n))`（判断時点の event seq）。評価 manifest は元の判断時点の `AsOf` を渡して、将来の情報を混ぜずに作る。
- `task_core::route_extract::outcome_labels_for_run(store, run_id) -> RouteOutcome`
  - 1 run の `celeris.route-outcome/1` label。git 操作をしない。計測不能の欄は `null` ＋ `missing`。
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
    "template": "task-brief/1"
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
    "labels": { "checks_passed": 3, "checks_total": 3, "cost_usd_list": 0.38, "missing": ["quota_used","changed_lines","changed_files_n","changed_crates_n","retries_after","repaired"], "review_pass": true, "run_status": "completed", "tokens_in": 12000, "tokens_out": 3400, "wall_s": 610.5 },
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

| コマンド | 結果 |
|---|---|
| `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` | ok（nextest 5138 passed / 0 failed / 14 ignored、168 binaries＋doctest 10、nextest_exit 0 / doctest_exit 0） |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo test -p task-core --lib route_` | 22 passed / 0 failed（route_record 6・route_extract 2・store::route_records 3・direct_route 11。`route_record_schema_pinned` で版と byte 固定） |
| `cargo test -p task-ops --lib routing_export_` | 3 passed / 0 failed（byte 安定・decision_id 順・outcome 結合・時刻/ stage フィルタ・brief 省略・feature 欠損の fail-soft） |
| `cargo test -p celerisctl --test routing_cli` | 1 passed / 0 failed（JSONL の行・主要欄・`--text none` の brief 省略） |
| `cargo test -p task-dispatch --lib route_dispatch` | 1 passed / 0 failed（`route_dispatch_records_feature_reference_and_snapshot`：event の hash と `route_features` 行の一致） |
| `sh scripts/dev/check-doc-links.sh` | ok（exit 0） |
| `sh scripts/dev/check-adr-numbers.sh` | ok (183 files)（exit 0） |
| `python3 scripts/dev/check-architecture-map.py` | exit 0 |
| `git diff --quiet "$CELERIS_WU_BASE" -- crates/` | clean（crates/ の差分はゼロ） |

## selector へ渡す未実装

- `route_decisions`・`route_outcomes` の書き手: dispatch は現状 features snapshot の記録（`routing_decided.features`）まで。判断 1 回ごとの `route_decisions` 行、run 終わりの `route_outcomes` 行を書くのは selector / rollout 段（判断 record の書き込みは `selector` leaf、ADR D5・D6）。
- `route_judgments` の API: 表と store API（`insert_route_judgment` / `route_judgment_get` / `route_judgments_for_hashes`）は作ってあるが、`POST /api/v1/routing/judgments` の HTTP API と判定器（decision model）本体・sidecar・`JudgePolicy` は未実装（ADR D9。この task では表と API の形だけ）。
- `changed_lines` / `changed_files_n` / `changed_crates_n` / `quota_used` / `retries_after` / `repaired`: label 側は現状 `null` ＋ `missing`。`run_write_sets`／`work_unit_write_sets` の base..head を `git diff --numstat` で数える git 操作と quota 計測は未実装（ADR D3(b)・未解決 3）。

## ADR の未解決 3 点の決着

- **secret 置換の規則をどの既存実装から取るか**: `task-worker` に自由文向けの汎用 token/key redactor は無いため、`task-core::route_record::redact_secrets` に Bearer、`sk-`/`sk_live_`/`ghp_`/`gho_`/`github_pat_`/`xox*` 等の token 形、`api_key=`/`token=`/`secret=`/`password=` の key=value 形を写した。自由文は record 化前にこの関数を通す（task-worker への依存を作らない）。`route_record_secret_replacement_covers_token_and_key_shapes` で固定。
- **brief の各節の字数上限の具体値**: 目的文は 4,000 字（`OBJECTIVE_MAX_CHARS`、ADR D2 の仮値のまま。超過は先頭・末尾＋省略マーカーで `task-brief/1` に残す）。title / check / 各 list 項目は 1,000 字。評価集合の文の長さ分布を見て要調整なら `task-brief/1` の版を上げる。
- **`changed_lines` を数える git 操作の置き場所**: 未実装のため未決。現状は `outcome_labels_for_run` が git 操作をせず `null` ＋ `missing` を返す。置き場所（daemon の終了処理か export 時の遅延計算か）と実装は selector / rollout 段で決める。
