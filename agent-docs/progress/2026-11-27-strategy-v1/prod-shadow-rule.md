---
tasks: [01M7XQ9PS0AFKKMRJ7P5ZMKQQE]
wu: count-rule
status: done
updated: 2026-11-28
completed: 2026-11-28
---
# 戦略層 v1 親 count-rule: 本番の R1 shadow の記録の数え方（事前に固定）

## 結論

- 決定 after-shadow の b（本番の shadow）のため、本番の記録を見る前に数え方と判定の順をこの file で固定する。数えるのは台本 [`prod_shadow.py`](prod_shadow.py) で、試験は [`test_prod_shadow.py`](test_prod_shadow.py)（24 件）。
- 本番の今の件数（2026-11-28T07:00:59Z に export）: 案件 9 件のどれも strategy root は 0 件。`project_controller_state` の行も 0 件で、controller の mode はどの案件も未設定（既定の off）。script の領域 `project_controller` の mode は `rust`。判定は `inconclusive`（shadow の案件が無い）。記録は [`prod-shadow/preflight-snapshot.json`](prod-shadow/preflight-snapshot.json) と [`prod-shadow/preflight.json`](prod-shadow/preflight.json)。
- 本番 DB には読み取り専用（`mode=ro`）で開いただけで、DB と config には書いていない。code・子の進捗・既存の `r1_*.py`・`shadow_bench.*` は変えていない。

## 固定する値

- K = 3。`crates/task-core/src/parent_checkpoint.rs` の `STALL_K` と同じ値で、台本の `K` も 3。観察の後に変えない。
- 窓: 始まりは記録の最初（表の全行を数える）、終わりは export の時刻（snapshot の `exported_at`）。controller はこれまで本番で 1 度も判断を書いていない（preflight で判断 0 件）ので、全行を数えても shadow を始めた後の記録だけになる。

## 数える物（本番 DB の表と行）

| 物 | 表と条件 | 根拠の code |
|---|---|---|
| R1 の判断（発火） | `project_controller_decisions` で `validation = 'pc/r1-stall'` の行。root は `effect_json` の `question_key`（`r1_stall:<root id>:<終端の子の数>`）から取る | 規則の id は `validation` 列に入る（`crates/task-dispatch/src/dispatcher/project_wake.rs` の `validation: decision.rule_id`）。key の形は `control-ts/src/domains/project_controller.ts` の `pc/r1-stall` |
| R1 の待ち（発火に数えない） | `validation` が `pc/r1-stall/` で始まる行（2 回目からの `pc/r1-stall/waiting`）。件数だけ写す | `escalateOnce` の `${rule}/waiting` |
| 直下の子の終端 | strategy root の `tasks.parent_id = root` の行で、`status` が `done`・`failed`・`cancelled` | `Status::is_terminal`、`project_snapshot::checkpoints` の直下の子 |
| Outcome の reached | `project_roadmap_outcome_events` で `kind = 'status'` かつ `status = 'reached'` の行 | Roadmap の到達の記録 |
| guard の escalate | `project_controller_decisions` で `validation` が `pc/guard/` で始まるか `pc/no-progress` に等しく、`effect_json` の `kind` が `open_escalation` の行 | `dispatcher/project_guard.rs` の `pc/guard/{name}`、TS の `pc/no-progress` |
| controller の起票 | `tasks.labels_json` に label `project-controller` を持つ行 | `task_core::project_proposal::CONTROLLER_LABEL` |
| controller の CoS run | `chat_messages.client_message_id` が `pc-input:` で始まる行（この入力が案件の CoS run を起こす） | `task_core::project_wake::COS_INPUT_KEY_PREFIX`、`chat/project_controller_input.rs` |
| controller の受信箱の項目 | `cos_inbox_items` で `source_kind = 'notice'` かつ `source_key` が、`feed_sources.source_key` が `project_controller:` で始まる行の `notice_id` | `project_wake.rs` の `open_escalation`（`project_controller:escalate:<案件>:<key>`） |
| controller の mode・phase | `project_controller_state` の行。行が無い案件は `null`（既定の off） | |
| script の領域の mode | `script_domain_modes` の全行 | |

## strategy root の見分け方

`crates/task-ops/src/project_snapshot.rs` の `checkpoints` と同じ条件にする。

