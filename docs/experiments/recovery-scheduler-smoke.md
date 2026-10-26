# 復旧比較 smoke 実行票（PBS / Sirius）

---
tasks: [01M5TCGEVJF1QW03CDWPJRRKZ5]
---

復旧比較 harness（[ADR](../../agent-docs/adr/2026-10-25-recovery-scheduler-backend.md) D2–D5・付記 I14・I15）を実 PBS に
つなぐ最小 smoke の実行票。本書は投入の承認ではない。承認は「認可の出典」節の人の決定 d-smoke だけで、未回答なら実行しない。
現時点の判定は末尾の「判定」節（`hpc_ready=false`）。

## 対象と資源

| 項目 | 値（両 arm 共通） | 出典 |
|---|---|---|
| cluster / scheduler | Sirius / PBS | ADR D2 |
| queue / account | `gen` / `NBBG` | ADR D2、KB `environment/clusters/sirius-job-submission.md`（2026-09-30 の人の pbs-budget 決定） |
| owner | `rmaeda` | spec の `owner` |
| select / place | `1:ncpus=24:mem=124gb:ngpus=1` / `exclhost`（GPU は使わない） | ADR D2、`scripts/campaign/recovery-smoke/spec.template.json` |
| walltime | 300 秒（job.sh の release 待ち上限 240 秒 + 余裕 60 秒） | ADR D2・I15 (4) |
| rerunnable | false（qsub `-r n`） | ADR D5 |
| payload | `scripts/campaign/recovery-smoke/job.sh` と `input.bin`。出力の期待 sha256 は `expected.sha256` | ADR I15 (4) |
| scenario / point / seed / rep | `rcv-1` / `camp-1` / 17 / 1 | `run_pbs` が `rcv-1`・`camp-1` 以外を拒否、spec の `seed`・`rep` |
| 条件 | normal と A2 を両 arm（baseline・celeris）に 1 回ずつ。計 4 trial = 4 job | ADR D2 |
| workload attempt 上限 | 1（spec `limits.workload_attempts`）。再起動・poll・回収・再採点は同じ attempt / job を続け、枠を補充しない | ADR D2 |

両 arm は同じ spec（payload・入力・期待出力・grader・env・資源・poll/deadline）を使い、別々の job を逐次に流す。
job 1 本は `out/<PBS_JOBID>/` に入力の写しと digest を書き、`release/<PBS_JOBID>` を待ってから `out.bin` を書く。
CPU 負荷・PBS 呼び出し・network は持たない（自己試験 `sh scripts/campaign/recovery-smoke/selftest.sh`）。

## 上限と停止条件

| 上限 | 値 | 効かせる所 |
|---|---|---|
| 総 job 数 | 4（spec `limits.max_jobs` と承認記録 `max_jobs` の小さい方） | `ApprovalGate` が qsub ごとに intent 台帳の `sent` 以降の数で拒否 |
| 同時 job 数 | 1（`limits.external_slots`・`worker_slots`） | 1 起動 = 1 trial。前の trial の `run_pbs` が戻り、その job が `F` になってから次を起動する |
| 試行の実経過 deadline | 1800 秒（`limits.trial_deadline_secs`） | observer と controller の待ち |
| ssh 1 回の全体 timeout | 60 秒（`limits.ssh_timeout_secs`） | `SshPbs`・`SshSite` の `run_bounded`。超えれば子を SIGKILL して Err |
| poll 間隔 | 30 秒（`limits.poll_secs`） | qstat |
| 承認の期限 | 承認記録の `valid_until`（UTC） | 起動時と qsub の直前。期限後の qstat（観測・回収）は続けてよい |
| 予約上限 | 4 物理 host × 300 秒 = 1/3 host-hour | 承認枠と PBS 課金単位の換算は投入前に照合する（未照合） |

次のどれかが起きたら、その場で止める（次の trial を起動しない。再投入・枠の補充をしない）:

- `run_pbs`（`run_from_env`）が Err・panic で終わった。
- unknown submit（qsub の timeout・切断・job ID 不正・受理後の台帳書込み失敗）。`SshPbs::find_by_marker` は
  `Unsupported` なので照合できず、その trial は成功に数えない（ADR D4・I15 (5)）。
- gate が qsub を拒否した（件数超過・期限切れ・承認の差し替え）。
- queue が資源条件を拒否した（縮退・別 queue への自動変更をしない）。
- job が `release` を待たずに終わった、または `out.bin` が無い・期待 sha256 と違う。
- ssh 接続が `BatchMode` で張れない（TOTP の要求が出た）。

## 所有権と取消対象

