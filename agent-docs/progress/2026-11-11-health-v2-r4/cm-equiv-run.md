---
title: R4 cm-equiv-run — run 終了・lease 更新・retry の等価性
tasks: [01M74VAMWFCC4JGZY4ENA92HJJ]
status: completed
updated: 2026-11-11
---
# run 終了・lease 更新・retry の等価性

完了日: 2026-11-11。コード変更無し。基準は `$CELERIS_WU_BASE`。

## 結論と測定範囲

3 件とも経路全体の verdict は `different`。run-close は 5 sites、lease-renew は実呼び出し全 5 sites、retry-backoff は 9 sites を記録した。非等価は共通部分が無いという意味ではない。task/WU の終了関数、lease 更新の store 操作、infra delay は既に共有されている。infra の上限判定には部分的な重複が残る。

提出時の HEAD の dispatcher.rs と dispatcher/ 配下を静的に比較した。行番号は `nl -ba` と `rg -n` で確認した。実行時間のベンチマークではない。store・task-ops・execution_scheduler の内部の原子性や計算式は今回の読み取り範囲外であり、呼び出し側から分かる範囲のみを判定した。architecture-map の Dispatcher facade、WorkUnit、eval trial、CoS の行と ADR-0082、ownerless-running-runs の終了契約を参照した。

## 作成前 brief

1. 根本原因: 処理名が似ていても前提や永続状態が異なり、単純な重複件数では共有可否を判断できない。
2. 責務 owner: worker_finish、work_units、leases、sinks、review_verdict、cos_chat、eval_trials。
3. 既存の再利用先: finish_worker_result_with、resolve_quota_estimate、renew_lease、infra_backoff_delay。
4. 選択肢: 呼び出し形だけで同一とする案を採らず、入力・前提・状態/event・失敗時の4軸で比較する。
5. 新しい状態・分岐・fallback・port: 無し。
6. 撤去対象: 無し（今回の範囲は測定のみ）。
7. 不変条件: コードを変えず、実在する行と関数を記録する。JSON の構造・参照・差分範囲を検査する。新規の製品試験は追加しない。

## run-close: different

全体は非等価。入力: task/WU は Task・任意 WU・RunOutcome/AdapterError・検査失敗、review は ReviewOutcome、CoS は ChatFinish/actions、eval は追跡中の run id と結果。前提: task/WU は入口で lease 一致を検査し stale を捨てる。review は lease を持たず Reviewing を確認、CoS は sink の finished と store 終端、eval は eval_runs と instance 所有を確認する。書き込み: task は WorkerFinished/QuotaEstimated 等を遷移と渡す。WU は先に WorkUnitTransitioned を書き、兄弟待ち/統合時は task を Running のまま events を個別追記する。review は Reviewer role の WorkerFinished と ReviewVerdict 等を書き、stale 時も cancelled の run 索引を閉じる。CoS は chat_run_finish_with_telemetry で会話状態・本文・usage を閉じ、Question も Completed。eval は run_index_finish と scoring/error を書く。CoS/eval のこの入口には task の WorkerFinished 追記や task lease 解放は無い。失敗: task の InvalidTransition は警告、その他の store error は返却、WU 遷移失敗は警告継続。CoS finish は store error を返すが finished は true にし、呼び元は警告。eval の索引失敗は警告して処理継続。部分共有: task と WU は既に finish_worker_result_with を共用し、task/review は resolve_quota_estimate を共用。共通化候補は run id・usage・metrics・明示終端から終了記録を組み立てる純粋処理まで。lease 所有の検証、状態遷移、transaction 境界、自己申告だけで完了させない条件を一律の close(run) に埋めてはならない。

根拠（全件は JSON）:

- `crates/task-dispatch/src/dispatcher/worker_finish.rs:337` — `finish_worker_result_with` (task)
- `crates/task-dispatch/src/dispatcher/work_units.rs:399` — `on_work_unit_checks_finished` (work_unit)
- `crates/task-dispatch/src/dispatcher/review_verdict.rs:20` — `on_review_finished` (review)
- `crates/task-dispatch/src/dispatcher/cos_chat/sink.rs:407` — `finish` (cos_chat)
- `crates/task-dispatch/src/dispatcher/eval_trials.rs:1046` — `on_eval_worker_finished` (eval_trial)

## lease-renew: different

5 呼び出しの store 操作は既に共通だが、呼び出し契約全体は非等価。入力は全て task id/run id/TTL。WU checks 開始は phase あり、handoff は結果の保存後で TTL=review_timeout*(2*checks+1)+grace。heartbeat は mutex と renew_every で間引き lease_ttl を渡す。回収時は手元の生存 process に max_wall_secs+grace、checking に review_timeout+grace を渡す。状態/event: 更新の委譲先は同じ renew_lease、呼び出し側が更新 event を追加するコードは無い。handoff のみ更新後に WorkUnitChecksHandedOff を追記。失敗: checks/handoff は Ok(false) を無視、Err は警告して進む（handoff event の失敗は返却）。heartbeat は false で戻り、busy だけ100ms間隔で最大3回再試行、他の Err/使い切りは警告して run を止めない。回収は true なら回収を避け、false なら後続の回収判定へ進み、Err なら次 tick へ延期。部分共有契約は renew_lease(task_id,run_id,ttl)->Result<bool,StoreError> の結果を保持する操作までで既存実装を再利用できる。bool の意味を消す共通の warn-and-continue wrapper は回収時の安全条件を壊す。store 内部は今回の読み取り範囲外で、ここでは呼び出しとその分岐を根拠とする。

