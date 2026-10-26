---
title: main-4 pilot readiness — 新 revision（final-freeze-2、mlsys-main-4-v4）での無評価 LLM 全段通しと pilot manifest の lock
tasks: [01M5WC0AZ6MP312BFER7C8QKA0]
status: done
updated: 2026-10-26
---
# main-4 pilot readiness — 新 revision（final-freeze-2、mlsys-main-4-v4）での無評価 LLM 全段通しと pilot manifest の lock

完了日: 2026-10-26

refreeze の新 revision（R2 = `main4-ready/bundle/final-freeze-2`、`rerun/preflight.json` の measurement_id
`mlsys-main-4-v4`）の写しで、既存 launcher の prepare〜final-accounting を stub harness（無評価 LLM）で通した。
12 段すべて初回 exit 0、paid_calls_made 0。続いて実行票 C7 の K4 だけ・K2 だけの pilot manifest を R2 と同じ
shared_ledger で作り lock した（lock exit 0）。pilot の LLM 試行・主測定はしていない。

以下 `A` = `/local/celeris/data/workspaces/01M5SRJMB1CSGBYT028X0B67N2/artifacts/main4-ready`、`N` = `A/rerun/nollm`。
出典は `A/rerun/notes/{collect-stub,nollm-run,nollm-post,consistency,pilot-manifests}.md`・`A/rerun/nollm-pass.json`
（schema `celeris.main4-nollm-pass/2`）・`A/rerun/consistency.json`・`A/rerun/pilot-manifests.json`。

## 変更
- `collect-stub: collect-observed-out は not_submitted だけなら記録を書いて exit 0` — collect の修正（下の「collect の修正」）。
- `integrate wu/collect-stub (phase fix)` — 上の修正の統合。
- 本 WU（close）: この進捗ファイルを追加。

## 各段の exit と ID（nollm-pass.json の stages）
共通 ID: measurement_id `mlsys-main-4-v4`、task_id `01M5WC0AZ6MP312BFER7C8QKA0`。
前半（nollm-run）の run_id `01M5WDNMPXZ9X8WVYC7FXYZEBC`、後半（nollm-post）の run_id `01M5WEETW5JK1N7K2M5A4TRY5Z`。
測定 daemon: pid 838837、instance `01M5WE18R8VJ01QSJEQHAMBHB7`、API 127.0.0.1:7742、release は R2 の RELEASE（読むだけ）、
nollm DB の project `01M5WE1NP3K2EEV028CXNEHD2Q`。

| 段 | exit | real_daemon | 主な ID・結果 |
|---|---|---|---|
| prepare | 0 | false | stage-grader-inputs は nothing to stage |
| start | 0 | true | pid 838837、port 7742 |
| cap-probe | 0 | true | A/B × K2/K4 の 4 本 ok。B probe task K2 `01M5WE218ZYE6FB6FFA424BGQQ`、K4 `01M5WE26T0GQRTWAT4VBV1BA7H` |
| preflight | 0 | true | ok=true、missing []、checked 16 |
| env-lock | 0 | true | campaign_lock・env_lock created → `env-lock --verify` verified |
| watch | 0 | true | settled（12 iteration）。A/k2・A/k4 は A stub、B/k2 task `01M5WE59TJ9NZAC1A4WBN2YR8P`（最終 run `01M5WE73QABRVF4PJA0JEE9QE7`）、B/k4 は arm_budget_cancelled |
| collect | 0 | false | ok=true、not_submitted 2、skipped_no_experiment 1、blocking 0（旧 nollm は exit 1） |
| grade | 0 | false | 3 件採点（model_failure 3）、B/k4 は not run。final_passed 0/4 |
| check | 0 | false | launcher と同じ 40 引数。ok=true、violations []、complete=false（B 1 trial 未実行） |
| export | 0 | false | RECORDS（75 records、`celerisctl campaign export`）で table.incomplete.{json,csv}・final-accounting.json |
| human-op-report | 0 | false | 4 trial 行、shared_overhead [] |
| final-accounting | 0 | false | out/export/final-accounting.json と同内容 |

後始末: DB 写し `N/db-copy/celeris.db`（sqlite backup API、integrity ok、tasks 9・runs 23、
sha256 `c4689fb36c84a5b5ae4f5b898c00beb17269c2efa01ae2986b0ef3b769f82218`）、`launcher stop` 0、port 7742 閉。
不変: R2 全 file と実 global-ledger（`/local/celeris/data/measure/main-4/global-ledger`）の sha256 一覧が前後で一致。
測定 token の値は記録・ログに 0 件。

