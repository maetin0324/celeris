---
tasks: [01M4M8D4AQHBC2KG3H1BPJ7Z0J]
title: routing の判断を「自然言語の task 記述・自然言語の選択肢・確率」の版つき record で残し、結果と結合して export する
---
# ADR: routing の判断を「自然言語の task 記述・自然言語の選択肢・確率」の版つき record で残し、結果と結合して export する

- Date: 2026-10-11
- Status: Accepted（redesign 段。実装は build 段 `task-features`、判断 record の書き込みは `selector`）
- 決定者: 人（2026-10-11 の design 段確認の指示 3、同日の「人の補足」「人の補足 2」による訂正）
- 関係: [2026-10-11-llm-source-model-routing-evaluation.md](2026-10-11-llm-source-model-routing-evaluation.md)（主 ADR。D2 の選択規則・D5 の実装の分け方がこの ADR の record を使う）、
  [2026-10-11-breaking-changes-by-default.md](2026-10-11-breaking-changes-by-default.md)、先行 task 01M4M8D4AP の ADR `2026-10-11-llm-source-harness-selection.md`（`RoutePolicy` 境界。main 取り込み前のためリンクにしない）
- 実測: 本番 DB での埋まり具合は `agent-docs/progress/2026-10-11-llm-source-model-routing/adr-features.md`

## Context

人は将来、Jev などの decision model（task を実行せず「この入力をどの実行主体にどの方針で処理させるか」を決める判定器）を routing に使うことを考えている。
最初の指示（design 段確認の 3）は「固定 schema・数値／カテゴリの特徴量」だったが、同日に人が訂正した:

- decision model は **自然言語の入力** と **自然言語で作った任意の選択肢** を受け取り、各選択肢の確率を出す。固定の数値特徴量 schema に合わせる必要は無い。
- task の難易度・特徴は、決定的に組み立てた自然言語の記述を主にし、数値・カテゴリは補助とする。選択肢の作り方を固定の enum にしない。
- 記録するのは「判定器に渡した入力文・選択肢の文・出た確率・実際の結果」の組で、版つきで再現・export でき、評価集合の問題と本番 task の両方で同じ形で作れること。
- 判定器の呼び出しは dispatcher・store に入れない（CLAUDE.md の禁止）。別の段で判定し、結果を DB に記録して `RoutePolicy` が読む。
- 入出力の形は人が示した参考に沿わせる（人の補足 2）: Input = 要求（task）・実行コンテキスト・制約・候補情報（能力・過去の成功率・費用）、
  Output = 選択先（selected）・実行方針（action）・候補ごとの score。今の根拠表による選択器も同じ形で判断を記録する（判定器を差し替えても record の形は同じ）。

いまの本番には、routing_decided（ADR-0069 の 9 軸 `TaskFeatures` を含む。worker run の 99.3%）と、前回の optimizer 系の `routing_features_recorded`（33.6%）・
`routing_outcome_recorded`（34.5%）がある。前者は lane を決めた規則の trace、後者は撤去する estimator の入力で、どちらも「判定器に渡した入力文と選択肢」を持たない。

## Decision

### D1. record は 4 種、全部版つき

| schema id | 中身 | 1 行の単位 |
|---|---|---|
| `celeris.task-features/1` | task 記述（自然言語の **brief** が主、補助特徴 **aux** が従）。D2・D3 | 判断の対象 1 件（本番の WU／葉 task、または評価集合の問題 1 件）の、ある時点の snapshot |
| `celeris.route-options/1` | 自然言語の選択肢の集合（作り方の版 `builder` つき）。D4 | 判断 1 回の問い 1 つ |
| `celeris.route-decision/1` | 判断 1 回の Input（features・options・constraints・candidates の参照）と Output（selected・action・scores）、判定器の識別。D5 | `RoutePolicy` の判断 1 回（本番の dispatch 1 回、または評価 run の割り当て 1 回） |
| `celeris.route-outcome/1` | 判断の後に分かった結果（label）。D3(b) | 判断 1 回に対し 1 行（終わった時点） |

`/N` は形を変えたら上げる（欄の追加でも、意味が変われば上げる）。brief の組み立て規則は schema とは別に `template` 版（例 `task-brief/1`）を持ち、
文面の規則を変えたら template 版を上げる。同じ入力から同じ template 版は同じ文字列（byte 一致）を出す。これを試験で固定する。