1. 案件の root: `tasks.project_id` が案件で、`parent_id` が NULL、`json` の `tree.parent_unit` と `conversation` が無い（`task_core::org::is_root_task`）。
2. その root を `target_id` に持つ `cos_operations` の行（`action = 'task.create'`・`state = 'applied'`・`payload_json` の `strategy.name` が NULL でない。順は `created_at, id`）のうち最初の行がある（`task_core::strategy_reads::strategy_task_creates(store, Some(root))`）。
3. その行の `strategy.name` が `strategy/<id>/v<版>.json`（`<id>` に `/` を含まない、版は先頭 0 の無い u32）に合う（`task_core::strategy_ref::strategy_artifact`）。

限界: controller の snapshot は大きさの上限を超えると done の root を落とす（`truncated`）。台本は落とさないので、そのときは台本の root の数が controller の見た数より多くなりうる（未確認。本番で上限を超えた例はまだ無い）。

## 定義

- 発火: R1 の判断 1 行。
- 誤検知: 発火の後（発火と同時刻は含まない）、窓の終わり（export の時刻。同時刻を含む）までに、同じ案件の Outcome が reached になった発火。
- 先行: 同じ root で guard の escalate より前の発火。guard が無いものを含む。
  - guard の判断は root を持たない（案件単位）。そこで「同じ root の guard」を、同じ案件で root の起票の時刻以後（同時刻を含む）の guard の escalate とする。
  - 発火の時刻まで（同時刻を含む）にその guard が 1 件も無い発火を先行と数える。
  - 発火の root が snapshot の strategy root に無いときは、案件の guard の escalate を全部使う。
- 安全違反: 次の件数の和。
  - `effect_state` が `suppressed` でない R1 の判断（`none`・`pending`・`applied`・`failed`）。
  - controller の起票・CoS run・受信箱の項目（上の表の 3 行。全期間で 1 件でもあれば違反）。

## 判定（上から最初に当たったもの）

1. 安全違反 ≥ 1 → `unsafe`
2. controller の mode が `shadow` の案件が無い、shadow の案件に strategy root が無い、または shadow の案件に直下の子の終端が 3 件（K）以上の strategy root が無い → `inconclusive`
3. 誤検知 ≥ 2 → `falsified`
4. 先行 ≥ 1 かつ 誤検知 0 → `useful`
5. それ以外 → `no_signal`

誤検知と先行は全案件の発火で数える（案件の mode で絞らない）。`suppressed` でない発火は 1 の安全違反で先に当たる。

## 時刻の比較

時刻は RFC 3339（`Z` か `±hh:mm`）を UTC に直して比べる。小数は 6 桁（μs）に切る。DB の時刻は ns までなので、同じ μs の中の前後は「同時刻」として扱う（試験 `test_reached_at_the_fire_time_is_not_after`）。

## snapshot と結果の形

- snapshot の key は次だけにする（`prod_shadow.py` の `ALLOWED_KEYS`）。verify はこれ以外の key と、200 字を超える文字列を落とす。
  - id: `schema_id`・`project_id`・`root_id`・`task_id`・`decision_id`・`event_id`・`outcome_id`・`message_id`・`thread_id`・`run_id`・`item_id`・`domain_id`・`generation_id`
  - 時刻: `exported_at`・`created_at`・`updated_at`
  - status: `status`・`mode`・`phase`・`state`・`controller_mode`・`controller_phase`
  - 規則の id: `rule_id`（guard は `:` より前だけを写す。後ろは Outcome の id の列で 200 字を超えうる）
  - effect_state: `effect_state`
  - 件数: `k`・`n_projects`・`n_decisions`・`n_r1_waiting`・`n_strategy_creates`（整数）
  - 入れ物: list の `projects`・`strategy_roots`・`children`・`decisions`・`guard_escalates`・`outcome_reached`・`controller_tasks`・`controller_cos_inputs`・`controller_inbox_items`・`script_domains`、object の `counts`
- 問い・summary・note・payload・chat の本文は写さない。
- 結果は `json.dumps(indent=2, sort_keys=True)` で、key `verdict`（`unsafe`・`inconclusive`・`falsified`・`useful`・`no_signal`）・`reason`・`counts`・`fires`（発火ごとの `false_positive`・`lead`）・`safety`（違反の id）・`projects`（案件ごとの strategy root の件数と mode）・`script_domains` を持つ。結果は snapshot だけから決まり、DB を読まない。

## 台本の使い方（repo の root で）

```
D=agent-docs/progress/2026-11-27-strategy-v1
python3 -I $D/prod_shadow.py export --out $D/prod-shadow/<名>-snapshot.json   # 本番 DB を mode=ro で開く
python3 -I $D/prod_shadow.py run --snapshot $D/prod-shadow/<名>-snapshot.json --out $D/prod-shadow/<名>.json
python3 -I $D/prod_shadow.py verify          # prod-shadow/ の全 snapshot を数え直して比べる。違えば exit 1
python3 -I $D/test_prod_shadow.py            # 試験 24 件
```

