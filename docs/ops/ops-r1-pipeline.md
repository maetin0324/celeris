---
tasks: [01M655T6CGPRQ2HSQPYQFXYGZX]
---
# ops-r1 の採点〜集計の経路（manifest・human-op condition・各段の入口）

ops-r1（measurement_id `mlsys-main-4-ops-r1-k4p1`）の 2 condition を既存の campaign 道具で採点・照合・集計するための形を決める。
判断の理由は ADR `agent-docs/adr/2026-10-30-ops-r1-pipeline-manifest.md`。実装は同じ task の pipe-manifest・pipe-raw・pipe-assemble・pipe-e2e の葉が行う。

verdict: feasible

daemon（crates/campaign 以外の crate）の変更は要らない。B の受付・工程・消費は `ops-r1-intake`・`ops-r1-trace`・`ops-r1-attribution`
（すべて crates/campaign）が既に出しており、足りないのは「その記録を runner の out の形に組み立てる入口」と「manifest と human-op で
condition を表す欄」だけで、どちらも crates/campaign の中で足せる。

## 1. 結論

- manifest は既存の `celeris.campaign-manifest` のまま使い、ConfigId は A・B を流用する。ops-r1 用に、省略時は直列化しない欄
  `conditions`（arm → condition_id・受付経路・trial_id）を足す。B の受付経路を `cos_thread` と宣言した manifest は、
  `run`・`preflight`・`cap-probe` など task API に投入する道具が拒否する（B を `POST /tasks` 投入として扱わない）。
- 新しい入口 `celeris-campaign ops-r1-assemble` が、A の raw と B の intake・trace・attribution と両腕の成果物 dir から、
  runner と同じ形の out dir（`campaign.json`・`input-manifest.json`・`run-meta.json`・`trials.jsonl`・`ledger.jsonl`・`summary.json`・
  `lock.json`・`workspaces/`）と decision-log 形の `records.jsonl` を作る。時刻・host・実行 path を出さず、同じ入力から 2 回作れば byte 一致にする。
- その後の grade-out → check → export → human-op report → final-accounting → `scripts/campaign/check-out-consistency.py` は
  既存の subcommand を `--manifest` 付きでそのまま使う（ops-r1 専用の分岐は manifest の `conditions` を読む数か所だけ）。
- human-op は `--condition <condition_id>` を足す（`--arm` と排他、`--manifest` 必須）。記録は experiment_set `ops-r1`・arm A/B と、
  省略時は直列化しない `condition_id` を持つ。`--arm` の値（A0/B0/B1/C1）は変えない。
- `stage-grader-inputs` は既に manifest 任意で動く（`crates/campaign/src/main.rs` の `stage-grader-inputs` 分岐）。ops-r1 の manifest の
  `problems`（`k4-cmp4-validity-analysis` 1 件）で絞って公開入力 tgz を grade-case の隣へ写せる。preflight の
  「stage-grader-inputs は manifest 必須」は誤りで、manifest が無かったので `--manifest` を付けられなかっただけ。

## 2. 各段が manifest から読むもの（現状）