## fixture（R2 → N の写しで変えた欄）
- campaign bin: 修正入り worktree で `cargo build --release -p celeris-campaign` した `N/bin/celeris-campaign`、
  sha256 `2dd77ed2a28772999878a65bd122450dc9eb1fb3eea6dc0d4c49cb7d39b18e6d`。
  **R2 の CAMPAIGN_BIN（`pinned/evidence/main-k2/bin/celeris-campaign-fixed`）と pins.json の CAMPAIGN_BIN_SHA は
  `5132230995128dc745593546132dfb947e537d1913718bea482cbc83f5ff93dd` で、食い違う**（R2 の bin には collect の修正が無い）。
- launcher.env: MANIFEST, DAEMON_CONFIG, TASK_TEMPLATE, A_PROGRAM（claude-a-stub）, CAMPAIGN_BIN, OUT, STATE_DIR,
  GLOBAL_LEDGER, RUN_PATH, CAP_ENV（stub cap-tools）を替え、INTERVAL_SECS=5・MAX_ITERATIONS=240・HEALTH_WAIT_SECS=120 を追加。
  CELERIS_BIN・RELEASE・MEASUREMENT_ID・CANARIES・NORMAL_CONFIG・資産 root・A_ACCOUNT は R2 のまま。
- final.json: `experiment_sets.main.fixed.shared_ledger` → `N/global-ledger`、`approved_trials` 2 → 4。
  campaign_sha256 `5d37a5ca71a022eeee411a4d095849af76cc5cd32aeca940d29ac917ea1f3911`（R2 と別）。
- daemon.toml: db/token/kb/scratch → `N/state`、workspace_root → 写し専用、port 7742、claude_dir → `N/accounts`（`{}` の印）、
  `[adapters.claude_code].command` → claude-b-stub、`[llm_sources.measure]` fake を追加し claude-code harness・route を fake へ、
  cap-probe harness command → `N/bin/celeris-campaign`。
- task-template.json: project_id → nollm DB の project。
- 共有台帳: `N/global-ledger` を ledger-init（承認は `rerun/ledger-work/approval.json` の写し）で作成。実台帳には書いていない。
- model catalog: start 後に nollm DB へ fake source `measure` の行を sqlite で入れた（launcher.rs と同じ行）。
- 注意: 写した `[[clusters]]` で測定 daemon が sirius の master を borrowed で掴み、pegasus の forward 127.0.0.1:18000 を up と報告した（旧 nollm と同じ）。

## collect の修正
- 原因: `crates/campaign/src/observe.rs` の `collect_out` が失敗 trial 1 件以上で Err にし、stub の not_submitted（job を投げない）と
  測定基盤の class を区別していなかった。
- 直し（commit `collect-stub: collect-observed-out は not_submitted だけなら記録を書いて exit 0`、ADR
  `agent-docs/adr/2026-10-26-collect-not-submitted-exit.md`）: 基盤 class を含む trial があるときだけ Err。報告に
  `not_submitted`・`blocking` を足し `ok` = blocking==0。失敗記録（collect-failure.json・collect-log.jsonl）は従来どおり書く。
  stub に偽の job 記録を書かせる案は観測契約を偽るので採らない。
- 試験（`crates/campaign/tests/launcher.rs`）: `collect_not_submitted_only_writes_failure_records_and_exits_zero`、
  `collect_not_submitted_mixed_with_infra_class_stays_nonzero` を追加し、既存の
  `grade_collects_job_observations_bound_to_the_trial_and_rejects_every_mismatch` の最終段を exit 0 期待に更新。

## 未導出の wait 区間（欠測は null のまま、0 ではない）
- human_wait（4 trial）: human-ops.jsonl が無い（評価者記録なし）。
- celeris_wait（A/k2・A/k4・B/k4）: A は Celeris を通らない、B/k4 は未実行。B/k2 だけ event から 0 秒で導出。
- internal_wait（4）: ready_not_running が入力に無い。
- external_queue_wait・external_run（4）: job 投入なし。
- cluster_wait_ms/cluster_queue_ms/cluster_run_ms/cluster_node_hours（B/k2）: records 全欠測。
- 共有 setup（prepare/start/cap-probe/preflight/env-lock）の待ち・所要: どちらの集計にも入力なし。
- 金額: final-accounting total の tokens・cash・list は null、export の cost_micro_usd_sum は A・B とも null、
  final_cutoff.missing_trials 1。
- 共有 setup の計上は **1 回でなく 0 回**（shared_unattributable rows 0、shared_overhead []）。setup 中の cap-probe task 8 件
  （4 組）が campaign tag なしで作られ RECORDS に入らないため（下の「提案」）。

## 照合結果（consistency.json）
- `python3 scripts/campaign/check-out-consistency.py --out N/out --manifest N/final.json --campaign-bin N/bin/celeris-campaign
  --human-ops-report N/human-ops-report --check-arg=…`（launcher の 40 引数から script が自前で付ける --manifest/--out を除いた 36 個）→ exit 0。