### D2. task 記述（brief、自然言語が主）

brief は DB の列と events から **LLM を使わず決定的に** 組み立てる Markdown の文。節の順と見出しは template が固定する。
自由文の欄は長さ上限で切り（先頭と末尾を残し、間に `…（N 文字省略、sha256=…）` を入れる）、切ったことと元の長さを aux に残す
（本番の task objective は p50 1,596 字・p90 28,804 字・最大 62,005 字なので上限が要る。WU objective は p50 571 字・p90 1,406 字）。

| 節 | 定義（何を書くか） | 型 | 決定的か | 抽出元 | 欠測の扱い |
|---|---|---|---|---|---|
| 要求 | task の title と、判断の対象の目的文（WU なら WU の objective、葉 task なら task の objective）。上限 4,000 字 | 文 | はい | `tasks.title`、`work_units.json.objective`／`tasks.objective` | 空は「（目的の記述なし）」と書き aux `missing` に入れる |
| 期待する成果物 | 受け入れ条件の文と check の種類（command／artifact_exists／reviewer／human）。WU なら `checks` の command 名（実行ファイル名と subcommand まで。引数・path は落とす）と `done_when`・`outputs` | 箇条書き | はい | `tasks.json.acceptance`、`work_units.json.{checks,done_when,outputs}` | 無ければ「決定的な検査なし」と明記（無いこと自体が難しさの手がかり） |
| 触る範囲 | 予定の path（`context.paths`）、repo、crate（path から `crates/<name>/` を数える）、書き込み集合の予測 | 箇条書き | はい | `work_units.json.context.paths`、`tasks.repos_json`、`run_write_sets`（前の試行のもの） | 無ければ「範囲の記述なし」 |
| 置かれた位置 | genre・kind・harness・lane・木の深さ・親の段（phase）・依存数・案件名 | 1 行ずつ | はい | `tasks.genre`、`work_units.kind`・`phase`・`depends_on_json`、`tasks.parent_id` の連鎖、`projects` | 不明は「不明」 |
| 過去の試行 | この WU／task の前の run の数・各 run の status（completed／failed／budget_exhausted／yielded…）・壁時計・model、落ちた検査の criterion 文と exit code、repair の数 | 箇条書き | はい | `runs`（同じ `work_unit_id`）、`events` の `review_verdict`（`criterion_idx`・`pass`。`reason` の stdout 部分は入れない）、`work_units.retries` | 初回は「初回」 |
| 性質（既存の判定） | ADR-0069 の 9 軸 `TaskFeatures`（judgment・ambiguity・…）を「判断の重さ: 中」のように文にする | 文 | はい（規則で決まる） | `routing_decided.record.decision.features`、作成時の hint | 無ければ節ごと省く |

入れないもの: worker に渡す組み立て済み prompt（system・KB・memory・skill の本文）、会話・chat、run の log・tool 出力、review の stdout、
task comment（人の文が混ざり、決定的な要約にできないため）、credential・token・account の secret。
自由文の欄には、既存の secret 検出規則（token・key 形の文字列）で置換をかけてから入れる。

**context・constraints・candidates は brief と分けて構造で持つ**（人の補足 2 の Input の 4 区分）。判定器へ渡す自然言語の入力は、brief に
constraints と candidates の文（D4・D5）をつないだもので、つなぎ方も template 版が決める。

### D3. 補助特徴 aux（数値・カテゴリ）と label

aux は brief と同じ snapshot に入れる **補助** の欄。固定の列にはしない（`/1` の中で欄名と型を下の表で定義し、増やすときは schema 版を上げる）。
decision model が文を主に読む前提なので、aux は集計・層別・欠測率の監視と、文を読まない単純な基準線（ロジスティック回帰など）のためにある。

**(a) 判断の前に決定的に出せるもの（入力側）**