| 段（入口） | manifest から読むもの | out から読むもの | ops-r1 で困る点 |
|---|---|---|---|
| grade-out（`grade_out.rs`） | `compute_sha256`（trial の campaign_sha256 照合）、`split`（locked な grade-case の配置） | `trials.jsonl`・`ledger.jsonl`・`lock.json`・`workspaces/`・`observed/` | trials.jsonl が無い（runner を通さない） |
| check（`checker.rs`） | configs（harness・model・版）、有効 arm・problems・repeats、budget・termination、contract の pinned_versions、experiment_sets.fixed | `run-meta.json`（executor）・`campaign.json`・`trials.jsonl`・`grades.jsonl`・`summary.json`・`lock.json` | B の routing 必須は `executor ∈ {celeris, arm-dispatch}` のときだけ（`celeris_run`）。ops-r1 は別の executor 名にすれば routing を要求されない |
| export（`export.rs`） | check と同じ＋lock | 同上＋final-accounting | 入力が揃えば変更不要 |
| final-accounting（`final_accounting.rs`） | campaign_id・hash・有効 arm・repeats・`budget.token_basis` | `trials.jsonl`＋`--records`（decision-log/1 の usage・run・trial 行） | B の消費は `celerisctl campaign export` では結べない（CoS 起票の task は campaign 識別を持たない）。attribution から decision-log 形に写す |
| human-op record/report/join（`human_ops*.rs`） | `--manifest` のとき `compute_sha256` だけ。arm は `--arm` の固定表（A0/B0/B1/C1 → experiment_set・ConfigId） | report の `--events` に out dir と records.jsonl | ops-r1 の condition を表す値が無い |
| stage-grader-inputs（`inputs.rs`） | `problems` だけ（任意） | なし | 変更不要 |
| check の (A, B) 組の検査（`checker.rs` の `reconcile_sets`） | `compare` に (A, B) があると、B の observed run の model がすべて Sonnet で、model 欠測も違反（`check_b0_observed`） | trials.jsonl の `observed` | attribution の行は model を持たない。DB の写しの `runs.model`・`runs.adapter` にはある（preflight の DB で worker run の model `claude-sonnet-5-5` を確認） |
| `manifest::validate`・`check_resolved` | `split=final` で B/C の routing を必須にする | — | CoS 受付の B は campaign が routing を固定しない。`conditions` で B=cos_thread のとき免除する |

## 3. condition の表し方

### 3.1 manifest の追加欄

```json
"conditions": {
  "A": {"condition_id": "mlsys-main-4-ops-r1-k4p1-A-direct-claude-code", "intake": "direct_claude_code",
        "trial_ids": ["mlsys-main-4-ops-r1-k4p1-A-t001"]},
  "B": {"condition_id": "mlsys-main-4-ops-r1-k4p1-B-celeris-ops", "intake": "cos_thread",
        "trial_ids": ["mlsys-main-4-ops-r1-k4p1-B-t001"]}
}
```

- 型は `BTreeMap<ConfigId, ConditionDecl>`（`deny_unknown_fields`）。`#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]` で、
  無い manifest の直列化・sha256 は今と同じ bytes になる。
- `intake` は `direct_claude_code`（A だけ）と `cos_thread`（B だけ）の 2 値。`trial_ids` の長さは `repeats` と同じで、`rep` 番目（1 始まり）の trial_id を表す。
- 検査（`validate`）: `conditions` を書くなら有効 arm を全部覆う、condition_id は互いに違い空でない、`contract.revision = main-4-ops-r1` を必須にする。
  逆に `contract.revision = main-4-ops-r1` で `conditions` が無いのも拒否する（ops-r1 契約の manifest は必ず condition を持つ）。
- `check_resolved`: `conditions.B.intake = cos_thread` なら B の routing 欠落を拒否しない。B の routing を書いたら拒否する（CoS が起票する task に campaign は routing を固定しない）。
- 投入の拒否: `run`・`dry-run`・`preflight`・`cap-probe`・`config-check`・`env-check` は `conditions` のある manifest を
  「ops-r1 の manifest は ops-r1-assemble と採点〜集計の段だけで使う」で拒否する（B を `POST /tasks` で流す道を塞ぐ）。
- 実験セット: `experiment_sets = {"ops-r1": {arms: [A, B], compare: [[A, B]], fixed: {...}}}`、`selected_sets = ["ops-r1"]`。fixed には
  `models`・`initial_info`（materials・kb snapshot の hash）・`success`（grade-case の sha256）を書く。`role_models` は B0 用の検査
  （`check_b0_all_sonnet`）に当たらないよう ops-r1 では書かない。B の CoS だけ opus という model の差は `conditions` ではなく固定票と
  attribution の `role`・model で照合する（下の §4.2）。

