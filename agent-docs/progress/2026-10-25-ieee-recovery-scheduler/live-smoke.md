---
title: IEEE recovery scheduler — Sirius 実機 smoke（live-smoke）
tasks: [01M5W80A67C0EN8WZX5Q6RWQ7T]
status: blocked
updated: 2026-10-26
---
# Sirius 実機 smoke（live-smoke）

基準は `git merge-base HEAD main`。手順は [実行票](../../../docs/experiments/recovery-scheduler-smoke.md)。

## 判定

**hpc_ready = false**（2026-10-26）。実機受入は未完了。

- 前提は揃った: 人の決定 d-smoke は approve（2026-10-26 回答）、`ssh -O check -o BatchMode=yes sirius` → exit 0
  （Master running）、I15 (4) の `RECOVERY_SMOKE_DIR` の qsub `-v` をこの task で実装した。
- 1 本目（baseline/normal）の qsub を PBS が受け付けなかった:
  `qsub: Job rejected: 'place=exclhost' is allowed only for node-exclusive placement.`（exit 32）。
  承認枠の資源（`select=1:ncpus=24:mem=124gb:ngpus=1`・`place=exclhost`）は gen では通らない
  （exclhost は物理ノード全体を取る要求にしか使えない。KB `environment/clusters/sirius-job-submission.md` の mcrp の記述と同じ挙動）。
- harness は qsub の非 0 を unknown submit に分類し、marker 検索（`Unsupported`）で照合できないため trial を止めた
  （`unresolved_stop`、再投入なし）。停止条件「queue が資源条件を拒否した」に当たるので、残り 3 trial は起動していない。
- 受理された job: **0 本**（PBS job ID なし）。`qstat -x -u rmaeda` の投入前後で新しい job は無い。qdel・他 job の取消・
  本番 daemon・SSH master の操作はしていない。validated_completed 0、同一 job 再開・再集計は未検証。
- close_from_env・regen_from_env は流していない（完了 trial が 0 で、閉じる対象の測定にならない。raw は書き換えない）。

## 実行記録

- measurement_id: `rcv-smoke-20261026-1`（新規。cluster 側 `/work/NBB/rmaeda/recovery-smoke/rcv-smoke-20261026-1`、
  手元 raw は run artifacts の `recovery-smoke/rcv-smoke-20261026-1/`、既存 raw と別）。
- 作業物（run artifacts の `recovery-smoke/run/`）: `spec.json`・`study.json`・`pins.json`・`approval.json`
  （`max_jobs=4`、`valid_until=2026-11-12T15:00:00Z`、source = d-smoke）、`pins.sha256`・`remote.sha256`
  （job.sh・input.bin の手元と cluster の sha256 一致）、`build.log`、`trial-baseline-normal.log`（exit 101）、
  `qstat-before.txt`・`qstat-after.txt`（qstat 原票）、`remote-tree.txt`、`raw.sha256`（raw 9 file の sha256 一覧）、`result.json`。
- raw の原票: `submit-intents.jsonl` の 4 行（intent_recorded → sent → unknown_submit（qsub の argv・stderr を含む）→
  unresolved_stop）、`trials/baseline-normal/` の `reject.json`・`control-1.stderr`・`replies-1.jsonl`・`state.json`。
- 独立 grade: 対象なし（`out.bin` が 1 本も無い）。

## 変更

- 「campaign: pass RECOVERY_SMOKE_DIR to the payload via qsub -v (ADR I15 (4))」— `QsubRequest::argv` の `-v` に
  `RECOVERY_SMOKE_DIR=dirname(script)` を足し、`FilePbs` の `Variable_List` も合わせた。root 直下の script は Err。
- 実行票の「判定」節を今回の結果に合わせた。

## 証拠（提出時の HEAD で実行）

- `cargo test -p celeris-campaign --test dagbench_harness harness_pbs_` → exit 0（20 passed）
- `bash scripts/dev/test-affected.sh` → exit 0（nextest 2709 passed、9 skipped。main 取り込み後）
- `cargo clippy --workspace -- -D warnings` → exit 0 / `cargo fmt --all -- --check` → exit 0
- `sh scripts/dev/progress-index.sh --check` → exit 0

## 未解決

- 資源の配置が gen で通らない。直すには spec の資源を変える必要があり、承認枠（d-smoke の place=exclhost）と
  ADR D2 の値も変わるので、人の決定が要る。今の測定 ID は使わない（ledger に sent 1 件が残る）。
- 課金単位（1/3 host-hour）の換算は引き続き未照合。marker 検索は `Unsupported` のまま。

## 人が行う手順（再実行の前）

1. 配置を選ぶ（下の提案）。決めた値を d-smoke の後継の決定として回答する。
2. その値で ADR D2・`scripts/campaign/recovery-smoke/spec.template.json`・実行票の資源表を更新する task を通す。
3. 新しい measurement_id（例 `rcv-smoke-<YYYYMMDD>-2`）で live-smoke をやり直す。cluster 側の
   `/work/NBB/rmaeda/recovery-smoke/rcv-smoke-20261026-1` は payload だけで、消す必要はない。

## 提案

- 推奨: `place=excl`（1 vnode = 24 cpu を排他。同じ gen の過去 job が `1:ncpus=24:mem=124gb:ngpus=1` で走っている）。
  費用は物理ノードの 1/4 で、承認枠の 1/3 host-hour に収まる。
- 代案: `select=4:ncpus=24:mem=124gb:ngpus=1` + `place=exclhost`（物理ノード全体）。exclhost の意図は保つが
  受理されるかは未確認で、1 job あたり 4 vnode を確保する。
- qsub の明示的な拒否（非 0 で stderr に `Job rejected`）は未投入が確実なので、unknown submit と分けて
  `rejected` と分類すると停止理由が読みやすくなる（D4 の分類の見直し。今回は変えていない）。