根拠（全件は JSON）:

- `crates/task-dispatch/src/dispatcher/work_units.rs:50` — `spawn_work_unit_checks` (work_unit)
- `crates/task-dispatch/src/dispatcher/work_units.rs:293` — `record_check_hand_off` (work_unit)
- `crates/task-dispatch/src/dispatcher/sinks.rs:304` — `heartbeat` (task/work_unit)
- `crates/task-dispatch/src/dispatcher/leases.rs:252` — `reclaim_expired_leases` (task/work_unit)
- `crates/task-dispatch/src/dispatcher/leases.rs:275` — `reclaim_expired_leases` (work_unit)

## retry-backoff: different

非等価。通常 task retry は DB attempts/updated_at と設定 base/max を retry_backoff に渡し期限まで dispatch を止める（2本目の並列 WU は省略）。インフラ障害は events 由来の連続 infra 回数+1 を max_infra_retries と比べ、以内なら InfraRequeue、超過なら非 retryable WorkerError。成功した遷移後だけメモリの infra_backoff に期限を置く。lease 回収にも同じ上限判定と infra_backoff_delay の使用が重複するが、孤児再起動は待たず、WorkerFinished.end/class と WU 照合も異なる。WU 準備はメモリの連続回数、2^n 秒（60秒cap）、5回目または permanent で Blocked/Question・WorkUnitTransitioned、1本目は task も Unroutable にする。heartbeat は DB busy の操作再試行のみ（3回、固定100ms）で task attempts/event を増やさない。CoS は同一 run/input の経路 index を進める方式で、終了 handle/Running・壁時計予算・capacity が前提、経路通知を記録し利用不能/期限超過なら会話を閉じる。ここに指数 backoff は無い。eval の adapter error は trial error と cleanup で終わり自動 retry しない。失敗: task/回収の InvalidTransition は警告、その他は返却（期限は成功時のみ）。WU 準備の transition error は返却、CoS の停止/finish error は警告。部分共有候補は (infra_n,max)->再試行可否 と既存 infra_backoff_delay(n)->Duration の純粋な規則。期限の時計、回数の正本、適用先状態/event、孤児例外、error 処理を各 owner に残す。通常 retry_backoff の計算本体は範囲外なので式の等価性は推測しない。

根拠（全件は JSON）:

- `crates/task-dispatch/src/dispatcher/dispatch_run.rs:218` — `dispatch_one` (task)
- `crates/task-dispatch/src/dispatcher/worker_finish.rs:578` — `finish_worker_result_with` (task)
- `crates/task-dispatch/src/dispatcher/leases.rs:363` — `reclaim_expired_leases` (task/work_unit)
- `crates/task-dispatch/src/dispatcher/work_units.rs:1014` — `on_work_unit_prepare_failed` (work_unit)
- `crates/task-dispatch/src/dispatcher.rs:990` — `wu_prepare_backoff` (work_unit)
- `crates/task-dispatch/src/dispatcher/sinks.rs:311` — `heartbeat` (task/work_unit)
- `crates/task-dispatch/src/dispatcher/cos_chat/routes.rs:47` — `retry_routes` (cos_chat)
- `crates/task-dispatch/src/dispatcher/route_failure.rs:285` — `decide_route_failure` (task)
- `crates/task-dispatch/src/dispatcher/eval_trials.rs:1075` — `on_eval_worker_finished` (eval_trial)

## 補足と限界

- run-close の WU 遷移と WorkerFinished は呼び出し上別の書き込みである。全経路が単一 transaction だとは判定していない。
- lease-renew の回収側は Ok(false) と Err を区別する。この差を消す共通 wrapper は提案しない。
- retry の通常 delay は範囲外の関数への委譲であり、WU 準備の指数式と同一とは断定しない。review の metrics.retries は 0 固定（review_verdict.rs:54）。review の再起動を task attempts と同じ回数として扱わない。
- phase_integration の統合 lease 延長は extend_task_lease（leases.rs:289）で、renew_lease の 5 件には含めない。decision_log_wire は WorkerFinished を読む投影（decision_log_wire.rs:267）であり、run 終了の別実装として数えない。

## 検証

提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`。

- 条件0（3 JSON、sites/evidence 各2件以上）: run artifacts の `validate.py` を python3 で実行 → exit 0。3 object、19 sites の包含関数名、全 evidence の実在行を確認。
- 条件1（入力・前提・状態/event・失敗時の根拠）: JSON の shared_contract と上記本文をソース行に照合。3 件とも different と部分共有範囲を記録。
- 変更範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` → exit 0、指定の4ファイルのみ。validate.py でも許可集合との一致を確認。crates/・scripts/ の差分無し。
- `bash scripts/dev/test-affected.sh` → exit 0。doc-only、試験対象の変更なし。repo-health は pass、新規/悪化指摘0（既存141）。
- `cargo clippy --workspace -- -D warnings` → exit 0。依存 deno_core の既存警告1件、workspace 検査完了。
- `cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/progress-index.sh --check` → exit 0、ok。

全 workspace 試験・browser/userns・web e2e は実行していない。

## 未解決事項

今回の受け入れ条件について無し。実装の変更や store 内部の調査は行っていない。

## 提案

後続設計では infra 上限判定を純粋関数にできるか検討し、孤児の待ち省略と各 owner の event/失敗処理を維持する。今回この設計を採用・実装したものではない。

## commit

- `phase 1: document dispatcher run lifecycle equivalence`
