---
title: IEEE recovery scheduler — 進捗の取りまとめと提出時 HEAD の必須検査（close-progress）
tasks: [01M5SSBCP75CTVF2SHD0QQ9ZGG]
status: done
updated: 2026-10-25
completed: 2026-10-25
---
# 進捗の取りまとめと提出時 HEAD の必須検査（close-progress）

完了日: 2026-10-25。基準は `git merge-base HEAD main`（WU は `$CELERIS_WU_BASE`）。
この unit はコードを変えていない。変えたのは [ADR](../../adr/2026-10-25-recovery-scheduler-backend.md) の実装付記（I1・I2・I3・I5 の訂正と I13）と本ファイルだけ。
実 scheduler・ssh・qsub・外部 network・LLM には一度も通信していない（全 unit で偽 transport `FakePbs` のみ）。

## 各 unit の成果

| unit | commit（件名） | 成果 | 詳細 |
|---|---|---|---|
| contract | `phase 1: fix PBS recovery backend contract and asset inventory`、`phase 1: record recovery contract validation` | ADR D1–D5（PBS/Sirius gen/NBBG、unknown submit の再送禁止、raw 境界、A2 停止境界） | ADR 本文 |
| adr-impl | `phase 1: add recovery scheduler implementation appendix to ADR` | 実装付記 I1–I9（module・trait・台帳・停止規則・fixture 表） | [adr-impl.md](adr-impl.md) |
| pbs-transport | `phase 1: add PBS transport with bounded ssh calls and reply classification`、`docs: record pbs-transport checks` | `PbsTransport`・`SshPbs`（全体 timeout、marker 検索は Unsupported）・応答分類・`QstatView`・`FakePbs` | [pbs-transport.md](pbs-transport.md) |
| submit-intent | `phase 1: add submit intent ledger with marker reconciliation`、`docs: record submit-intent checks` | 投入前 intent 台帳、決定的 `submit_intent_id`、file lock、unknown_submit の marker 照合・再照合・停止 | [submit-intent.md](submit-intent.md) |
| ext-pbs | `phase 1: add PBS ExtBackend with shared recovery spec`、`docs: record ext-pbs checks` | `PbsExt`（`ExtBackend`）、両 arm 共通 `RecoverySpec` と pin、`Lookup/SubmitError::Unresolved`、消費台帳 | [ext-pbs.md](ext-pbs.md) |
| raw-collect | `phase 1: add scheduler raw writer/validator with collector binding`、`docs: record raw-collect checks` | `celeris.dagbench-scheduler-raw/1` writer・validator、trial への collector 結合、read-only 再収集 | [raw-collect.md](raw-collect.md) |
| wire-entry | `phase 1: wire opt-in PBS recovery entry with offline e2e`、`docs: record wire-entry checks` | opt-in 入口（既定 ext-sim、`--backend pbs` は CLI で拒否）、wrapper 拒否、A2 barrier、offline e2e | [wire-entry.md](wire-entry.md) |

各段の統合 commit は `integrate wu/<unit> (phase <段>)` の件名で入っている。

## task 受け入れ条件との対応（offline 試験）

- 正常・adapter/replay: `dagbench_pbs_e2e`（両 arm × normal/A2、物理 qsub 4・消費 4、全 trial `validated_completed`）、`dagbench_pbs_backend`（既存 `Baseline` を PbsExt に差し替え全保存点で停止→再開、qsub 2・消費 2）。
- 応答喪失後の既存 job 照合: `dagbench_pbs_intent`（qsub timeout → marker 1 候補一致で reconciled、qsub 1）。
- 照合不能・複数一致: `dagbench_pbs_intent`（0 候補の deadline 停止、複数候補、不一致 10 種、Unsupported）、`dagbench_pbs_e2e`（unknown submit で両 arm 停止、`rejected_before_send`/not_submitted にしない、消費 0）。
- 原票欠落・hash 改変・job ID/trial 不一致・collector 障害・再収集: `dagbench_pbs_raw`（validator の拒否、欠測 5 種は null+理由、再収集で qsub 1 のまま・消費台帳不変）。
- 物理投入/消費回数: 全 fixture で `FakeCounts` の qsub 数・`consumed.jsonl` の行数を assert。
- 既存契約: 既存 `dagbench_*` 試験は無変更で合格（ext-pbs unit で 15 file 106 passed / 3 skipped）。