ops-r1 manifest の例（実値は pipe-assemble が freeze の固定票から作る。ここは形だけ）:

| 欄 | 値 |
|---|---|
| schema | `celeris.campaign-manifest/1`（既存の CAMPAIGN_SCHEMA） |
| campaign_id | `mlsys-main-4-ops-r1-k4p1` |
| split / problems / repeats | `final` / `["k4-cmp4-validity-analysis"]` / 1 |
| configs.A | harness `claude_code`、harness_version `2.1.303`、model `claude-sonnet-5-5`、policy_version は固定票の A の版、routing なし |
| configs.B | harness `celeris`、harness_version `94f86928dc13`（release）、model `claude-sonnet-5-5`（worker）、policy_version `cos-v6`、routing なし |
| contract | revision `main-4-ops-r1`、pinned_versions A{claude_code_version, claude_code_sha256}、B{claude_code_version, claude_code_sha256, pricing_version, cos_policy_version} |
| budget | token_basis `all`、per_case.max_tokens 1,500,000、per_case.max_wall_secs 1800、max_parallel 1 |
| termination | trial_timeout_secs 1800、max_attempts_per_trial 1 |
| order | 既定（A→B の順は `arm_rank` に記録する） |

### 3.2 human-op の condition 指定

- `human-op record|correct|list` に `--condition <condition_id>` を足す。`--arm` と同時には書けない。`--manifest`（path）が必須で、
  `conditions` から arm（A/B）を引く。`--manifest-sha256` だけでは引けないので拒否する。
- 記録の欄: `experiment_set = "ops-r1"`（`ExperimentSet` に `OpsR1 = "ops-r1"` を足す）、`arm` は A/B、`condition_id` を新しい任意欄
  （`#[serde(default, skip_serializing_if = "Option::is_none")]`）に入れる。`arm_label()` は `ops-r1-A`・`ops-r1-B` を返す。
- `--scope shared` は `--experiment-set ops-r1` を受ける（試行に配らない監督・準備の作業）。
- `--arm` の値・意味、`main-ab`・`jev-bc` の記録、既存の `record_id` は変えない（任意欄は無ければ直列化されず、record_id の材料も変わらない）。
- `--arm` に値を足す案は採らない（理由は ADR D3）。

## 4. 各段の入力への写像

`ops-r1-assemble` の入力:

- `--manifest`（上の ops-r1 manifest）、`--input-manifest`（既存の input manifest。`input.sha256` と一致）
- A: `--a-raw <ops-r1-a-raw/1 の JSON>`、`--a-artifact <A の作業 dir の写し>`
- B: `--b-intake <ops-r1-intake/1>`、`--b-trace <ops-r1-trace/1>`、`--b-attribution <ops-r1-attribution/1>`、`--b-artifact <B の成果物 dir の写し>`、
  `--b-observed-versions <health と claude --version の記録>`
- lock の材料（`--cases-root`・`--grader-root`・`--grade-case-root`・`--measurement-id`。`run` と同じ `expected_lock` で lock.json を作る）
- `--out <dir>`（空であること）、`--records-out <file>`

A の raw（`ops-r1-a-raw/1`）は pipe-raw が crates/campaign に足す小さな記録で、測定者が runbook の手順で作る。欄は
trial_id・condition_id・started_at・ended_at・stop（`exited`|`cutoff`|`token_cap`|`operator_stop`）・exit_code・claude_code{version, sha256}・
models（transcript の assistant message の model の集合）・token_usage 4 欄（transcript の usage を message id で重複を除いて和）・
cost_usd（Claude Code の報告額。無ければ null）・workspace_sha256_initial・session_id。transcript の読み方は
`operator_usage.rs` の transcript 読みを再利用する。

### 4.1 試行の識別・試行数・分母