- 止めてよいのは、測定 runner（`run_pbs`）が自分で起こした測定専用 controller の子 process（同じ test binary の
  `pbs_control_child`）だけ。runner は `/proc/<pid>/stat` の starttime（第 22 欄、`proc::proc_starttime`）を起動直後に控え、
  A2 で停止する直前に同じ pid の starttime と一致することを確かめる（pid の再利用を見分ける）。結果は trial dir の
  `a2.json`（`killed_starttime`）と `process-events.jsonl` に残る。人は手で kill しない。
- 人が実行中に所有を確かめるとき（読み取りだけ）:
  `ps -o pid=,lstart=,args= -p <pid>` で args が dagbench_harness の test binary であること、
  `awk '{print $22}' /proc/<pid>/stat` が `process-events.jsonl` の `starttime` と一致することを見る。
- 止めない・触らない: 本番の celeris daemon（`celeris@*` unit）、SSH master（`ssh -O exit` をしない）、PBS server、
  compute host、他者の job、この測定以外の自分の job。
- Celeris・harness・payload は `qdel`・`qalter`・`qhold`・`qrls`・`qmove` を持たない。残った job は release 待ち上限と
  walltime で自然に終わる。
- 人が手で `qdel` してよいのは、その測定の `<DAGBENCH_OUT>/submit-intents.jsonl` の `accepted` 行にある job ID だけ:

  ```sh
  grep '"accepted"' "$DAGBENCH_OUT/submit-intents.jsonl"   # job ID を目で確かめる
  ssh -o BatchMode=yes sirius qdel <その job ID>            # 人だけが実行する
  ```

  取り消したら進捗に job ID・時刻・理由を追記する（raw は書き換えない）。

## 実行手順

前提: 人の決定 d-smoke が `approve`、`ssh -O check -o BatchMode=yes sirius` が exit 0（ControlMaster が生きている）、
I15 (4) の qsub `-v RECOVERY_SMOKE_DIR=…` が実装済み（「判定」節の不足 3）。どれかが欠ければ始めない。
実行するのは ssh を許された担当（cluster-hpc）か人。作業は repo の作業ツリーで行い、cluster 側には測定 dir だけを書く。

1. 測定 ID と保存先を決める（既存 raw に無い新しい値。例 `rcv-smoke-<YYYYMMDD>-1`）:

   ```sh
   MID=<measurement_id>
   RDIR=/work/NBB/rmaeda/recovery-smoke/$MID                 # cluster 側。basename = MID
   export DAGBENCH_OUT=/local/celeris/data/measure/recovery-smoke/$MID   # 手元 raw。新規・作業ツリーの外
   test ! -e "$DAGBENCH_OUT"                                  # 既にあれば別の MID にする
   W=<その run の artifacts>/recovery-smoke                   # spec・承認・pins・ログ・機械結果の置き場
   mkdir -p "$W"
   sh scripts/campaign/recovery-smoke/selftest.sh             # exit 0
   ```

2. test binary を作り、grader と source の pin を取る（grader は同じ binary の `entry::grade`）:

   ```sh
   cargo test -p celeris-campaign --test dagbench_harness --no-run 2>&1 | tee "$W/build.log"
   BIN=$(grep -o '([^)]*dagbench_harness-[^)]*)' "$W/build.log" | tr -d '()' | tail -1)
   sha256sum "$BIN" scripts/campaign/recovery-smoke/job.sh scripts/campaign/recovery-smoke/input.bin | tee "$W/pins.sha256"
   git rev-parse HEAD > "$W/source.txt"
   ```

3. spec を作る: `spec.template.json` を `$W/spec.json` に写し、`measurement_id=$MID`、`owner=rmaeda`、
   `payload.script=$RDIR/job.sh`、`script_sha256`・`input_sha256`（2. の値）、`expected_output_sha256`
   （`cat scripts/campaign/recovery-smoke/expected.sha256`）、`grader.binary_sha256`（2. の `$BIN`）、
   `grader.version=celeris.recovery-grade/1`、`env.modules=[]`、`env.env_sha256`（`env.modules` の JSON の sha256）を埋める。
   資源・limits・conditions・seed・rep は雛形のまま変えない（変えるなら新しい MID と ADR の更新が先）。
4. study を作る（`$W/study.json`）:
   `{"pins":{"campaign_id":"recovery-smoke-$MID","manifest_sha256":<spec.json の sha256>,"case_sha256":<expected.sha256 file の sha256>,"source":"agent-platform@<source.txt>","binary_sha256":<$BIN の sha256>},"problem_id":"rcv","trial_no":1}`。
   pins の 2 欄（manifest・case）は暫定の対応で、「P0-2 identity 照合」節の未確定 U2。close 用に `pins` の中身だけを
   `$W/pins.json` に写す（`StudyPins`、`deny_unknown_fields`）。