| 欄 | 定義 | 型 | 抽出元 |
|---|---|---|---|
| `genre` | task の genre（harness の分野） | カテゴリ（文字列） | `tasks.genre` |
| `kind` | WU の kind（implement・test・integrate・repair…）、葉 task なら `task` | カテゴリ | `work_units.kind` |
| `harness` | 先行 ADR で選ばれた harness id | カテゴリ | 判断時の harness 選択結果 |
| `lane` | frontier／standard／cheap | カテゴリ | `routing_decided.record.decision.lane` |
| `depth` | root からの深さ（root=0） | 整数 | `tasks.parent_id` の連鎖 |
| `phase_index` | 計画の何段目か（無ければ null） | 整数 | `work_units.phase` と `execution_plans` |
| `deps_n` | 依存の数（WU の depends_on、task の depends_on） | 整数 | `work_units.depends_on_json`、`tasks.json.depends_on` |
| `acceptance_n`・`check_command_n`・`check_artifact_n`・`check_reviewer_n`・`check_human_n` | 受け入れ条件・check の種類ごとの数 | 整数 | `tasks.json.acceptance`、`work_units.json.checks` |
| `objective_chars`・`objective_truncated` | 目的文の長さと切ったか | 整数・真偽 | 同上 |
| `context_paths_n`・`crates_n`・`repos_n` | 予定の path・crate・repo の数 | 整数 | `work_units.json.context.paths`、`tasks.repos_json` |
| `write_set`・`write_set_gaps` | 書き込み集合の予測（前の試行の path を `<repo>:<path>` で正規化・重複除去・整列）と、元の行のうち `incomplete`／`unavailable` のもの（`source`・`owner_id`・`repo`・`status`） | 文字列の配列・object の配列 | task は `run_write_sets`（task_id 一致）、WU は `run_write_sets`（work_unit_id 一致）と `work_unit_write_sets`。判断時点より前の行だけ |
| `prior_runs_n`・`prior_failed_n`・`retries_n`・`repairs_n` | 前の run 数・失敗数・再試行数・repair WU 数 | 整数 | `runs`、`work_units.retries`、同じ task の `kind='repair'` の WU |
| `budget_turns`・`budget_wall_s` | 予算 | 整数 | `work_units.json.budget`／`tasks.json.budget` |
| `task_features_v69` | ADR-0069 の 9 軸（low／medium／high） | カテゴリ 9 個 | `routing_decided.record.decision.features` |
| `missing` | 取れなかった欄の名前 | 文字列の配列 | — |

欠測は 0 や既定値で埋めない。値を null にし、欄名を `missing` に入れる（未知を「小さい」と同一視しないため）。

**(b) 判断の後にしか分からないもの（label 側。`celeris.route-outcome/1` に入れ、features には入れない）**

| 欄 | 定義 | 型 | 抽出元 |
|---|---|---|---|
| `run_status` | run の終わり方 | カテゴリ | `runs.status` |
| `checks_passed`・`checks_total` | 決定的 check の合否 | 整数 | `review_verdict`（`criterion_idx`・`pass`） |
| `review_pass` | 最終の review 判定（reviewer・人） | 真偽／null | `review_verdict` |
| `wall_s` | 壁時計 | 実数 | `runs.metrics_json.wall_ms` |
| `cost_usd_list`・`tokens_in`・`tokens_out` | 費用と token | 実数・整数 | `runs.usage_json` |
| `quota_used` | 窓ごとの quota 消費（測れたときだけ） | 実数 map | quota 計測（主 ADR D1） |
| `changed_lines`・`changed_files_n`・`changed_crates_n` | 実際の変更規模 | 整数 | `run_write_sets`／`work_unit_write_sets` の base..head を `git diff --numstat` で数える |
| `retries_after`・`repaired` | この判断の後に再試行・repair が要ったか | 整数・真偽 | `work_units.retries`、後続の repair WU |

label は判断の時点の features と別 record にする（後で分かったことを当時の入力に混ぜない）。

**(c) LLM で出すもの。** 難易度の推定などを LLM（decision model を含む）で出すときは、features には入れない。必ず D5 の判断 record として
「入力文（brief＋constraints＋candidates の文）の sha256・選択肢・判定器の識別（model id・版・重みの hash・温度などの設定）・出力（確率）」を残す。
同じ入力 hash と判定器識別で再実行すれば同じ出力になるか、ならない判定器は `deterministic=false` を記録する。

### D4. 選択肢（自然言語、作り方は版つきの builder）