| 段の入力（TrialRecord / ledger の欄） | A（直接 Claude Code） | B（CoS 受付） |
|---|---|---|
| config | `A` | `B` |
| problem_id | manifest の problems[0] | 同左 |
| rep / trial_no | `conditions.A.trial_ids` の位置（=1）/ 1 | `conditions.B.trial_ids` の位置（=1）/ 1 |
| arm_rank | 1（固定票の order） | 2 |
| 試行の対応確認 | raw の trial_id・condition_id が manifest の `conditions.A` と一致 | intake・trace・attribution の trial_id が 3 つとも同じで `conditions.B.trial_ids` にある。attribution の condition_id と intake の condition_id が `conditions.B.condition_id` と一致 |
| 分母 | 1（manifest の計画 cell。checker の `expected` が数える） | 1（同左）。trace の outcome が無い・cutoff 超えでも trials.jsonl に 1 行書き、分母から落とさない |
| ledger.task_id / run_id | null（`not_applicable`） | trace `created.ref.root_task_id` / `executed.ref.runs` の最初の worker run |
| ledger.child_task_ids | 空 | `created.ref.extra_root_task_ids` ＋ `executed.ref.task_ids` から root を除いた残り |
| ledger.started_at / submitted_at | raw.started_at（epoch 秒） | intake.accepted_at |
| run-meta.executor | `ops-r1-assemble`（checker の `celeris_run` は false になり、B の task 単位 routing を要求しない） | 同左 |

### 4.2 終了理由・wall・消費

| TrialRecord の欄 | A | B |
|---|---|---|
| wall_secs | `ended_at - started_at`（秒、切り捨て）。cutoff のときは 1800 に揃える（checker は Timeout の wall が trial_timeout と等しいことを求める） | `min(report.at, cutoff) - intake.accepted_at`。report 前に cutoff なら 1800 |
| outcome / reason（executor の終了。成否ではない） | stop=exited・exit 0・成果物あり → `passed`/null。exit 0・成果物が初期と同じ → `failed`/`no_artifact`。exit≠0 → `failed`/`harness_exit`。stop=cutoff → `timeout`/null。stop=token_cap → `cancelled`/`case_budget`。stop=operator_stop → `cancelled`/`operator_stop` | trace outcome.to=done かつ `cutoff.ref.report_before_cutoff=true` → `passed`/null。to=failed → `failed`/`task_failed`。to=cancelled → `cancelled`/`task_cancelled`。outcome.at が無いか cutoff 後 → `timeout`/null。attribution の 4 欄の和が 1,500,000 を超え測定者が止めた → `cancelled`/`case_budget` |
| passed | outcome=passed の写し | 同左 |
| token_usage | raw.token_usage 4 欄（cache_creation = transcript の cache_creation_input_tokens） | attribution の行の和: input・output・cache_read・cache_creation（= `cache_write_tokens`）。1 行でも null の欄があればその欄は null |
| tokens | `token_usage.count(all)`（4 欄が揃うときだけ。欠けたら null。0 にしない） | 同左 |
| cost_micro_usd | raw.cost_usd × 1e6（Claude Code の報告額） | attribution の cost_usd の和（全行が値を持つときだけ） |
| observed | 1 行: role `worker`、harness `claude_code`、model（raw.models が 1 つのとき） | attribution の `source=runs` の行（worker・reviewer・planner）を 1 run 1 行。model・harness は attribution に足す欄（`runs.model`・`runs.adapter`。schema を `ops-r1-attribution/2` に上げる）から写す。CoS の chat_runs は observed に入れず records.jsonl に入れる。CoS の model（固定票では claude-opus-5-5）は assemble が `chat_runs.resolved_config_json` から読んで照合し、違えば条件逸脱として assemble を止める |
| observed_versions | raw.claude_code の版・sha256 | `--b-observed-versions`（測定 daemon の `GET /health` の release・pricing_version・cos_policy_version と Claude Code の版・sha256） |
| harness・model・版の 4 欄 | manifest の configs.A の写し | manifest の configs.B の写し |
| routing | 書かない | 書かない（CoS 起票の task に campaign は routing を固定しない。checker は executor が celeris でないので要求しない） |
| workspace / workspace_sha256_final | `workspaces/A/<problem>/rep1` に `--a-artifact` を写し `hash_tree` | `workspaces/B/<problem>/rep1` に `--b-artifact` を写し `hash_tree` |
| workspace_sha256_initial | raw.workspace_sha256_initial | runbook が B の作業開始前に記録した値（未確定。§6） |
| inputs | 公開入力 tgz の name・sha256（両腕で同じ） | 同左 |

