---
title: "健全性v2 R4 cm-owners: Dispatcher field の所有候補"
tasks: [01M74VAMWFCC4JGZY4ENA92HJJ]
status: completed
updated: 2026-11-11
---
# Dispatcher field の所有候補

完了日: 2026-11-11。全116 field を8候補に割り当て、異なる owner の module が書く12 field を競合として記録した。製品コードの変更はない。所有境界の決定ではなく、R4 の設計に使う測定結果である。

## 実装前 brief

1. 根本原因: cm-access の測定には所有規則が無く、書き手が複数ある状態の責務を比較できない。
2. 責務 owner: この測定の change-map-measure.py。製品の Dispatcher の境界はここでは変えない。
3. 再利用先: cm-access の fields・readers・writers・cfg_test・co_change と、architecture-map / B0 capability-map。
4. 選択肢: module だけの分類では設定注入・実行状態・混在 module を区別できないため、明示した module 表と理由付き field 指定を採用。共変更の最大数だけで所有を決めない。
5. 新しい状態・分岐・fallback・port: 製品には無し。測定には owner 規則と未分類を拒む検証を追加する。
6. 撤去対象: 無し。既存の読み書き測定の定義は維持する。
7. 不変条件: 全 field の候補が8種類のいずれかで、cfg(test) は test-hooks。競合を候補選択で消さず、不明なら拒否する。正例の全件照合と規則欠落の反例で確認する。

## 件数

| owner | field 数 |
|---|---:|
| execution-coordination | 31 |
| run-supervisor | 7 |
| routing | 14 |
| project-orchestration | 2 |
| host-maintenance | 9 |
| cluster-runtime | 16 |
| composition-root | 22 |
| test-hooks | 15 |
| 合計 | 116 |

`test-hooks` は cfg(test) の10 field と、製品で既定実装を使う試験用差し替え5 field（knowledge_probe・now_unix_fn・cluster_liveness_probe・cluster_ssh_command_override・eval_version_probe）。後者は daemon の setter 呼び元がなく、宣言と setter の試験用注記も確認した。

## 競合する field

owner_conflict は測定された writers の module owner が2種類以上なら true。constructor は cm-access の測定対象外、setter は除外しない。readers だけの owner の違いでは true にしない。field 別指定をしてもこの判定は変えない。
以下の module 名は crates/task-dispatch/src/ からの相対 path（.rs を省略）。

| field | 候補 owner | 書き手 module → module owner |
|---|---|---|
| accounts_scan_cache | routing | dispatcher → execution-coordination; dispatcher/route_select → routing; dispatcher/snapshot → execution-coordination |
| awaiting_human | execution-coordination | dispatcher/leases → run-supervisor; dispatcher/review_spawn → execution-coordination |
| checking | execution-coordination | dispatcher → execution-coordination; dispatcher/leases → run-supervisor; dispatcher/work_units → execution-coordination |
| cos_chat_launch | project-orchestration | dispatcher/cos_chat/launch → run-supervisor; dispatcher/cos_chat/triage → project-orchestration; dispatcher/project_retention → project-orchestration; dispatcher/project_wake → project-orchestration |
| infra_backoff | run-supervisor | dispatcher/dispatch_run → execution-coordination; dispatcher/leases → run-supervisor; dispatcher/worker_finish → run-supervisor |
| integrating | execution-coordination | dispatcher/leases → run-supervisor; dispatcher/phase_integration → execution-coordination |
| pending_subjects | execution-coordination | dispatcher/leases → run-supervisor; dispatcher/phase_integration → execution-coordination; dispatcher/review_spawn → execution-coordination; dispatcher/review_verdict → execution-coordination; dispatcher/worker_finish → run-supervisor |
| reviewing | run-supervisor | dispatcher/leases → run-supervisor; dispatcher/ownerless_runs → run-supervisor; dispatcher/review_spawn → execution-coordination; dispatcher/review_verdict → execution-coordination |
| running | run-supervisor | dispatcher/dispatch_run → execution-coordination; dispatcher/leases → run-supervisor |
| scratch | host-maintenance | dispatcher → execution-coordination; dispatcher/housekeeping → host-maintenance |
| stagnation | project-orchestration | dispatcher → execution-coordination; dispatcher/project_wake → project-orchestration; dispatcher/tree_units → execution-coordination |
| task_workspaces | execution-coordination | dispatcher/dispatch_run → execution-coordination; dispatcher/housekeeping → host-maintenance |

## 規則と根拠

[owners.json](owners.json) に48 module の候補・各 module の理由・出典と、60 field の個別指定・理由を置いた。出典は docs/architecture-map.md の task-dispatch の行と agent-docs/progress/2026-11-07-health-v2-b0/capability-map.json。特に project-controller、orphan-takeover、continuation-session-resume、workunit-scheduling-stage-integration の役割を参照した。cluster は architecture-map の SSH / durable job 行と ADR-0018 / ADR-0090 が根拠。