5. 承認記録を作る（`$W/approval.json`。人の決定 d-smoke の回答を写す。値は spec と完全一致）:

   ```json
   {"schema":"celeris.recovery-smoke-approval/1","source":"task:01M5TCGEVJF1QW03CDWPJRRKZ5/decision:d-smoke",
    "cluster":"sirius","queue":"gen","account":"NBBG","select":"1:ncpus=24:mem=124gb:ngpus=1","place":"exclhost",
    "walltime_secs":300,"max_jobs":4,"valid_until":"2026-11-12T15:00:00Z","measurement_id":"<MID>"}
   ```

   `valid_until` は d-smoke の期限 2026-11-13 を JST の 0 時（UTC 前日 15:00）で切る。期限・件数を広げない。
6. cluster の測定 dir に payload を置き、hash を照合する:

   ```sh
   ssh -o BatchMode=yes sirius mkdir -p "$RDIR/release"
   scp -o BatchMode=yes scripts/campaign/recovery-smoke/job.sh scripts/campaign/recovery-smoke/input.bin "sirius:$RDIR/"
   ssh -o BatchMode=yes sirius sha256sum "$RDIR/job.sh" "$RDIR/input.bin" | tee "$W/remote.sha256"   # 2. と一致
   ```

7. trial を 1 本ずつ 4 回流す（順に baseline/normal → baseline/a2 → celeris/normal → celeris/a2。前の trial が戻り、
   `qstat -x -f <job_id>` が `job_state = F` になってから次）:

   ```sh
   for t in baseline:normal baseline:a2 celeris:normal celeris:a2; do
     DAGBENCH_BACKEND=pbs DAGBENCH_PBS_TRANSPORT=ssh DAGBENCH_PBS_SPEC="$W/spec.json" DAGBENCH_PBS_STUDY="$W/study.json" \
     DAGBENCH_PBS_APPROVAL="$W/approval.json" DAGBENCH_ARM=${t%%:*} DAGBENCH_RECOVERY=${t##*:} \
     DAGBENCH_SCENARIO=rcv-1 DAGBENCH_POINT=camp-1 DAGBENCH_OUT="$DAGBENCH_OUT" \
       cargo test -p celeris-campaign --test dagbench_harness -- --exact run_from_env --ignored --nocapture \
       > "$W/trial-${t%%:*}-${t##*:}.log" 2>&1 || { echo "stop: $t"; break; }
   done
   ```

   `DAGBENCH_PBS_STATE` は与えない（ssh では Err）。各 trial の stdout 末尾の JSON（`job_id`・`job_ref`・`saved_wait`・
   `controllers`・`a2`）が機械結果。1 本でも落ちたら残りを流さない（「上限と停止条件」）。
8. 測定を閉じる（1 回だけ。2 回目は `run.json` が既にあるので Err で何も書かない）:

   ```sh
   DAGBENCH_CLOSE_RAW="$DAGBENCH_OUT" DAGBENCH_CLOSE_PINS="$W/pins.json" \
     cargo test -p celeris-campaign --test dagbench_harness -- --exact close_from_env --ignored --nocapture \
     > "$W/close.log" 2>&1
   ```

   validate が落ちれば Err（`run.json` は残り、その測定は成功に数えない）。

## 観測・回収・再集計

- 停止中・終了後の観測（読み取りだけ。trial ごと）: runner の observer が `qstat -x -f <job_id>` を poll し、原票を
  `$DAGBENCH_OUT/scheduler/`・`scheduler-events.jsonl` に残す。人が確かめるときも同じ照会だけを使う:
  `ssh -o BatchMode=yes sirius qstat -x -f <job_id> > "$W/qstat-<job_id>.txt"`（`Exit_status`・`run_count`・
  `Variable_List` の `CELERIS_TRIAL_KEY`・`CELERIS_SUBMIT_INTENT` を見る。`run_count` が 1 を超えれば rerun で、成功に数えない）。
- 回収: controller が `SshSite::fetch_outputs` で `cat -- $RDIR/out/<job_id>/out.bin` を trial dir の `attempt-1/out.bin` へ写す。
  呼び出しは trial dir の `site-calls.jsonl` に 1 行ずつ残り、`raw::validate` が `out.bin` の sha256 と行の `stdout_sha256` を照合する。