`no_artifact`・`case_budget` 以外の reason（`harness_exit`・`operator_stop`・`task_failed`・`task_cancelled`）は checker の reason 規則
（`checker.rs` の budget 系・blocked・no_artifact の検査）に当たらない自由文で、分母にも残る。pipe-assemble は checker がこれらを
違反にしないことを試験で確かめる（未確認）。

### 4.3 合否

| 段 | 入力 | 写し方 |
|---|---|---|
| grade-out | trials.jsonl の workspace と hash、grade-case（`stage-grader-inputs` で公開入力 tgz を隣に置いた dir） | 変更なし。outcome は読まない。成果物が無い試行も grader が not_submitted → fail（model_failure）にする |
| check | grades.jsonl | 変更なし。success は独立 grader の final_pass だけ（固定票の成功契約） |
| export / check-out-consistency.py | campaign.json・trials.jsonl・grades.jsonl | 変更なし。成功/失敗/欠測の件数と分母を独立に再計算して照合する |

### 4.4 消費（final-accounting の records.jsonl）

attribution の 1 行を decision-log/1 の `usage` 行と `run` 行に写す（`final_accounting.rs` の `parse_usage` が読む欄名に合わせる）。

| records.jsonl の欄 | 値 |
|---|---|
| schema / record_type | `celeris.decision-log/1` / `usage` と `run` |
| campaign_id / manifest_hash | manifest の campaign_id / `compute_sha256` |
| arm / case_id / repetition | `B` / problem_id / rep |
| run_id / task_id | attribution の run_id / task_id（CoS の chat_runs は null） |
| role | attribution の role（`cos_project`・`cos_general`・`planner`・`worker`・`reviewer`） |
| input_tokens・output_tokens・cache_read_tokens | attribution の同名欄 |
| cache_creation_tokens | attribution の `cache_write_tokens` |
| list_cost_usd | attribution の cost_usd（cost_basis が単価表の換算のとき）。それ以外は null |
| is_retry / shared_overhead | false / false（trial に帰属した行だけを写す。帰属しない行は attribution が出さない） |
| dedup_key | `ops-r1:<run_id>` |

A は decision-log を持たないので records.jsonl に行を書かない。final-accounting は A の消費を外側 trial の値のまま `undecomposed` に置く（既存の規則）。
本番 daemon（監督）と外部 operator の消費は records.jsonl に入れない（固定票の消費の分け方。operator-usage の別欄）。

### 4.5 human-op report

`human-op report --file human-ops.jsonl --events <out> --events <records.jsonl のある dir> --out <dir>` をそのまま使う。join は
`(manifest_sha256, case, arm, rep)` で結ぶので、`--condition` で書いた記録（arm A/B・同じ manifest_sha256）は assemble の out と結び付く。
arm 別の表は `arm_labels` が `ops-r1-A`・`ops-r1-B` になる。

## 5. 旧系列を変えない根拠

- manifest: 足す欄は `conditions` 1 つで、`skip_serializing_if = "BTreeMap::is_empty"`。旧 manifest（A0/B0/B1/C1、`mlsys-main-4-v4`）は
  この欄を持たないので、`normalized()` → canonical JSON → sha256 の bytes は変わらない。`ContractRevision` などの既存の型は変えない。
  pipe-manifest は、公開済みの値（`docs/ops/mlsys-main-4-pilot-runbook.md` の R2 campaign_sha256 `3756ff02…1c65`、K4 pilot `e9fb2320…eee0`、
  K2 pilot `3447ea28…3a0e`）を、その manifest の写しが手元にあれば再計算で照合し、無ければ repo の fixture manifest の sha256 を
  変更前後で比べる試験を置く。