DB の path は `2026-11-27-strategy-v1-9/audit_query.py` と同じ `/local/celeris/data/db/celeris.sqlite3`（`--db` で変えられる。試験は合成の DB に使う）。

## 本番の今の件数（preflight）

`prod-shadow/preflight.json` の要点（export 2026-11-28T07:00:59.072900Z）:

- 案件 9 件。どの案件も strategy root 0 件（strategy 付きの `task.create` の applied の行が本番に 0 件）。
- controller の mode: 9 件とも `null`（`project_controller_state` の行が 0 件。既定の off）。
- script の領域: `project_controller` は `rust`、`cos_digest`・`lane_rules` は `compare`、`execution_gate`・`route_priority` は `rust`。
- 判断 0 件、R1 の発火 0 件、guard の escalate 0 件、Outcome の reached 0 件、安全違反 0 件。
- 判定 `inconclusive`（理由 `no project with controller mode shadow`）。

この状態のまま shadow を始めると、strategy root が 0 件なので判定は `inconclusive` になる見込み（計画 v4 の見込みと同じ）。

## 試験（`test_prod_shadow.py`、24 件）

- 判定 5 つ: `unsafe`（R1 の `effect_state` が `applied`・`none`、controller の起票・CoS 入力・受信箱のそれぞれ。inconclusive より先）、`inconclusive`（shadow 無し・root 無し・終端 2 件。終端 3 件にすると `useful`）、`falsified`（誤検知 2 件。先行があっても）、`useful`、`no_signal`（発火 0、誤検知 1、guard が先）。
- 誤検知の境界: 発火と同時刻（9 桁の同じ μs も）は数えない、1 μs 後は数える、export の時刻ちょうど（別の時差の書き方も）は数える、その 1 μs 後・発火の前・別の案件は数えない。
- 先行の境界: guard 無しは先行、発火と同時刻の guard は先行でない、1 μs 後の guard は先行のまま、root の起票と同時刻と起票と発火の間の guard は先行でない、root の起票の前と別の案件の guard は無視、snapshot に無い root は案件の guard を全部使う。
- 形: 許さない key・201 字の文字列・件数の型・scalar の key の入れ物を落とす、200 字は通す。verify は snapshot 無し・結果無し・数え直しとのずれ・悪い snapshot で exit 1、一致で 0。
- export: 合成の sqlite で strategy root（参照なし・版の先頭 0・conversation・root でない子・rejected を外す）、R1・待ち・guard（`:` の後ろを落とす）・reached・controller の 3 種を、controller と同じ行で拾い、DB の bytes が変わらず、chat の本文を写さないことを確かめる。`mode=ro` の接続が書き込みを拒むことも確かめる。

既存の試験（`test_r1_rule.py`・`test_r1_bench.py`・`shadow_bench`）は本番の記録の数え方を扱わないので、この規則の誤りを検出できない。

## 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `python3 -I agent-docs/progress/2026-11-27-strategy-v1/test_prod_shadow.py`: exit 0、24 件 OK。
- `python3 -I agent-docs/progress/2026-11-27-strategy-v1/prod_shadow.py verify`: exit 0（`preflight: ok (verdict inconclusive)`）。
- `bash scripts/dev/test-affected.sh`: exit 0（試験対象の変更なし。repo-health verdict=pass・fail_grade=0。new 1・worsened 1 はどちらも警告の `dup.public-surface` で、`prod_shadow.py` の subcommand 名 `run`・`verify` がほかの script と同じことによる。名前は objective が決めているので変えない）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `sh scripts/dev/progress-index.sh --check`・`bash scripts/dev/check-doc-links.sh`: ok。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の path はこの file・`prod_shadow.py`・`test_prod_shadow.py`・`prod-shadow/` の 2 file だけ。`git diff --quiet "$CELERIS_WU_BASE" HEAD -- crates control-ts web scripts docs` と既存の `r1_*.py`・`shadow_bench.*`・子の進捗: exit 0（差分 0）。

## 未確認

- 本番で controller が shadow で R1 を出したときの `effect_json` の `question_key` の形は、TS の試験と r1-ts の記録で確かめただけで、本番の行では未確認（本番の判断は 0 件）。形が違えば `root_id` が `null` になり、先行は案件の guard を全部使って数える。
- strategy root が本番で作られる経路（CoS が strategy を付けて `task.create` する）が動くかは未確認（子 2 の未確認と同じ）。