- raw の sha256 一覧（閉じた後。raw dir には書かない）:

  ```sh
  (cd "$DAGBENCH_OUT" && find . -type f | LC_ALL=C sort | xargs sha256sum) > "$W/raw.sha256"
  ```

- 独立 grade の確認: 各 trial の `grade.json`（`celeris.recovery-grade/1`）の `output_sha256` と `passed`、および
  手元での再計算 `sha256sum "$DAGBENCH_OUT"/trials/*/attempt-1/out.bin` が `expected.sha256` と一致することを見る。
  qstat の `Exit_status=0` だけでは成功にしない（原票・identity・成果物 hash・期待出力・grade が全て要る。ADR D3）。
- 再集計（raw から指標を作り直す。出力は新しい dir）:

  ```sh
  DAGBENCH_REGEN_RAW="$DAGBENCH_OUT" DAGBENCH_REGEN_OUT="$W/regen" \
    cargo test -p celeris-campaign --test dagbench_harness -- --exact regen_from_env --ignored --nocapture \
    > "$W/regen.log" 2>&1
  ```

  受入で見るもの: trial 4 本それぞれ物理 qsub 1・消費 1（重複投入・重複消費 0）、A2 は停止中の同じ job の原票と
  再開後の同じ job ID の回収（再投入なし）、grade 合格、`regen` の結果が trial の stdout・`run.json` の `real_jobs` と一致、
  2 回目の `regen_from_env`（別の新しい out）でも同じ値。

## 認可の出典

| 出典 | 本件での扱い |
|---|---|
| 親進捗 [承認表](../../agent-docs/progress/2026-10-25-ieee-recovery-scheduler.md) の「このrunの永続認可」（software-engineering → cluster-hpc） | 部門間の連携だけ。job 数・予算の承認ではない |
| 同表の KB `sirius-job-submission.md`・`sirius-experiment-pitfalls.md`（2026-09-30 pbs-budget: NBBG・gen） | queue/account の選定根拠。数値の残枠・本件の利用量の承認ではない |
| 同表の short-ML 運用（debug/NBBG、上限 2 本、使い切り） | 本件へ転用しない |
| 同表の main-4 `final.json` の `approved_trials=2`・human-op runbook | 主比較 A0/B0 の宣言値・歴史的記述。本件の承認に使わない |
| ADR D2 の「合計最大 4 job・1/3 host-hour」 | 技術上限であって承認済み枠ではない |
| **人の決定 d-smoke**（この task の計画、needed_before live-smoke） | 本件の唯一の承認。`approve` の回答があるときだけ 5. の承認記録を作る。**提出時点で未回答 = 未承認** |

承認記録の `source` には決定の参照（`task:01M5TCGEVJF1QW03CDWPJRRKZ5/decision:d-smoke`）を書き、中身の解釈はしない。
d-smoke の枠（Sirius/gen/NBBG、上の資源、4 job 以下、同時 1、attempt 1、追加投入・qdel なし、期限 2026-11-13）を
超える値は承認記録に書かない。

## P0-2 identity 照合

読んだもの（2026-10-25、読み取りだけ）: `celerisctl show 01M5SRJMB1CSGBYT028X0B67N2`（exit 0、status Blocked）と
`/local/celeris/data/workspaces/01M5SRJMB1CSGBYT028X0B67N2/artifacts/main4-ready/` の `pin-reconcile.json`
（pin-audit、campaign_id `mlsys-main-4-v2`、`measurement_ready=false`）、`consistency.json`（grade-integrate、
`consistent=true`・`mismatches=[]`、ただし旧 v2 manifest の nollm out）、`freeze-report.json`（final-freeze、
campaign_id = measurement_id = `mlsys-main-4-v3`、`measurement_ready=false`、`paid_calls_made=0`、`check_main_pins` exit 1）。

ADR D3 の対応表の各行:

