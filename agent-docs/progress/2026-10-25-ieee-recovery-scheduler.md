---
title: IEEE recovery scheduler — 現行資産・承認枠とPBS接続契約
tasks: [01M5SRQ6DAEA8RMHFENY699T5B]
status: done
updated: 2026-10-25
completed: 2026-10-25
---
# 現行資産・承認枠とPBS接続契約

完了日: 2026-10-25（contract 段のみ）。実装・実HPC測定は未実施、測定開始不可。
基準は `git merge-base HEAD main`。本taskはP0-1/P0-2全体を依存にせず、取得済み情報で契約を固定した。
決定は[ADR](../adr/2026-10-25-recovery-scheduler-backend.md)。既存5成果、コード、旧raw、bundle、本番設定に変更なし。

## 現行 main・ローカル release・研究 bundle の照合

2026-10-25 の read-only 調査。`git diff --quiet main HEAD` は着手時 exit 0、
`git merge-base --is-ancestor main HEAD` も exit 0。
main/着手時HEADの件名は「progress: mcp-read-only-db-and-api の検査結果を記録」。
`readlink -f /local/celeris/state/current` と、その配下の manifest.json/promoted.json を照合した。
ローカル昇格版は b7b9c3b0d0ff、schema_version 93、build 2026-10-23T21:16:50Z、
promoted 2026-10-23T21:18:26Z（stop-start）。release manifest の source と着手時 main は一致。
これはローカル配置情報であり、稼働 process の binary 照合や release/verify の再実行ではない。
古い GitHub main は今回の根拠に使わず、外部照会しなかった。

研究 bundle root:
`/local/celeris/data/workspaces/01M5KVHA4222TK6VEKW599SNMA/artifacts/mlsys-main-4`。
freeze-status.json の現物を読むと 45 frozen / 14 pending。
revision は `revisions/judged-scope-v2`、campaign_id は `mlsys-main-4-v2`、campaign SHA256 は
`c678b60a999f95ed8f34d792d3ec3bb6ff2a4f2344885e0951adaf92d9a8048e`。
RELEASE/A実体/測定binary・env-lock/cap-env・設定・K2 repo-src・preflight・project・shared-ledger は依然 pending。
`nollm/summary.json` は fixture host/simulated executor と実 launcher 未接続を明記しており、実測証明に昇格させない。
機械向け照合記録は本run成果物の inventory.json（秘密値なし）。

## 既存5件の維持と再利用

| 成果/task | 現行の証拠と再利用点 |
|---|---|
| K4 / 01M5KVHA4222TK6VEKW599SNMA | [進捗](2026-10-23-k4-validity-grading.md)、bundle freeze-status/nollm。32セル中30 valid の独立再計算、入力pinを維持 |
| 成功契約・human_ops / 01M5KVHDWYTACKJKBNKWQXGX0M | [進捗](2026-10-23-success-contract-human-ops.md)、`crates/campaign/src/success.rs`。executor と grade の結合、canonical trial、欠測/null を維持 |
| A job ID/collector / 01M5KVHDY26YCNYGZKDTANGJX6 | [進捗](2026-10-23-a-job-id-collect-classes.md)、harness_exec/observe/ssh_qstat/celeris-qsub。受理記録→track→台帳→独立観測を再利用 |
| graded harness / 01M5KVHDZ6PGW45VNA5E6MGRPC | [既存記録](../../docs/progress/2026-10-23-graded-dagbench-harness.md)、dagbench_harness/graded/baseline。共通 grading pin・attempt 消費・validated_completed |
| durable A2 / 01M5KVHHCMVFY0ZDPKYVS09GXC | [進捗](2026-10-23-durable-wait-fault-harness.md)、`tests/common/dagbench_durable*.rs`、既存 k3-a2-v1/summary.json 現物。独立process/保存後停止、8条件・12grade の旧証跡を維持 |

コードの調査対象はADR D1表の全箇所。既存wrapperのコメントと実装に応答喪失照合が無いことを確認した。
ExtBackend の論理時計、SshQstat の既知ID照会、dispatcher の gone によるwait解放は、それだけで実測の成功根拠にならない。
契約固定のみなので既存反復を再実行せず、大規模refactorも行わなかった。

## 承認・認証・queue/account（秘密値は記録しない）

| 出典 | 有効範囲・上限・有効期間 | 本件への判断 |
|---|---|---|
| このrunの永続認可 | software-engineering → cluster-hpc の部門間連携。今後ずっと、資源上限の記載なし | 調査・接続準備は可能。ジョブ数・予算の承認ではない |
| KB `environment/clusters/sirius-job-submission.md`、`projects/benchfs/sirius-experiment-pitfalls.md` | 2026-09-30 人の pbs-budget 決定: NBBG、利用queue gen。終了期限の記載なし。数値残枠/本件利用量の承認なし | Sirius/PBS/gen/NBBG を選定。mcrp/NBBG は不可。過去の1152 ncpus制限はqueue観測値であり承認残枠ではない |
| [short-ML 運用](../../docs/ops/short-ml-campaign.md) | 2026-10-19 `sirius-full-vnode-case=approve-full-vnode`、debug/NBBG、1 vnode、600秒、硬い上限2本。用途限定、上限使い切り。期限の記載なし | 残枠0。本件へ転用しない。short-ML正式化もしない |
| main-4 revision の `pinned/main/final.json` | `approved_trials=2`、shared_ledger は `/local/celeris/data/measure/main-4/global-ledger`。期限情報なし。freeze-status では台帳初期化/承認 pending | 宣言値を現在のPBS承認とみなさない。main-4 root は未配置（measure は main/main-2/main-3）。共有過去消費を含めた残枠は未確認 |
| [human-op runbook](../../docs/ops/eval-human-ops-runbook.md) | 12試行以内の歴史的記述、本文自身が測定自動開始・予算増額の承認を含まないと明記 | 本件4jobの承認には使わない。有効期間/残枠不明 |