優先順は cfg(test) → field 個別指定 → writers の単一 owner → writers が空なら readers の単一 owner。それでも決まらない場合は exit 非0。未登録 module、無効な owner、空の理由、削除済み field の古い個別規則も拒否する。未知 field を無条件で既定 owner に入れる分岐はない。

各 owner_reason には個別指定または単一 owner の根拠、全 readers/writers の module と規則、co_change の上位組ごとの回数（該当組なしは0件）を記録した。競合時は書き手 module とその owner を明記する。共変更は724 commit の file 単位の相関で、field 自体の変更回数でも因果関係でもない。上位5組という cm-access の制限を維持する。

### constructor / setter の確認

`rg -n 'set_|Dispatcher::new' crates/celeris/src/daemon` と測定対象の writer の関数本体を照合した。

- daemon/bootstrap.rs:587 で store と DispatchConfig を注入する。config は apply_dispatch_config（dispatcher.rs:1847 付近）による reload も設定入力とみなす。実効 max_concurrency は tick が解決するため execution-coordination と分けた。
- bootstrap.rs:603–673 と admin.rs:241–244,291–294 は route registry・adapter・credential gate・routing mode・target sweep を設定する。target_sweep と target_sweep_scope は設定だけだが、disk_watch は tick_disk_watch でも可変 runner を更新するので host-maintenance。
- cluster port は bootstrap.rs:654–673、clusters.rs:185–191 が注入する。cluster_conn・cluster_connected・connect_pending_clusters は接続の実行状態なので cluster-runtime。
- credential_resumer は bootstrap.rs:617、routing_context_registry は services.rs:301 と run.rs:140、publisher は api.rs:517、eligible は run.rs:228、variant_observation は bootstrap.rs:712 からの注入。orphan_takeover は run.rs:208 で instance と pid_alive を注入し、leases.rs:742 は読むだけなので composition-root。
- dispatch_routing は Dispatcher::new の既定設定を dispatch_run.rs:1435 で読む。外部 setter が無くても constructor に閉じた設定として composition-root。
- accepting_new_work は tick_loop.rs:133,141 が走行中にも切り替える受付状態。classification_wait は tick_loop.rs:621、login_pending_accounts と connect_pending_clusters も tick_loop が更新するため、setter 名だけで composition-root にしない。
- container_probe は bootstrap.rs:677 の detect_container_runtime で実環境を観測する。account_books・quota_activity・quota_calibration・classification_board・tunnel_probe_state は共有内部状態の変更があるため、writers が空でも設定とはみなさない。

### 限界

字句測定の限界は change-map.json の method.limits と cm-access.md のとおり。lock 内部・別名・範囲外の型の可変 method・health_review/ の mounted module は writers に出ない場合がある。したがって owner_conflict=false は競合不存在の保証ではない。health_inventory は health_review/inventory.rs の InventoryRuns も読み、保守状態として個別指定したが、測定の探索範囲は変えていない。dispatcher.rs と snapshot.rs は役割の混在があるので、その module owner は測定用の代表候補である。

## 再生成と検証

基準は `$CELERIS_WU_BASE`。検査は提出時の HEAD で実行。全体 nextest・一括 doc test・browser/e2e は走らせない。

- 条件0: `python3 agent-docs/progress/2026-11-11-health-v2-r4/change-map-measure.py --out agent-docs/progress/2026-11-11-health-v2-r4/change-map.json` → exit 0。116件が有効な owner・非空の理由・bool の競合を持つ。宣言 field と集合一致、cfg(test) 10件は全て test-hooks。別出力への再生成は byte 一致。
- 条件1: `$TMPDIR` の小さな script / owners.json の写しから cos_chat_launch の個別指定を消し、同じ CLI を実行 → exit 1、`unresolved owner for cos_chat_launch: project-orchestration, run-supervisor`。出力 file は作らない。未知 module・参照無し field・無効 owner の反例も ValueError で拒否した。
- 条件2: JSON から本書の owner 件数表と競合一覧を照合 → exit 0、116件 / 12競合で一致。
- `bash scripts/dev/test-affected.sh` → exit 0。文書・測定のみの変更で対象 crate は0、health 検査を実行。
- `cargo clippy --workspace -- -D warnings` → exit 0（依存の deno_core に deprecated warning 1件。workspace の検査は合格）。
- `cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/progress-index.sh --check`、`git diff --check`、範囲検査 → exit 0。変更は指定ディレクトリの4 file のみ。crates/・scripts/ の差分は0。

commit 件名: `phase 1: assign Dispatcher field owner candidates with evidence`

## 未解決事項

製品の owner 境界の確定はこの WU の範囲外。測定規則による未分類 field は0件。

## 提案

R4 の設計時に、上の12競合と字句測定外の内部可変状態を併せて検討する。候補を採用するだけで共有状態の問題が解決したとは判断しない。