| D3 の行 | P0-2 の記録 | 復旧 smoke の記録 | 結果 |
|---|---|---|---|
| 研究資産 | `freeze-report.bindings` に campaign_id・manifest（campaign_sha256・input_manifest）・lock・case・campaign binary・daemon 候補 binary の完全 sha256 と research source。RELEASE は未 pin（`blocked: release`） | `run.json` に spec 由来の payload・input・grader・env の完全 sha256 と `StudyPins`（campaign_id・manifest・case・source・binary） | 形式は一致（`identity_format.p0_3_alignment` も同じ規則）。値は別 dataset で共有しない。P0-2 の RELEASE 未 pin は smoke に影響しない（smoke は daemon を使わない） |
| collector identity | `pin-reconcile.p0_3.align`: `campaign_sha256, config, problem_id, rep, trial_no` | `Study::collector`: campaign_sha256 = pins.manifest_sha256、config = `config_of(arm)`、problem_id = `rcv-<condition>`、rep・trial_no | 一致 |
| harness marker | `campaign_id, manifest_sha256, arm, case, rep, account_id` | qsub `-v` の `CELERIS_TRIAL_KEY`（trial_key）と `CELERIS_SUBMIT_INTENT`。LLM を使わないので `account_id` を持たない | 部分一致。U1 |
| 復旧比較 | `identity_format.rule`: measurement_id = campaign_id、旧 `mlsys-main-4-v2*` と混ぜない | 新しい `measurement_id`（`rcv-smoke-…`）、arm・scenario/point/seed/condition、controller epoch | 一致。smoke の MID・campaign_id は `mlsys-main-4-*` と重ならない値にする |
| 結合 | 主比較は A0=Config A・B0=Config B、主比較の campaign_id/out | baseline → Config A、celeris → Config B（`config_of`）。主比較の campaign_id・out は使わない | 一致。ただし recovery 専用 manifest は未作成。U2 |
| 外部操作 | P0-2 の通しは stub harness で外部 job なし（`consistency.fixture`） | `job_ref = pbs:sirius:<完全な PBS ID>`、logical_job_id・workload_attempt・submit_intent_id・wait_id。PID を job ID にしない | 照合対象なし（P0-2 側に実 job が無い）。U3 |

分母の分離: P0-2 の分母は manifest の計画セル（`consistency.denominator`）、smoke の分母は 4 trial。混ぜない。
P0-2 の `consistency.json` は旧 manifest の out で取ったもので、final-freeze の新 revision での再照合は P0-2 側の残件
（`freeze-report.blocked` の `consistency-rerun`）。

未確定項目:

- U1: harness marker の `account_id` は LLM を使わない smoke では意味を持たない。recovery 側で null と理由を残すか、
  marker の対応から外すかは未決（主比較の marker と結合するときに決める）。
- U2: ADR D3 の「recovery 専用 manifest」が未作成。smoke では暫定で `manifest_sha256` = 埋めた spec.json の sha256、
  `case_sha256` = `expected.sha256` file の sha256 とする（実行手順 4.）。本測定の前に正式な manifest で置き換える。
- U3: P0-2 の最終 freeze が未了（`measurement_ready=false`。blocked は release・a-account-login・claude-oauth-catalog・
  sirius-controlmaster・real-preflight・shared-ledger・repo-src-submodule-commits・consistency-rerun）。
  外部操作の行は P0-2 の実 pilot が job を持つまで照合できない。
- U4: P0-2 の blocked `sirius-controlmaster`（cap-probe 用の ControlMaster が dangling）と本件の Sirius 接続不足は同じ根。
  人が TOTP で張り直すまで両方止まる。

## 判定

**`hpc_ready = false`**（2026-10-26、[live-smoke 進捗](../../agent-docs/progress/2026-10-25-ieee-recovery-scheduler/live-smoke.md)の値と同じ）。

- 接続（実装）: 完了。ssh 承認 gate・`SshSite`・`close_from_env`・cluster payload、`RECOVERY_SMOKE_DIR` の qsub `-v` は
  実装と stub 試験まで済んだ。d-smoke は承認、BatchMode 接続もあった（以前の不足 1〜3 は解消）。
- 実機受入: **未完了**。measurement_id `rcv-smoke-20261026-1` の 1 本目（baseline/normal）の qsub を gen が
  `'place=exclhost' is allowed only for node-exclusive placement`（exit 32）で拒否した。受理 job 0 本、validated_completed 0。
  停止条件どおり残り 3 trial は流していない。同一 job 継続・回収・二重投入/消費 0・独立 grade・raw 再集計一致は、
  どれも実機では確かめていない（偽 transport の決定的試験でだけ確認済み）。この測定 ID は再利用しない（ledger に sent 1 件）。
- 残る人の判断:
  1. 資源の配置（`place`）を決め直す（推奨 `place=excl`、代案 `select=4:…`+`place=exclhost`）。d-smoke の後継の決定として回答し、
     ADR D2・`spec.template.json`・上の資源表を更新してから新しい measurement_id で再実行する。
  2. 承認枠と PBS 課金単位（1/3 host-hour）の換算を照合する。
  3. 再実行の時期。Sirius の保守と claude_max_lab の login 復旧を待つ（それまで Sirius へ接続しない）。
- 残る実装の不足: unknown submit の marker 検索が未実装（`find_by_marker` は `Unsupported`）。qsub の明示的拒否を
  `rejected` と分ける分類は提案のまま。