KB は `celerisctl knowledge search sirius` と `knowledge get <上記path>` で取得した。
`--scope kb:environment/clusters` では検索0件だったため、scopeを外して正本を取得。
`ssh -O check -o BatchMode=yes sirius` は **exit 255、Control socket missing**。
これはローカル多重化socket確認のみで、外部ネットワーク照会/認証試験ではない。
このrunが利用可能な認証接続はない。鍵/OTP/秘密ファイルは読まず、対話認証を開始しなかった。
ADR-0018の接続借用とADR-0032による後続認証機能を区別し、本runから本番の接続設定を変更しない。

不足は (1) 運用者が既存のクラスタ接続手順でSirius接続を回復すること、(2) recovery用途の有効な承認出典・期限・
4job以下/資源上限・既消費・残枠を確認すること、(3) 接続後にgen/NBBGの現行制約とmarker履歴検索の可否を確認すること。
ADRの4job・1/3 host-hourは技術上限であって承認済み枠ではない。unknown submit も消費/予約を解消するまで枠を戻さない。
本段はread-only契約固定が目的なので、人の回答待ちにはしない。実投入を伴う後続段の開始条件として引き継ぐ。

## P0-2との照合状況と最終照合項目

`celerisctl show 01M5SRJMB1CSGBYT028X0B67N2` は exit 0。
取得時は Blocked（awaiting_plan_approval）、d-release/d-model がOpen。
同task成果物ディレクトリの execution-plan.json と result.json は存在するが、最終freeze/実測identityはまだない。
result.json はP0-3と「測定identity・記録形式だけを合わせる」と明記。
予定証跡は同taskの `main4-ready/{pin-reconcile,nollm-pass,consistency,freeze-report}.json`。
今回は現行 observe::trial_identity、TrialKey、TrialMarker、human-opの正本へ対応付けを固定した（ADR D3）。

最終資産を取得した時の照合項目:

- P0-1採用資産を含むmanifest/case/input/graderの完全hash、source/release/binaryの実体。
- campaign_id・measurement_id・trial_key、case/problem、arm/config、rep/repetition、trial_noの一意対応。
- observed-jobs/submit-audit/grade/human-op各schema、raw hash・job_ref、canonical trialと分母、共有作業の一度計上。
- P0-2主比較はA0=単体Claude Code/B0=Celeris。こちらのbaseline/celeris無LLM制御比較とdataset/outを分ける。
- release/model決定を本taskで代決しない。recovery rawの実時計とmain-4の費用/人手記録を混ぜず、欠測はnull。

## 受入・必須検査

提出時の HEAD で実行（最後の同期後。基準は `git merge-base HEAD main`）。
以下は結果記録commit後にも同じコマンドで再確認する。ログは
`/local/celeris/data/workspaces/01M5SRQ6DAEA8RMHFENY699T5B/artifacts/` の final-*.log。

- 条件0（契約固定）: ADR D1–D5で再利用・PBS・両arm・identity/raw・unknown停止・所有権を固定。
  `git diff --quiet $(git merge-base HEAD main) -- crates/ scripts/ docs/` → exit 0、既存実装/成果への差分なし。
- 条件1（認可と照合）: `celerisctl knowledge get environment/clusters/sirius-job-submission.md`、
  `celerisctl show 01M5SRJMB1CSGBYT028X0B67N2` → exit 0。出典/上限/期限不明/枠不足を上表に記載。
  `ssh -O check -o BatchMode=yes sirius` → exit 255、借用接続なし（想定した不足の確認、検査不具合ではない）。
- 条件2: `test -s agent-docs/adr/2026-10-25-recovery-scheduler-backend.md && test -s agent-docs/progress/2026-10-25-ieee-recovery-scheduler.md` → exit 0。
- 条件3: `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` → exit 0、全てok（ADR 293 files）。
- `bash scripts/dev/test-affected.sh` → exit 0、docsのみでRust試験対象なし。
  health: source-size失敗0（既存debt警告3、理由付き例外1）、architecture-map 726パス、
  progress hash自己試験5項目、Python 38+1試験、doc-links自己試験が合格。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0、差分なし。
- `git diff --check` → exit 0。

全workspace試験、release/verify/promote、実browser、外部ネットワーク試験、LLM、本番操作は行わなかった。

## commit（件名）

- `phase 1: fix PBS recovery backend contract and asset inventory`
- `phase 1: record recovery contract validation`

## 未解決・本番への移行

接続実装、実schedulerでの同一job再開の検証、認証、承認残枠、P0-2最終freezeとの照合は後続段。
このtaskで本番移行・設定変更は不要。controller/observerは測定専用環境に限定する。

## 提案

後続段はADRの最小adapterとoffline fixtureから実装し、実投入直前に認証/承認出典と残枠を再確認する。
PBS履歴検索の可視性が不足すればunknown submitを停止する契約を維持し、再送で見かけの成功を作らない。