- validate・check_resolved: 追加の検査は `conditions` か `contract.revision = main-4-ops-r1` があるときだけ走る。旧 manifest はどちらも持たない。
- checker・export・final-accounting・grade-out: コードの分岐を足さない（executor 名 `ops-r1-assemble` は既存の `celeris_run` 判定の外に落ちるだけ）。
- human-op: `ExperimentSet` に値を足すだけで、`main-ab`・`jev-bc` の文字列・`--arm` の 4 値・record_id の計算は変えない。`condition_id` は
  無ければ直列化しない。旧 human-ops.jsonl の行は byte 単位で同じに読み書きされる。
- 台帳: assemble は `global-ledger`・`series-ledger` を開かない。旧 `mlsys-main-4-v4` の global-ledger は使わない（固定票 bundle.ledger）。
- 試験: pipe-manifest が「旧 manifest の serialize と sha256 が変わらない」「旧 human-op 記録の record_id が変わらない」試験を足し、
  既存の manifest_tests・human_ops の試験がそのまま通ることを確かめる。

## 6. 未確定・未確認の事項

- B の `workspace_sha256_initial` と `--b-artifact` の取り方: 固定票の「開始前に残る条件 2（B の worker に公開入力と kb/snapshot.md を
  渡す方法）」と一緒に runbook で決める。assemble は値と dir を引数で受けるだけにする。
- (A, B) 組の B0 検査（B の observed run が全部 Sonnet）は ops-r1 でもそのまま効かせる。固定票では planner・worker・reviewer が
  claude-sonnet-5-5 なので、正しく動いた試行は通り、別 model の run は違反になる。observed に model が入ることが前提（§4.2）。
- set 宣言の照合（`reconcile_set_fixed`）は「宣言した欄の証拠が無いと違反」になる。fixed に `policy`・`harnesses`・`role_models` を書かず、
  `models`・`initial_info`・`success` だけにしたとき routing の無い B が通るかは未確認。pipe-assemble が試験で確かめ、違反になるなら
  executor 名で検査を外すのではなく、`conditions.B.intake = cos_thread` を読んで外す。
- `harness_exit`・`operator_stop`・`task_failed`・`task_cancelled` を checker が受けることは未確認（§4.2）。
- check-out-consistency.py が `run-meta.executor` の値を見るかは未確認。見るなら `ops-r1-assemble` を受けるように scripts/campaign で直す。
- 人の principal の token（preflight の g-intake-human-principal）と releases_dir の写し（g-measure-release-not-current）は runbook の担当で、この経路の設計には影響しない。

## 7. 葉への分担

| unit | 作るもの |
|---|---|
| pipe-manifest | `conditions` 欄と検査、投入系 subcommand の拒否、human-op の `--condition`・`ExperimentSet::OpsR1`・`condition_id`、旧 manifest・旧記録の不変の試験 |
| pipe-raw | A の raw 記録 `ops-r1-a-raw/1`、attribution の model・harness 欄（`runs.model`・`runs.adapter`、schema `ops-r1-attribution/2`） |
| pipe-assemble | `ops-r1-assemble` 入口、ops-r1 manifest の雛形、既存段（stage-grader-inputs・grade-out・check・export・final-accounting）の ops-r1 での通し、§6 の未確認の点の確認 |
| pipe-e2e | `crates/celeris/tests/` か `crates/campaign/tests/` の `ops_r1_pipeline*` 試験: A・B の stub 記録で assemble → stage-grader-inputs → grade-out → check → export → human-op report → final-accounting → check-out-consistency.py が全段 exit 0、試行数・分母・終了理由・合否の一致、同じ raw から 2 回作った out と records.jsonl の byte 一致 |