選択肢は固定の enum にしない。`celeris.route-options/1` は `{question, builder, options:[{option_id, text, binds}]}` で、
- `question`: 判定器に問う文（例「この作業を 1 回の run で決定的な検査に通せる見込みが最も高いのはどれか」）。
- `builder`: 選択肢の作り方の識別と版（例 `candidates-one-shot/1`、`difficulty-3/1`）。作り方を変えたら版を上げる。新しい builder は code の enum ではなく、
  登録された builder 名の文字列として増やせる（record の読み手は未知の builder を落とさず素通しする）。
- `option_id`: 選択肢の文の sha256 の先頭 16 hex（文が同じなら同じ id）。
- `text`: 自然言語の選択肢（例「claude-code で claude-opus-5-5（Claude Max）を使えば 1 回で通る」「gpt-6-luna で十分」「難易度: 易」）。
- `binds`: 選択肢が実行に結び付くときの機械向けの対応（`{source_id, model_id, harness_id}` など）。難易度のように実行に結び付かない選択肢は null。
  `RoutePolicy` が確率を使って選べるのは `binds` が適格候補に一致する選択肢だけ。

この task で作る builder は `candidates-one-shot/1`（適格候補ごとに 1 つ）だけ。難易度などの他の builder は判定器を入れる将来の task で足す。

### D5. 判断 record（人の補足 2 の Input／Output の形）

```json
{
  "schema": "celeris.route-decision/1",
  "decision_id": "<ULID>", "subject": {"kind": "work_unit|task|eval_problem", "id": "…"}, "run_id": "…|null",
  "input": {
    "features": {"schema": "celeris.task-features/1", "template": "task-brief/2", "hash": "sha256:…"},
    "context": {"harness_id": "claude-code", "harness_version": "…", "genre": "coding", "stage": "shadow|enforce|eval"},
    "constraints": {"lane": "standard", "model_pin": null, "max_wall_s": 1800, "max_turns": 60,
                    "quota_remaining": {"<account>/<window>": 0.42}, "permissions": ["repo_write"]},
    "candidates": [{"source_id": "claude_max", "model_id": "claude-opus-5-5", "label": "Claude Opus 5.5（Claude Max）",
                    "evidence": {"evidence_version": "…", "n": 30, "pass": 21, "pass_lower": 0.53, "pass_upper": 0.84,
                                 "wall_median_s": 610, "usd_list_median": 1.9, "quota_per_run": {"5h": 0.012}},
                    "route_default_rank": 2, "excluded": null}],
    "options": {"schema": "celeris.route-options/1", "builder": "candidates-one-shot/1", "hash": "sha256:…"}
  },
  "output": {
    "selected": {"source_id": "…", "model_id": "…"},
    "action": "execute",
    "scores": [{"option_id": "…", "p_success": 0.71, "expected_cost_usd": 1.9, "rank": 1}],
    "rule": "evidence|route_defaults|model_pin|judge|explore",
    "propensity": 0.9
  },
  "judge": {"kind": "rule|model", "id": "evidence-policy", "version": "routing-policy/1", "config_hash": "sha256:…", "deterministic": true}
}
```

- 根拠表の選択器（主 ADR D2）も同じ record を書く。`scores` は根拠表の `pass` 点推定と区間・費用、`judge.kind="rule"`。
  `route_defaults` で選んだときは `scores` を空にし、`rule="route_defaults"` と候補の順位を残す。
- `action` は人の補足 2 の「実行方針」。この task で値は `execute`（source・model を選ぶだけ）に固定し、`answer_directly`・`delegate`・`plan`（ExecutionPlan の分解）は
  将来の拡張として予約する（schema に enum を置かず文字列。読み手は未知の値を素通しする）。
- `propensity` は選んだ選択肢が選ばれる確率（探索 ε を含む。主 ADR D2）。決定的な規則で探索なしなら 1。オフライン評価（DR）の材料。
- record に prompt・credential・account の secret は入れない。account は id だけ。

### D6. 出す時点