- consistent=true、mismatches []。checker ok・violations []・grades_bound=true、export lock_verified=true。
- 件数: A は n 2 / ran 2 / model_failure 2 / final_passed 0、B は n 2 / ran 1（B/k2 executor_failed）/ 未実行 1（B/k4 executor_cancelled）/
  final_passed 0・tokens 欠測 1。checker・export・human-op report・生記録で一致。
- final-accounting は script の照合外のため `rerun/consistency-work/merge.py` で生記録と突き合わせた: arm ごとの n・ran・not_run・
  tokens 欠測、cell 集合、observed_trial_tokens 210、missing_trials 1、manifest_hash = lock の campaign_sha256、total.tokens null。全一致。

## pilot manifest（pilot-manifests.json）
R2 の final.json から problems を 1 件に絞り campaign_id だけ派生 ID に替えた（他は R2 と同じ）。shared_ledger は R2 と同じ
`/local/celeris/data/measure/main-4/global-ledger`（lock は台帳に触れない）。各 `start → lock → stop` は全て exit 0。

| k | measurement_id | path | sha256 | lock_exit |
|---|---|---|---|---|
| k4 | `mlsys-main-4-v4-pilot-k4` | `A/rerun/pilot/k4/final.json` | `472594e3ceca7fc79a44d455347a41a755398f4f7a0f0ee67c668d99c2c505de` | 0 |
| k2 | `mlsys-main-4-v4-pilot-k2` | `A/rerun/pilot/k2/final.json` | `0bd0db33718536c32f0e2665e6bbf4bb20953793b88002e550718952221654c7` | 0 |

- campaign_sha256: k4 `e9fb2320303a16d17e38ea6831d1d53b337bb5a869514a2a4bbfa39c0b6ceee0`、k2 `3447ea282a8c19696aeae9a1793f863782a11a3083c3727cfd652c91cd683a0e`。
- lock_sha256: k4 `7b840f04399b0abdcc1ac0b209e775c492c60f73085af0b2987c17cd6b7f071b`、k2 `0c236423339bc18dd875e96ea07c0fa791ccf11aa623477e33ad96321bc39938`。
- launcher env の写しは MEASUREMENT_ID・OUT（`pilot/<k>/out`）を替え、K4 だけ REPO_SRC を外した（初期 tree を固定する case が無い）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `sh scripts/dev/progress-index.sh --check` → exit 0
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → exit 0
- `sh scripts/dev/check-doc-links.sh` → exit 0
- collect の修正の検査（collect-stub WU、提出時の HEAD）: launcher 試験 3 件 pass、`bash scripts/dev/test-affected.sh` exit 0（2652 passed, 7 skipped）、
  `cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0。

## 未解決（人）
- **R2 の CAMPAIGN_BIN を修正入り binary へ再固定する**（本 task の統合・release の後）。手順:
  1. 本 task branch が main に入り、release gate（`scripts/selfdeploy/release.sh`）を通ったことを確かめる
     （`git merge-base --is-ancestor <collect 修正の commit> main` が 0）。
  2. その main で `cargo build --release -p celeris-campaign` し、出来た `celeris-campaign` を
     `A/bundle/final-freeze-2/pinned/evidence/main-k2/bin/celeris-campaign-fixed` に置き換える（旧 binary は別名で残す）。
  3. `sha256sum` の値を `pinned/main/pins.json` の `CAMPAIGN_BIN_SHA` に書き、`rerun/revision.json` の `campaign_bin` も更新する。
  4. `python3 scripts/campaign/check-main-pins.py`（R2 の pins.json に対して）→ exit 0 を確かめる。
  5. bin が変わるので env-lock・preflight を R2 で取り直す（lock が内容の変化で新しい measurement_id を要求する場合は新 ID を採る）。
     pilot manifest（k4・k2）の lock も新 bin で取り直す。
- pilot 用の pins を作り check-main-pins を pilot manifest に通すかは未決（pilot-manifests の範囲外）。

## 提案
- cap-probe の decision-log 行に campaign_id と shared_overhead=true を付け、setup を 1 回として数える規則（段ごとに 1 回、
  watch 内の再 probe は除く等）を final-accounting に足す。または launcher の prepare〜env-lock を
  `human-op record --scope shared --mode automatic` で 1 行ずつ書く。
- check-out-consistency.py に final-accounting の照合（merge.py の項目）を入れ、`sources.final_accounting` を script が出す。
- check-out-consistency.py に launcher の引数表を直接受ける入口（例 `--launcher-args <file>`）を足し、--manifest/--out の二重を防ぐ。
- grade-out の未採点行 `{"graded":false,"why":"not run"}` に campaign_sha256 が無く、`celerisctl campaign export --grades` が拒否する
  （exit 1）。未採点行にも campaign_sha256 を書くか、export 側で未採点行を飛ばす。`docs/ops/measure-run-final.md` §8 の例は
  未実行 trial がある out では通らない。