提出時の HEAD で実行: `cargo test -p celeris-campaign --test dagbench_pbs_transport --test dagbench_pbs_intent --test dagbench_pbs_backend --test dagbench_pbs_raw --test dagbench_pbs_e2e`
→ exit 0、8 + 14 + 7 + 6 + 5 = **40 passed**、0 failed。

## 実投入の状態

**実 scheduler への投入・照会は未実施。** 次の確認が済むまで行わない。

- Sirius の認証（ssh 鍵・TOTP・ControlMaster の有無）。
- recovery 用の承認残枠（node 時間・job 数。spec の `max_jobs` は 4）。
- marker 履歴検索の可否（`qselect -x` や `qstat -x -f -u` で終了済み job の `Variable_List` が見えるか、履歴保持期間）。
  未確認のため `SshPbs::find_by_marker` は `Unsupported` を返し、unknown submit は照合不能で停止する。
- PBS server 名の先頭 label が cluster 名（`sirius`）と一致するか（不一致なら照合が全部停止する。安全側）。

## 人が実行する実接続前の確認手順（read-only。qsub は打たない）

1. ControlMaster を張る（Pegasus と同じ運用。TOTP は人が入れる）: `ssh -o ControlMaster=yes -o ControlPersist=4h -fN sirius`
   確認: `ssh -o BatchMode=yes -o ControlMaster=no sirius true; echo $?` → 0。
2. 投入先と会計: `ssh sirius qstat -Q gen` で queue が enabled/started、`ssh sirius 'qstat -B -f | grep -E "server_host|job_history"'`
   で server 名と `job_history_enable`・`job_history_duration` を記録する。
3. marker 履歴検索: 既に終わった自分の job 1 本について `ssh sirius 'qselect -x -u $USER'` と
   `ssh sirius 'qstat -x -f <job id> | grep -E "Variable_List|Job_Owner|Rerunable|Submit_arguments"'` を実行し、
   `-v` で渡した変数が終了後も見えるか、`qselect -x -v` 系の絞り込みが使えるかを記録する。
4. 承認残枠: recovery 比較 4 job（`1:ncpus=24:mem=124gb:ngpus=1`、exclhost、walltime 300 秒）の消費を人が承認する。
5. 1〜4 の結果を本 task の進捗か ADR D4.4 に記録し、`find_by_marker` の実 command を決める（次の task）。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）

最後の同期は本 WU の base（wire-entry 統合後）からの作業で、main の取り込みはしていない。

- `bash scripts/dev/test-affected.sh` → 下の「検査結果」
- `cargo clippy --workspace -- -D warnings` → 下の「検査結果」
- `cargo fmt --all -- --check` → 下の「検査結果」
- 文書検査: `sh scripts/dev/progress-index.sh --check`、`python3 scripts/dev/check-architecture-map.py`、`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv`

### 検査結果

（記録 commit で追記）

## 未解決

- marker 検索の実 command（D4.4）、PBS server 名の照合規則、人の解決を表す `human_resolution` 行（I13）は実接続前に決める。
- A2 の controller 停止は in-process（incarnation の終了）。別 process の PID・starttime 確認と SIGKILL は実接続の段。
- CLI からの実 PBS 実行（`SshPbs` と実 Site の barrier・成果物同期）は未実装（`--backend pbs` は拒否）。
- `celeris.dagbench-scheduler-metrics/1`（実時間の集計）は未実装。
- `process-events.jsonl`・`durable-wait.json` の raw 同梱は未実装（raw-collect の未解決のまま）。
- accepted 後の qstat 原票不一致は台帳に停止行を残さない（毎回 `Unresolved` で止まるが、台帳だけ読むと accepted に見える）。

## 提案

- accepted/reconciled 後の原票不一致も台帳に `unresolved_stop` 行（理由付き）を追記し、台帳単独で停止が読めるようにする。
- 実接続の次 task は「read-only 確認（上の手順 1〜3）の結果で `find_by_marker` を実装 → 人の承認 → 1 trial の normal だけ投入」の順に段を切る。