| 時点 | 出すもの | 理由 |
|---|---|---|
| task 作成時 | 何も出さない | 計画・依存・前の試行で入力が変わる。作成時の snapshot は判断に使われないので学習データにならない |
| routing の判断時（dispatcher が worker run の source・model を決める 1 回ごと） | features snapshot（無ければ作る。内容 hash で重複しない）・options・decision | 判定器に渡る入力は判断時点のもの。`routing_decided` event に `decision_id`・features の schema 版と hash・options の hash を入れる |
| WU（または葉 task）の終わり／run の終わり | outcome（label） | 後で分かったことを別 record にする |
| 評価集合の manifest 作成時 | 各問題の features snapshot（問題の元の WU の、元の判断時点の DB の値から組み立てる。同じ template） | 評価と本番を同じ形にする。manifest に features の hash を書き、manifest の sha に含める |
| 評価 run の割り当て時 | decision（`stage="eval"`、`rule="eval_assignment"`、propensity は割り当て設計の確率） | 評価集合の結果も同じ export に入る |

features の組み立ては daemon の dispatch 経路で行うが、LLM を呼ばない純関数（DB の値 → 文字列）で、`task-core` に置く。

### D7. 保存先（DB 表、migration、events）

build 段 `task-features` が migration で新しい表を作る（番号は全 branch を走査して空きを取る）。どの表も **追記専用**（UPDATE・DELETE しない）。

| 表 | 主な列 | 備考 |
|---|---|---|
| `route_features` | `hash`（PK、本文の sha256）、`schema`、`template`、`subject_kind`、`subject_id`、`brief`（TEXT）、`aux_json`、`created_at` | 内容で引く。同じ hash は 1 行 |
| `route_options` | `hash`（PK）、`schema`、`builder`、`question`、`options_json`、`created_at` | 同上 |
| `route_decisions` | `decision_id`（PK）、`subject_kind`、`subject_id`、`run_id`、`features_hash`、`options_hash`、`input_json`（context・constraints・candidates）、`output_json`、`judge_json`、`stage`、`created_at` | 1 判断 1 行 |
| `route_outcomes` | `decision_id`（PK、FK）、`schema`、`labels_json`、`recorded_at` | 終わった時点で 1 行 |
| `route_judgments` | `judgment_id`（PK）、`features_hash`、`options_hash`、`input_sha256`、`judge_json`、`probs_json`、`source`（sidecar／判定 run の id）、`created_at` | 判定器の出力（D9）。この task では表と API だけ作り、書き手は無い |

events は追記専用のまま: `routing_decided` の payload に `decision_id`・`features: {schema, hash}`・`options_hash` を足す（主 ADR D4 の新 payload の一部）。
brief の本文は event に入れない（event を大きくしない。本文は `route_features` が持つ）。過去の event は書き換えない。
過去の run の features は backfill しない（当時の DB の状態を再現できない欄がある）。export は記録が始まった後の判断だけを出す。

### D8. export（学習データ）

`celerisctl routing export --db <path> --out <file.jsonl> [--since <RFC3339>] [--until <RFC3339>] [--stage shadow|enforce|eval] [--text brief|none]`。
DB は明示の `--db` を読み取り専用（`mode=ro`）で開く。1 行 = 判断 1 回:

```json
{"schema": "celeris.route-training/1", "decision_id": "…", "subject": {…},
 "features": {"schema": "celeris.task-features/1", "template": "task-brief/2", "hash": "…", "brief": "…", "aux": {…}},
 "options": {"builder": "…", "question": "…", "options": [{"option_id": "…", "text": "…", "binds": {…}}]},
 "input": {"context": {…}, "constraints": {…}, "candidates": [{…, "evidence": {…}}]},
 "output": {"selected": {…}, "action": "execute", "scores": [...], "rule": "…", "propensity": 0.9},
 "judge": {…}, "outcome": {"schema": "celeris.route-outcome/1", "labels": {…}} | null,
 "evidence_version": "…"}
```

- `--text none` は brief を落として hash と aux だけを出す（外へ渡すとき用）。既定は `brief`（判定器の入力の主が文であるため）。
- prompt（組み立て済み worker prompt）・会話・log・credential は D2 で brief に入らないので、export にも出ない。
- 評価集合の行は `input.context.stage="eval"` で区別する。同じ問題の本番の判断と評価の判断は `subject` と features の hash で結合できる。
- 出力の各行の順は `decision_id` 順で決定的。同じ DB と引数で byte 一致する。

既存の `celerisctl routing export`（dataset v1、estimator shadow 用）と `routing evaluate`（estimator・旧 shadow の候補 policy 比較）は、
この export で置き換えて同じ変更で消す（[breaking-changes-by-default](2026-10-11-breaking-changes-by-default.md)）。

### D9. `RoutePolicy` の入力と判定器を差し込む境界

`RoutePolicy::choose` の入力を次の構造にする（先行 ADR の `eligible` に足す）:

```text
RouteInput {
  features: FeaturesRef { schema, template, hash, aux },   // brief 本文は DB にあり、policy は hash で引ける
  context, constraints,                                  // D5 の input.context / input.constraints
  candidates: [Candidate + Evidence + route_default_rank], // 適格候補（先行 ADR の admit 後）と根拠表の行
  options: RouteOptions,                                  // candidates-one-shot/1
  judgments: [RouteJudgment],                             // route_judgments のうち features_hash・options_hash が一致し、許可された judge のもの
}
-> RouteOutput { selected, action, scores, rule, propensity }  // D5 の output
```

- 判定器（decision model）は **dispatcher・store の中で呼ばない**。別の段（daemon の外の sidecar process、または事前の判定 run）が
  `route_features`・`route_options` を API で読み、判定して `POST /api/v1/routing/judgments` で `route_judgments` に書く。dispatcher は読むだけで、判定を待たない
  （無ければ judgments は空）。
- 判定結果を使う policy（例 `JudgePolicy`）は、`judgments` の確率で `binds` が適格候補に一致する選択肢から選び、無い・古い（hash 不一致）・許可されていない
  judge のときは主 ADR D2 の根拠表の規則にそのまま落とす。enforce 段の根拠表の同等集合の外へ出るかどうかは、判定器を入れる task の ADR で決める（この task では作らない）。
- この task で作るのは `RouteInput`／`RouteOutput` の型、根拠表 policy と `route_defaults` policy がこの型で判断して D5 の record を書くこと、
  `route_judgments` の表と API。decision model 本体・sidecar・`JudgePolicy` は作らない。

### D10. 置き換える旧い仕組み

| 旧い仕組み | 扱い |
|---|---|
| `RoutingFeaturesRecord`・`RoutingRequestRecord` と `routing_features_recorded` の書き手（`crates/task-core/src/model_router/feedback.rs` ほか） | 主 ADR D4 の model_router 撤去と同じ変更で消す。過去の event は消さない |
| `celerisctl routing export`／`routing evaluate`（`task_ops::routing_replay` の dataset v1・report v1・estimator 比較） | D8 の export に置き換えて消す。主 ADR D4 の「`routing_replay` を残す」は評価集合の出所の抽出部分だけに読み替える（dataset v1・estimator 比較の部分は消す） |
| ADR-0069 の 9 軸 `TaskFeatures` | 残す（lane の決定に使う）。D2 の「性質」節と aux の `task_features_v69` に写す |

## Consequences

- 判断はどの policy でも同じ `celeris.route-decision/1` に残り、`route_outcomes` と結合して JSONL で出せる。判定器を差し替えても record の形と export は変わらない。
- decision model を入れる時は、sidecar と `JudgePolicy` を足すだけで、dispatcher に LLM 呼び出しは入らない。判定の入力文は `route_features` の brief と同じもの。
- 本番の DB に task の目的文の写し（上限つき）が増える。容量は 1 判断あたり最大数 KB（brief は hash で重複しない）。
- 過去の run は backfill しないので、学習データは記録開始後から貯まる。評価集合（約 30 問×7 model）が最初のまとまった行になる。

## 未解決（build 段で決める）

- secret 置換の規則をどの既存実装から取るか（task-worker の redact 系は worker 側にある。`task-core` へ移すか写すか）。
- brief の各節の字数上限の具体値（要求 4,000 字は仮。評価集合の問題で文の長さの分布を見て決める）。
- `changed_lines` を数える git 操作の置き場所（daemon の終了処理か、export 時の遅延計算か）。

## 実装の付記（2026-10-11、task-features 段）

実装の進捗・特徴量一覧・export の使い方と出力例・試験の証拠は [進捗](../progress/2026-10-11-llm-source-model-routing/task-features.md)。

- 版定数は `crates/task-core/src/route_record.rs`（`TASK_FEATURES_SCHEMA`＝`celeris.task-features/1`・`ROUTE_OPTIONS_SCHEMA`・`ROUTE_DECISION_SCHEMA`・`ROUTE_OUTCOME_SCHEMA`・`TASK_BRIEF_TEMPLATE`＝`task-brief/2`・`CANDIDATES_ONE_SHOT_BUILDER`＝`candidates-one-shot/1`）。export の行は `celeris.route-training/1`（`task-ops::routing_export`）。`route_record_schema_pinned` は定数と固定入力の brief・aux・hash（`sha256:49393ff8…`。aux に `write_set`・`write_set_gaps` を足したときに hash を更新した（brief の書式は不変のため template は `task-brief/2` のまま））を byte 固定する（版を変えると落ちる）。`TASK_BRIEF_TEMPLATE` は `task-brief/1` から `task-brief/2` に上げ、固定試験の期待値と hash を同じ変更で更新した。
- `task-brief/2`（final review の差し戻し対応で `task-brief/1` から上げた）: 「期待する成果物」の受け入れ条件は `AcceptanceInput`（`Criterion` の文と `CheckInput`）を「文（検査: 種類）」の 1 行で出す。`CheckInput` は `Criterion.check` の種類（`command`／`artifact_exists`／`knowledge_page`／`reviewer`／`human`）と `WorkUnitCheck`（`cmd`、常に command）を表し、command は実行ファイル名と subcommand まで（`cargo test -p x` → `cargo test`、先頭の `VAR=value`・option・path 形の引数は落とす）。「過去の試行」は数の行（前の run が無ければ「初回」）、`FeaturesInput.prior_runs`（`PriorRunInput`: status・model・wall_s・落ちた criterion の文と exit code）の 1 run 1 行、repair 数の行。「性質（既存の判定）」は ADR-0069 の 9 軸を固定の日本語名と段（低／中／高）で 1 軸 1 行・軸の順固定（判断の重さ・要求の曖昧さ・決定的な検証のしやすさ・やり直しやすさ・失敗したときの損失・読むべき文脈の大きさ・道具を使う度合い・想定される長さ・領域をまたぐ度合い）で出し、無ければ節ごと省く。aux `task_features_v69` は型つき（`model_policy::TaskFeatures`）。`prior_runs` が無いときは `missing` に入る。
- brief の節は D2 の表の順（`要求` → `期待する成果物` → `触る範囲` → `置かれた位置` → `過去の試行` → `性質（既存の判定）`）。自由文は `redact_secrets`（secret 置換）の後、objective 4,000 字（`OBJECTIVE_MAX_CHARS`）・title・check・各 list 項目 1,000 字の上限。超過は先頭・末尾と `…（N 文字省略、sha256=…）` を残り、`aux.truncated`（`field`・`original_chars`）に記録。
- aux は D3(a) の欄に `truncated`（切断記録）を足す。`write_set` は D2「触る範囲」の書き込み集合と同じ値（brief の「書き込み集合:」節に 1 path 1 行）。判断時点の境界は `AsOf::Timestamp` なら `recorded_at` と run の `started_at`、`AsOf::EventSeq` なら runs と同じくその run を指す event の `seq` と、境界 seq までの event の最大 `ts` 以前の `recorded_at`。行が無い（complete の行も path も無い）ときは `null` で `missing` に `write_set`。欠測は値を `null` のまま `aux.missing` に欄名を入れる。
- D3(a) の表との読み替え（実装の抽出元）: `lane`・`harness`・`task_features_v69` は、判断時の dispatch の値（`RouteFeatureContext`）を優先する。表の「`routing_decided` から」は context が無いときの fallback。`check_command_n`・`check_artifact_n`・`check_reviewer_n`・`check_human_n` は `Criterion.check` の種類と `work_units.json.checks` の数（checks は command のみ）。`retries_n` は `work_units.retries` 列、`repairs_n` は同じ task の `kind='repair'` WU 数。`budget_*` は WU の budget、無ければ task の budget。`deps_n` は subject 種別で抽出元が違う: WU は `work_units.depends_on_json` の配列長、task は `tasks.json.depends_on` の配列長（task で WU の LEFT JOIN 列を読むことはしない。JSON が欠落・不正なら null と `missing`、空配列は 0）。
- DB 読み出しは `task_core::route_extract::features_for_subject(store, Subject{kind:"work_unit"|"task", id}, as_of, context)`（`AsOf::Timestamp` は runs の `started_at` と events の `ts` を境界、`AsOf::EventSeq` は events の `seq` を境界。判断時点より後の情報を入れない）。`context`（`RouteFeatureContext`: lane・harness・9 軸）は dispatch が今回の判断の値を渡す。無いときだけ過去の `routing_decided` から読む。aux の抽出元（`checks` の種類・`work_units.retries`・repair 数・budget・dispatch の context）は [進捗の aux 表](../progress/2026-10-11-llm-source-model-routing/task-features.md) に書いた。outcome label は `task_core::route_extract::outcome_labels_for_run(store, run_id)`（git 操作をしない。`quota_used`・`changed_lines`・`changed_files_n`・`changed_crates_n`・`retries_after`・`repaired` は未計測のため `null` ＋ `missing`）。
- 記録の時点: dispatch が worker run の source・model を決める 1 回ごとに `SqliteStore::record_route_features`（`TaskStore` trait。抽出 `features_for_subject` → `insert_route_features` で内容アドレス化・追記専用）が `route_features` に書き、`RoutingRecord.features: RouteFeaturesRef{schema, template, hash}`（`routing_decided` event）に参照だけ入れる（本文は event に入れない）。旧 event は `features` 無しでも復元可能（`Option`）。抽出・保存の失敗は警告ログで dispatch を続行。
- export は `task_ops::routing_export::export(db_path, &ExportOptions{since, until, stage, text}, out)`（明示 path を `mode=ro` URI で開く、行の順は `decision_id` 順で決定的）。CLI は `celerisctl routing export --db <path> --out <file.jsonl> [--since <RFC3339>] [--until <RFC3339>] [--stage shadow|enforce|eval] [--text brief|none]`。`--text none` は `features.brief` を落として hash と aux だけを出す。
- D10: 旧 `celerisctl routing export`（dataset v1）・`celerisctl routing evaluate`（estimator・旧 shadow 比較）と `task-ops::routing_replay` を消して新 JSONL に置き換えた。`routing show`（config 表示）だけ残す。ADR-0069 の 9 軸 `TaskFeatures` は残す（brief の「性質」節と aux の `task_features_v69` に写す）。

### 未解決 3 点の決着

- **secret 置換の規則**: task-worker に自由文向けの汎用 token/key redactor が無いため、`task-core::route_record::redact_secrets` に Bearer、`sk-`/`sk_live_`/`ghp_`/`gho_`/`github_pat_`/`xox*` 等の token 形（トークン部 8 字以上）、`api_key=`/`token=`/`secret=`/`password=` の key=value 形を写した（`task-worker` への依存を作らない）。走査は byte 列の 1 回で線形（照合は ASCII の接頭辞だけなので char 境界でしか当たらず、照合しない区間は元の `&str` の slice で写す）。20 万字の入力は `route_record_redact_large_input_replaces_every_secret` で固定。自由文は record 化前に通す。`route_record_secret_replacement_covers_token_and_key_shapes` で固定。
- **字数上限の具体値**: objective 4,000 字（D2 の仮値を維持）、title・check・各 list 項目 1,000 字。評価集合の問題で文の長さの分布を見て調整する場合は `task-brief/2` の版を上げて固定試験を更新する。
- **`changed_lines` の git 操作の置き場所**: 未決・未実装。現状 `outcome_labels_for_run` は git 操作をせず `null` ＋ `missing` を返す。置き場所（daemon の終了処理か、export 時の遅延計算か）と実装は selector／rollout 段で決める。
- **selector へ残す未実装**: `route_decisions`・`route_outcomes` の書き手（dispatch は features snapshot の記録まで。判断 1 回の `celeris.route-decision/1` 行と run 終わりの `celeris.route-outcome/1` 行を書くのは selector 段）、`POST /api/v1/routing/judgments` の API と判定器本体・sidecar・`JudgePolicy`（D9。この task では `route_judgments` 表と store API（`insert_route_judgment` / `route_judgment_get` / `route_judgments_for_hashes`）のみ）。
