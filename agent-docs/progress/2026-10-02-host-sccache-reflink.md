---
title: sccache 撤去・host 設定化と reflink target（ADR-0129）
tasks: [01M3YD2Z585N1YCBZK4AH8QXR0, 01M3YEQW40FPMH82X1JMNXGJEE]
status: done
updated: 2026-10-03
---
# sccache 撤去・host 設定化と reflink target（ADR-0129）

> 旧 `docs/PROGRESS.md`（現 `agent-docs/PROGRESS.md`）に main が足した節を ADR-0128 D6 に従い sync-main-2 migrate-docs（task 01M3Z8CXYG1J6BZCQ87YS3FC67）がここへ移した。本文は元のまま（リンクだけ新配置へ直した）。

WorkUnit ごとの記録（旧 `docs/progress/` から移した）: [reflink-target-experiment](2026-10-02-host-sccache-reflink/reflink-target-experiment.md)、[sync-recheck](2026-10-02-host-sccache-reflink/sync-recheck.md)、[seed-refresh](2026-10-02-host-sccache-reflink/seed-refresh.md)。

## sccache 撤去 schema・運用文書（task `01M3YEQW40FPMH82X1JMNXGJEE`）

ADR-0129 (1) に従い `ScratchStatus.sccache` / `cache` の型は互換用に残し、説明を廃止・常に null に更新した。API schema と worker protocol schema は生成試験で照合。scratch status 試験は両欄が JSON null であることを確認する。GUI/web の型コメントも ADR-0129 に合わせた。`docs/ops/sccache-l1.md` は廃止と ADR 参照、人が本番 host で行う unit・旧 cache 後始末の手順に置き換えた。本番 host 操作は行っていない。

- `UPDATE_SCHEMA=1 cargo test -p task-api`: schema 一致と null 断言を含む unit 72 件成功、2 ignored。続く integration test `production_h3_injects_once_without_exposure` は sandbox の `unshare: Operation not permitted` で失敗。
- `UPDATE_SCHEMA=1 cargo test -p task-worker protocol::tests:: --lib`: 9 件成功（worker protocol schema 一致を含む）。
- `UPDATE_SCHEMA=1 cargo test -p task-core --lib`: 625 件成功。
- `corepack pnpm@11.27.0 -C gui gen:types` と `corepack pnpm@12.6.0 -C web gen:types`: pnpm store SQLite を開けず終了。生成型コメントは schema の ADR-0129 記述に手動同期し、型構造は変更していない。


## ADR-0129 (1) sccache 撤去: 統合後の全体検証（work unit `verify`）

完了日 2026-10-02。HEAD `273bf5f36153`（統合段 consumers → core → schema-docs がすべて終わった後）で
workspace 全体の test・clippy・fmt と、sccache 撤去自体の確認を行った。コードの変更は無し（検証のみ）。

- `cargo fmt --all -- --check` → exit 0（差分なし）。
- `cargo clippy --workspace -- -D warnings` → exit 0（警告なし）。
- `cargo test --workspace`:
  - 1 回目 exit 101。`crates/celeris/tests/instance_handoff.rs` の
    `a_newer_release_takes_over_while_the_old_one_finishes_its_run` が 1 件だけ失敗（「旧インスタンスがタスクを
    dispatch しない」、10 秒待ちで timeout）。他は全 passed。
  - 単独再実行（`cargo test -p celeris --test instance_handoff
    a_newer_release_takes_over_while_the_old_one_finishes_its_run -- --exact`）→ exit 0、1 passed。
    同時実行中の他試験との負荷競合による flake と判断し、設計変更は不要と判断した（このテストは
    release handoff のタイミング試験で、ADR-0129 の sccache 撤去とは無関係。このタスクの範囲では修正しない。
    時間依存試験の決定化は別タスクの対象）。
  - 2 回目のフル実行 → exit 0。120 試験バイナリすべて `test result: ok`、合計 **3228 passed / 0 failed /
    0 measured**（ignored を除く）。
- sccache 撤去自体の確認（非試験コード）:
  - `crates/scratch-cache` crate は存在しない。`crates/celeris/src/cache_server.rs` は存在しない。
  - `celeris-sccache.service` / `celeris-scratch-cache.service` は repo 内に存在しない
    （`scripts/selfdeploy/install-units.sh` は置かない。本番 host 側の既存 unit 停止・削除は
    `docs/ops/sccache-l1.md` に人が行う手順として書いてある。本番操作はしていない）。
  - `scripts/scratch/setup-sccache.sh` は存在しない。
  - `RUSTC_WRAPPER` / `SCCACHE_` の出現は `grep -rn --include='*.rs' crates/`（試験ファイル・`_tests.rs` 除外）で
    全件確認し、すべてコメント・doc comment（「差し込み・除去をしない」という設計を説明する注記、
    `ScratchSccacheView` 等の廃止済み型の doc）であって、実際の env 構築・差し込み・除去コードは無い。
  - `docs/ops/sccache-l1.md` は廃止の旨と ADR-0129 への参照、本番 host 側の後始末手順に置き換わっている
    （schema-docs work unit で完了済み。本 work unit では内容の変更なし）。
- 未解決事項: `instance_handoff.rs` の `a_newer_release_takes_over_while_the_old_one_finishes_its_run` は
  共用 host の負荷下で稀に flake する（今回 1/2 回）。ADR-0129 の変更とは無関係なので、このタスクでは直さない。
  時間依存試験の決定化（別タスクで進行中）の対象に含めるとよい。

### ADR-0129 (1) 撤去後の全体検証: 再実行（work unit `verify`、check 不合格の追試）

上記の run の後、celeris の post-run check が `cargo test --workspace` を再実行したところ別の 1 件
（`crates/task-dispatch/src/dispatcher/tests/cluster_job_wait.rs` の
`a_wait_parks_the_task_polls_and_resumes_as_a_continuation`、「condition not reached after 200 ticks」）で
exit 101 になった。コードは変更していない（検証のみ）ので、同じ HEAD で追試した。

- `uptime` は実行のたびに load average 34〜46（1 分）と非常に高い（他の並行 work unit・task による共用 host の
  負荷）。
- `cargo test -p task-dispatch --lib` で上記 1 件だけを単独実行 → exit 0、1 passed。この試験はバックグラウンド
  スレッド（`fake_poller` 等）の完了を `tick_until`（最大 200 回 × 20ms sleep = 4 秒)の実時間待ちで見ており、
  host が高負荷だとバックグラウンドスレッドが 4 秒以内に進まないことがある。`instance_handoff.rs` と同様、
  共用 host の負荷に起因する実時間待ちの flake であり、ADR-0129 の sccache 撤去とは無関係（dispatcher の
  cluster job wait 機能の話）。このタスクの範囲では修正しない。
- `cargo test --workspace` をさらに 2 回実行: 1 回目は上記と同じ 1 件が exit 101 で再現、2 回目は
  **exit 0、105 試験バイナリ、合計 3228 passed / 0 failed**（doctest 含む）。
- `cargo clippy --workspace -- -D warnings` → exit 0（警告なし）。
- `cargo fmt --all -- --check` → exit 0（差分なし）。
- sccache 撤去の確認を再実行し同じ結論を確認: `crates/scratch-cache` crate 無し、
  `crates/celeris/src/cache_server.rs` 無し、`celeris-sccache.service` / `celeris-scratch-cache.service` を
  置く記述が repo 内に無し、`scripts/scratch/setup-sccache.sh` 無し。非試験 `.rs` 内の `RUSTC_WRAPPER` /
  `SCCACHE_` の出現（`task-ops/src/daemon.rs`・`task-dispatch/src/dispatcher/{worker_task,housekeeping}.rs`・
  `task-worker/src/{preamble,scratch}.rs`・`celerisctl/src/commands/scratch.rs`）はすべて doc comment
  （廃止・常に `None` な互換型の説明、「継いだ env に触れない」という設計の注記）で、実際に env を組み立てる
  コードは無い。
- 結論: `instance_handoff.rs` に続き `cluster_job_wait.rs` も共用 host の負荷下で稀に flake することを確認した
  （どちらも ADR-0129 とは無関係）。コードの修正は行わず、`cargo test --workspace` が exit 0 になる実行を
  得たことと、sccache 撤去自体の確認が変わらないことを記録する。

### ADR-0129 (1) 撤去後の全体検証: 2回目の追試（work unit `verify`、check 不合格の再追試）

上記の再実行の後、celeris の post-run check が `cargo test --workspace` をさらに再実行したところ、
また別の 1 件（`tests/e2e/tests/api_scenarios.rs` の
`daemon_view_shows_in_flight_runs_and_cooldowns_and_throttle_is_recorded`、
「no cooldown in /daemon」）で exit 101 になった。コードは変更していない（検証のみ）ので、同じ HEAD で
追試した。

- `uptime` は実行のたびに load average 26〜40（1 分）と非常に高い状態が続いている（他の並行 work unit・
  task による共用 host の負荷。`instance_handoff.rs`・`cluster_job_wait.rs` の追試時と同様）。
- `cargo test -p e2e --test api_scenarios daemon_view_shows_in_flight_runs_and_cooldowns_and_throttle_is_recorded
  -- --exact` で単独実行 → exit 0、1 passed。この試験は fake-local provider の in-flight run から cooldown が
  `/daemon` に現れるまでを実時間で待っており（daemon の tick は `tick_ms: 50`）、host が高負荷だと
  background の tick/throttle 処理が期待する実時間内に進まないことがある。`instance_handoff.rs`・
  `cluster_job_wait.rs` と同様、共用 host の負荷に起因する実時間待ちの flake であり、ADR-0129 の sccache
  撤去とは無関係（daemon view の cooldown/throttle 表示機能の話。表示されている `scratch.sccache: null`・
  `scratch.cache: null` は ADR-0129 の設計どおり）。このタスクの範囲では修正しない。
- `cargo test --workspace` を再実行 → **exit 0、全試験バイナリ `test result: ok`**（lib 625 passed、
  task-dispatch lib 500 passed、task-worker lib 664 passed + 4 ignored 等を含む。failed 0 件）。
- `cargo fmt --all -- --check` → exit 0（差分なし）。
- `cargo clippy --workspace -- -D warnings` → exit 0（警告なし）。
- sccache 撤去の確認を再実行し同じ結論を確認: `crates/scratch-cache` crate 無し、
  `crates/celeris/src/cache_server.rs` 無し、`install-units.sh` に `celeris-sccache.service` /
  `celeris-scratch-cache.service` を置く記述が無し、`scripts/scratch/setup-sccache.sh` 無し。非試験 `.rs`
  （`*_tests.rs`・`tests.rs`・`tests/` 配下を除外）の `RUSTC_WRAPPER` / `SCCACHE_` の出現
  （`task-ops/src/daemon.rs`・`task-worker/src/{preamble,scratch}.rs`・
  `task-dispatch/src/dispatcher/{worker_task,housekeeping}.rs`・`celerisctl/src/commands/scratch.rs`）は
  すべて doc comment（廃止・常に `None`/`null` な互換項目の説明、「継いだ env に触れない」という設計の
  注記）で、実際に env を組み立てるコードは無い。
- 結論: 3 回の追試でそれぞれ異なる時間依存試験（`instance_handoff.rs`・`cluster_job_wait.rs`・
  `api_scenarios.rs` の 3 件）が共用 host の高負荷（load average 1 分で 20〜40 台）下で単発 flake したが、
  いずれも単独実行では即 pass し、ADR-0129（sccache 撤去）とは無関係であることを確認した。コードの修正は
  行わない（設計変更が要る — 実時間待ちを時計注入やイベント待ちに変える — ため、このタスクの範囲外。
  時間依存試験の決定化は別タスクで進行中）。`cargo test --workspace` が exit 0 になる実行を得たことと、
  sccache 撤去自体の確認（crate・cache_server・unit・setup スクリプトの不在、env 構築コードの不在）が
  変わらないことを記録する。

### ADR-0129 (1) sccache 撤去後の final review 不合格: browser_h3_wire の再確認（work unit `h3-recheck`）

final review で `cargo test --workspace` が `task-worker` の `browser_h3_wire`
（`inner_injection_wire`: `page target: SinkFailed`、
`real_broker_browser_injection_receipt_and_origin_guards`）で落ちた旨の指摘を受けた。

- ブランチ差分の確認: `git diff 7b77f17a39b3 HEAD -- crates/task-worker/src/browser*
  crates/task-worker/tests/browser*` は**空ではなかった**（Objective の前提が外れていた）。差分は
  `crates/task-worker/src/browser_specialist.rs` の 4 行のみで、`worker-core`（commit `75c54e53`、
  「task-worker から sccache の型・配線と env 除去の経路を外す」）が `WorkerAdapter` trait から
  `with_env_removed`（旧: `RUSTC_WRAPPER`/`SCCACHE_*` を継いだ env から取り除くための経路）を削除した際に、
  `BrowserSpecialistAdapter` 側の同名ラッパー実装 1 箇所を一緒に削除したもの。trait 側にも他の実装にも
  `with_env_removed` は残っておらず、`browser_h3_wire.rs` が検査する injection/origin guard のロジックには
  触れていない（env 除去経路の削除は本タスクの Objective そのもの）。
- `cargo test -p task-worker --test browser_h3_wire` を単独で 3 回実行 → **3 回とも exit 0、2 passed
  0 failed**（`inner_injection_wire` ok、`real_broker_browser_injection_receipt_and_origin_guards` ok）。
  2 回目の実行では cargo のビルドキャッシュ更新時に SQLite ロックの再試行ログ（`Error code 5: database is
  locked`、cargo 自身の再試行で解消）が出たのみで試験結果に影響は無い。
- `cargo fmt --all -- --check` → exit 0（差分なし）。
- `cargo clippy --workspace -- -D warnings` → exit 0（警告なし）。
- `cargo test --workspace` → **exit 0**。試験バイナリ 115 件すべて `test result: ok`、failed 0 件、
  `test result:` 行の `passed` 数を合算すると 3228。`tests/browser_h3_wire.rs` も同じ run 内で
  `inner_injection_wire ... ok` / `real_broker_browser_injection_receipt_and_origin_guards ... ok`。
- 否定 grep（基準2、前回 attempt の不合格点）`! grep -rn '"SCCACHE_' crates/task-dispatch/src
  crates/task-worker/src crates/celerisctl/src --include=*.rs | grep -v -E '/tests?(/|\.rs|_)'` → exit 0
  （`ctl-literal` work unit の修正が本ブランチに取り込まれていることを確認）。
- 結論: final review が報告した `browser_h3_wire` の失敗は、単独実行・全体実行のいずれでも再現しなかった
  （共用 host の負荷に起因する一時的な flake と推測するが、本タスクのコード — `browser_specialist.rs` の
  `with_env_removed` 削除 — を疑わせる具体的な根拠は無かった）。browser のコード・試験は変更していない。

再試行（attempt 2）で上の結論を取り直した。`git diff 7b77f17a39b3 HEAD -- crates/task-worker/src/browser*
crates/task-worker/tests/browser*` は同じ `browser_specialist.rs` の 4 行のみで変化なし。

- `cargo test -p task-worker --test browser_h3_wire` を単独で 3 回実行 → **3 回とも exit 0、2 passed
  0 failed**。
- `cargo fmt --all -- --check` → exit 0。`cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo test --workspace`（fail-fast、既定）を 2 回実行したところ、いずれも `task-worker` に到達する前に
  `e2e --test api_scenarios` の `daemon_view_shows_in_flight_runs_and_cooldowns_and_throttle_is_recorded`・
  `writes_from_celerisctl_and_api_while_celeris_ticks_fast_never_hit_database_is_locked`（実時間待ち依存、
  既存の共用 host flake。ADR-0129 の sccache 撤去や browser コードとは無関係）で停止した。
  `--no-fail-fast` で通したところ 1 回目は同じ e2e 2 件は `ok` だったが、代わりに
  `tests/browser_injection_wire.rs`（`browser_h3_wire.rs` とは別ファイル。同名の `inner_injection_wire`・
  `real_broker_browser_injection_receipt_and_origin_guards` を持つ）が同種の `SinkFailed` で落ち、
  `browser_h3_wire.rs` 自体は同じ run 内で `ok`。2 回目（通常の `cargo test --workspace`、fail-fast）は
  再度 e2e の `writes_from_celerisctl_and_api_...` のみ失敗。3 回目（`--no-fail-fast`）で
  **exit 0、115 バイナリすべて `test result: ok`、`passed` 合算 3228、failed 0** を得た
  （`browser_h3_wire.rs` は `inner_injection_wire ... ok` /
  `real_broker_browser_injection_receipt_and_origin_guards ... ok`）。
- 結論（attempt 2）: `browser_h3_wire` 単体は 3/3 で常に pass。`cargo test --workspace` 全体は実行ごとに
  e2e か他の browser 試験ファイルのいずれかで散発的に失敗するが、同じ run 内で失敗する試験ファイルが
  毎回違う（e2e 2 件 → browser_injection_wire → e2e 1 件 → 全 pass）ことから、特定のコード欠陥ではなく
  共用 host の負荷に起因する実時間待ち flake と判断する。本ブランチの browser 系ファイルに差分は無く
  （`browser_specialist.rs` の `with_env_removed` 削除のみ）、sccache 撤去のコードを疑う根拠は無い。
  最終的に `cargo test --workspace` exit 0 の run を得ている。

## sccache の host 設定化と reflink target

### deflake-lock: api_scenarios の `database is locked` 試験を進捗待ちへ（2026-10-02）

原因: `writes_from_celerisctl_and_api_while_celeris_ticks_fast_never_hit_database_is_locked`
（`tests/e2e/tests/api_scenarios.rs`）は 180 件の task の完了を `wait_until(Duration::from_secs(120), ...)`
という固定 wall-clock 期限で待っており、共用 host が高負荷のとき tick 処理が実時間内に収まらず
期限切れで落ちていた（daemon 自体は生きたまま in-flight で処理中）。
方式: `wait_until` とは別に `wait_for_progress(overall_limit, stall_limit, target, count)` を追加し、
done の件数が増え続ける限りは待ち、`stall_limit`（60s）だけ増えなければ失敗、全体は `overall_limit`
（600s）を安全弁にする出来事待ち（docs/testing.md 方法 2）に変えた。`tick_ms = 20` と件数（150+30）、
主張（全 done・daemon が生きている・`database is locked` が出ない・replay 一致）は変えていない。

- `cargo build -p celeris -p celerisctl` → exit 0。
- `cargo test -p e2e --test api_scenarios writes_from_celerisctl_and_api` → exit 0、1 passed。
- `cargo fmt --all -- --check` → exit 0（差分なし）。
- `cargo clippy -p e2e --all-targets -- -D warnings` → exit 0（警告なし）。


### verify: main の取り込み・全体検証（work unit `verify`、2026-10-02）

main を `wu/verify` の作業ツリーへ2段階で merge した。まず他 WU（`ops-doc`）ブランチの先端 `e901c9388601`（main を取り込んだもの）を取り込み、`docs/PROGRESS.md` の衝突は冒頭の節（ADR-0129 sccache schema・browser launcher 権限分離の2節）を両方残し、文末付近の `### land-main3` / `### pick-chrome` が本節（`## sccache の host 設定化と reflink target`）の下に誤って連結されていたのを `## ADR-0122 完了（ui-ux 外部 skills）` 配下の `### land-main` の直後へ戻した（内容は変更していない、見出しの付け先だけ修正）。それ以外の衝突（`Cargo.lock`・`crates/celeris/*`・`crates/task-dispatch/*`・`crates/task-worker/*` など）は git の自動 merge で解決し、手動介入は無かった。続いてその後に main が進んだ `14b052eaacee`（ADR-0079 D7 continue note を child_objective/human_decisions へ届ける変更。`crates/task-dispatch/src/dispatcher/work_units.rs`・`crates/task-ops/src/phase_gate.rs`・`crates/task-ops/src/tree.rs`・試験・ADR 付記のみで `docs/PROGRESS.md` には触れない）を merge し、衝突なしで取り込んだ。

- `git merge-base --is-ancestor main HEAD` → exit 0（main `14b052eaacee` が HEAD の祖先）。
- 衝突マーカー確認: `grep -rln '^<<<<<<<\|^=======$\|^>>>>>>>' .`（`.git/` 除外）→ 該当なし。
- `cargo fmt --all -- --check` → exit 0（差分なし）。
- `cargo clippy --workspace -- -D warnings` → exit 0（警告なし）。
- `cargo test --workspace`:
  - 1 回目の完走 → **exit 0**。108 試験バイナリすべて `test result: ok`、`passed` 合算 **3326**（main `14b052eaacee` の `tree_decisions.rs` 新規試験 2 件を含む）、`failed` 0、`ignored` 12（内訳: `task-api` 2 件 `UPDATE_SCHEMA` 手動確認用、`task-ops` 4 件（`execution_metrics_comparison_tests` の手動計測・`profile_timeline_against_a_db_copy`）、`browser_runtime_isolated.rs` の helper process 3 件、`ssh_cluster_manual.rs` の実クラスタ要の 2 件、`task-worker` doctest 1 件。いずれも既存の意図的な ignore で、このタスクの変更とは無関係）。再実行なしで一発で通ったため、共用 host の負荷起因 flaky（`instance_handoff`・`cluster_job_wait`・`api_scenarios` 系）は今回は発現しなかった。
  - sandbox の userns 制約で落ちる既知試験: 今回は発現せず、`CELERIS_ISOLATION_TESTS=skip` 等の opt-in は使わなかった。`crates/celeris/tests/instance_handoff.rs` は8件（`a_newer_release_takes_over_while_the_old_one_finishes_its_run`・`a_stale_heartbeat_promotes_the_standby` を含む）すべて `ok`。
  - `tests/e2e` の負荷時 flaky（`writes_from_celerisctl_and_api_while_celeris_ticks_fast_never_hit_database_is_locked`・`daemon_view_shows_in_flight_runs_and_cooldowns_and_throttle_is_recorded`）も今回の `cargo test --workspace` の中で両方 `ok`。本 work unit は `tests/e2e` を変更していない（`git diff` の変更範囲に `tests/e2e/` は含まれない）。
- `deflake-lock`（work unit `deflake-lock`）で固定 120s `wait_until` を `wait_for_progress` の出来事待ちへ直した試験 `writes_from_celerisctl_and_api_while_celeris_ticks_fast_never_hit_database_is_locked` は、上記 `cargo test --workspace` の中で `tests/api_scenarios.rs`（11 passed）の一部として `ok`。`database is locked` は出ず、180 件の task はすべて done に到達した（試験の主張どおり）。

### verify: main の追加取り込み（33aca5a3）と再検証（Run #3、2026-10-03）

Run #2 で main がさらに進んだ `33aca5a3`（アカウント/プロバイダー分離とモデル階層ルーティング関連。`config/*.example.toml`・`crates/celeris/src/config/*`・`crates/celeris/src/daemon/*`・`crates/llm-proxy/src/config.rs` などを含む。`docs/PROGRESS.md` には触れない）を merge し、コミット `bffc87b2`（`wu/verify: main (33aca5a3) を取り込み`）として取り込み済みだった。Run #3 ではこの状態を引き継ぎ、PROGRESS への記録が未了だった全体検証をやり直した。

- `git status --short` → 変更なし（Run #2 終了時点で merge 済み・commit 済み）。
- `git merge-base --is-ancestor main HEAD` → exit 0（main `33aca5a3` が HEAD の祖先。`git fetch origin main` 後の `origin/main` と一致）。
- 衝突マーカー確認: `grep -rn '^<<<<<<<\|^=======$\|^>>>>>>>' --include=*.rs --include=*.md --include=*.toml .`（`target` 除外）→ 該当なし。
- `cargo fmt --all -- --check` → exit 0（差分なし）。
- `cargo clippy --workspace -- -D warnings` → exit 0（警告なし、`Finished` のみ）。
- `cargo test --workspace` → **exit 0**。120 試験バイナリすべて `test result: ok`、`passed` 合算 **3351**、`failed` **0**、`ignored` **12**（内訳は上の 14b052ea 検証時と同じ既存の意図的 ignore。main `33aca5a3` が追加した provider/config 周りの新規試験も含めすべて通過）。一発で通り、再実行は不要だった。
  - sandbox の userns 制約で落ちる既知試験: 今回も発現せず、`CELERIS_ISOLATION_TESTS=skip` は使わなかった。`crates/celeris/tests/instance_handoff.rs` は8件（`a_newer_release_takes_over_while_the_old_one_finishes_its_run`・`a_stale_heartbeat_promotes_the_standby` を含む）すべて `ok`。
  - `tests/e2e` の負荷時 flaky 2 件（`writes_from_celerisctl_and_api_while_celeris_ticks_fast_never_hit_database_is_locked`・`daemon_view_shows_in_flight_runs_and_cooldowns_and_throttle_is_recorded`）も `cargo test --workspace` の中で両方 `ok`。本 work unit は `tests/e2e` を変更していない。
  - 参考: Run #2 の途中経過では `cargo test --workspace` を3回走らせており、2回目（merge 直後、commit 前）に `task-worker` の `browser_restore_deliver.rs::live_session_delivers_restored_state_over_its_own_cdp_pipe` が1件だけ `StateRejected` で落ちた（共用 host の負荷に伴うタイミング依存の既知 flaky、`docs/testing.md` 方法3〈SIGSTOP stutter〉系統。本 work unit は `task-worker` の browser 配送コードを変更していない）。3回目の全体再実行では発現せず `ok`。Run #3 の今回の実行でも発現しなかった。
- `deflake-lock` で直した試験 `writes_from_celerisctl_and_api_while_celeris_ticks_fast_never_hit_database_is_locked` は、今回の `cargo test --workspace`（`tests/api_scenarios.rs`、11 passed の一部）でも `ok`。`database is locked` は出ず、全 task が done に到達した。

#### ディスク使用量・ビルド時間の実測（参照）

`experiment` work unit の `docs/progress/reflink-target-experiment.md` に記録済みの実測を要約する（本 work unit では再測定していない）。

- seed から空で build: 5 crate を compile、0.74 秒。
- seed を同じ source・別 target path へ `cp -a --reflink=auto` でコピーした直後の build: compile 0 件、0.03〜0.04 秒（`df` 増分 +27.6 MiB、コピー先 `du` 28 MiB — 当時の実験環境は ext 系で実体コピー。container の `/local`（btrfs）は `FICLONE`/`FICLONERANGE` が EPERM だが、`copy_file_range(2)` で extent 共有となり `df` 増分 0 MiB を人が別途実測済み、`docs/adr/0129-host-sccache-reflink-targets.md` 参照）。
- 新規 checkout を模して mtime を更新した場合: `pdep`（path 依存）と `app` の 2 crate が再 compile、0.23 秒。registry 依存（itoa・libc・anyhow）は mtime 更新でも再ビルドされなかった。
- 結論: seed からのコピーは target path・source path が変わっても mtime を保てば fresh 判定を保てるが、worktree の checkout で mtime が変わる経路（path 依存とその利用側）は再ビルドが残る。これは ADR-0129 付記と `reflink-target-experiment.md` の既存の結論のままで、今回の merge・検証で変化はない。

#### 未解決事項

- 本番の切り替え（host の `~/.cargo/config.toml`・sccache user unit・`/local` への scratch 移行）は人が `docs/ops/host-sccache-reflink-targets.md` の手順で実行する。本 work unit は本番 host には触れていない。
- `/local`（btrfs）上での実際の reflink 共有（`copy_file_range` の extent 共有、`df`/`filefrag` での確認）は `CELERIS_REFLINK_TEST_DIR` を人がこの環境変数に `/local` 配下のパスを設定して実行する必要がある。このタスクの run 環境には書き込み可能な `/local` が無いため実行していない。
- 共用 host の負荷起因の時間依存試験（`instance_handoff`・`cluster_job_wait`・`api_scenarios` 系）は今回発現しなかったが、既知の flaky として別タスク（時間依存試験の決定化）で追跡中。

#### 提案

- `docs/PROGRESS.md` の末尾追記方式（複数 work unit が同時に EOF へ `##`/`###` を足す）は、今回のように無関係な既存節（main 側の `land-main3`/`pick-chrome`）が別 work unit の新設 `##` 節の下に紛れ込む merge 結果を生みやすい。長期分岐タスクでは「新しい `##` 節は必ず対象の h2 の直後に挿入する」運用、または merge 後に見出しの親子関係をざっと確認する一手順を `docs/testing.md` か ADR に足すとよい。


## main (aed80844) 取り込みと衝突解消（work unit `sync-main`）— 2026-10-03

完了日 2026-10-03。前回 review 差し戻し時点の main `33aca5a3`（HEAD の祖先）から main `aed80844`
（本番 admission ADR 番号振り直し 41366893・web release packaging 修正 aed80844 を含む）まで進んでいたため、
`git merge main --no-ff` で取り込んだ。衝突は想定どおり 2 file:

- `docs/PROGRESS.md`: HEAD 側（ADR-0129 sccache 撤去・reflink target の節）と main 側（launcher の
  SCM_CREDENTIALS 対応・web release 依存欠落の事故記録・ADR-0135/ADR-0138）の両方を、見出しの前後関係を
  保ったまま残した（衝突マーカーを除去するだけで内容の削除・改変はしていない）。
- `scripts/selfdeploy/install-units.sh`: main が足した `celeris-web-lan.socket`/`celeris-web-lan.service`
  と ADR-0135 D3 のコメントを採用しつつ、本ブランチの撤去（`celeris-sccache.service`・
  `celeris-scratch-cache.service` を `for unit in ...` に含めない）を保った。結果の行は
  `for unit in celeris@.service celeris-gui@.service celeris-web@.service celeris-web-lan.socket celeris-web-lan.service; do`。

`docs/architecture-map.md` は auto-merge のみ（2 行削除、main 側の反映）で衝突なし。他に壊れた file はない。

### 証拠コマンドと結果

- `git merge-base --is-ancestor main HEAD` → exit 0。
- `git merge-tree --write-tree main HEAD` → exit 0（衝突なしの tree を出力）。
- `cargo fmt --all -- --check` → exit 0。
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0（警告なし）。
- `cargo test -p task-worker scratch` → exit 0、33 passed、0 failed。
- `cargo test -p task-dispatch --lib scratch` → exit 0、10 passed、0 failed。
- `for t in scripts/selfdeploy/tests/*.sh; do bash "$t" || exit 1; done` → exit 0。全 12 本 ok
  （`install-units.sh` を直接叩く試験は無いが、`promote_web_follows_release.sh`・`web_follow_health_gate.sh`
  が web-lan 経路を通す。いずれも ok）。
- `cargo test --workspace` → exit 0。3408 passed、0 failed（`database is locked` は出ず、`instance_handoff`・
  browser launcher 系の既知 flaky も今回は発現しなかった）。

### 未解決事項

- 本番 host（`~/.cargo/config.toml`・sccache user unit・`/local` への scratch 移行、
  `celeris-web-lan.*` の install）は引き続き人が `docs/ops/host-sccache-reflink-targets.md` と
  「人が実行する手順」節の手順で行う。本 work unit は本番 host には触れていない。

main aed80844 取り込み、selfdeploy 試験全 pass（work unit `merge-latest`、別ブランチ）。main はこの task の work unit `sync-latest` を既に `30e4a37d` で取り込み済みで、HEAD がその祖先だったため `git merge main` は fast-forward（新規 merge commit なし、`docs/PROGRESS.md` に衝突マーカーなし）。`git merge-base --is-ancestor 41366893 HEAD` は exit 0。

## main (0b8a2256) 再取り込み（work unit `sync-main` 再実行）— 2026-10-03

完了日 2026-10-03。前回 review 差し戻し後、main が `aed80844` からさらに `merge-latest` 工程の統合
commit `0b8a2256`（launcher の browser_isolation・credentiald injection_ipc・prod_admission 試験の追加）
まで進み、HEAD（`afa5bddf`/`6ef858f6`、main `aed80844` 時点）の祖先ではなくなっていたため、再度
`git merge main --no-ff` で取り込んだ。衝突は `docs/PROGRESS.md` のみ（上の 2 節が別ブランチ由来の
重複する記録だったため、両方をそのまま残して解消。内容の削除・改変はしていない）。
`scripts/selfdeploy/install-units.sh` は main 側の新規コミットが同ファイルを変更していなかったため
自動 merge で衝突なく、web-lan unit を含み sccache/scratch-cache unit は含まない形のまま残った。
他の file（`docs/architecture-map.md` 含む）に衝突・壊れはない。

### 証拠コマンドと結果

- `git merge-base --is-ancestor main HEAD` → exit 0。
- `git merge-tree --write-tree main HEAD` → exit 0（衝突なしの tree を出力）。
- `cargo fmt --all -- --check` → exit 0。
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0（警告なし）。
- `cargo test -p task-worker scratch` → exit 0、33 passed、0 failed。
- `cargo test -p task-dispatch --lib scratch` → exit 0、10 passed、0 failed。
- `for t in scripts/selfdeploy/tests/*.sh; do bash "$t"; done` → 全 12 本 exit 0
  （`promote_web_follows_release.sh`・`web_follow_health_gate.sh` が web-lan 経路を通す）。

### 未解決事項

- 本番 host の手順は変わらず `docs/ops/host-sccache-reflink-targets.md` を参照。本 work unit は
  本番 host には触れていない。

## ADR-0136 /local 配置整合と main (3527c8e3) 取り込み（work unit `merge-main`）— 2026-10-03

完了日 2026-10-03。final review 差し戻し 3 点（ADR-0136 の `/local/celeris/data/scratch` への整合、
ADR-0129 §3 の mount 不在時の記述、ADR-0129 frontmatter の位置）は前段 `align-docs` 工程で直し済み。
本 work unit は最新 main `3527c8e3`（受信箱/通知フィード統合 `inbox-and-notifications` 一式、
ADR-0133 を含む）を `git merge main --no-ff` で取り込んだ。

ADR-0136 整合 3 点（merge 後も残存を確認済み）:

1. `docs/adr/0129-host-sccache-reflink-targets.md` §3・`docs/ops/host-sccache-reflink-targets.md` の
   scratch path を `/local/celeris/data/scratch` に統一し、両文書から
   ADR-0136（`0136-local-hot-data-layout.md`、main 未取り込み） へリンク。`crates/task-worker/src/scratch/tests.rs:1054`
   付近の試験 path も同じ値に揃えた。旧 path `/local/celeris/scratch` は残っていない。
2. ADR-0129 §3 の mount 不在時の記述を実装（`config/scratch.rs` の `apply_mount_check`）に合わせ、
   「mount 指定があり満たされないときは従来の既定 scratch dir へ戻り、理由をログに出す。`/local` 上に
   同名ディレクトリは作らない」とした。
3. ADR-0129 frontmatter（`---` / `tasks: [01M3YD2Z585N1YCBZK4AH8QXR0]` / `---`）をファイル 1〜3 行目に移動。

main 取り込みは衝突 2 file（事前の `git merge-tree` 見積もりでは無衝突想定だったが、実際の merge では
`docs/PROGRESS.md`・`scripts/selfdeploy/install-units.sh` 以外の crate 側ファイルは ort の自動 merge で
解消、衝突マーカーが残った file はゼロ）:

- `docs/PROGRESS.md`: 両ブランチの節をそのまま両方残した（本節もその後ろに追記）。
- `scripts/selfdeploy/install-units.sh`: main 側に本 merge による変更はなく、web-lan unit を含み
  sccache/scratch-cache unit を含まない既存の形のまま。

### 証拠コマンドと結果

- `git merge-base --is-ancestor main HEAD` → exit 0（main `3527c8e3` は HEAD の祖先）。
- `git diff --quiet HEAD && git ls-files -u` → 差分なし・unmerged パスなし。
- `grep -n '/local/celeris/scratch' docs/adr/0129-host-sccache-reflink-targets.md docs/ops/host-sccache-reflink-targets.md crates/task-worker/src/scratch/tests.rs` → 該当なし（exit 1）。
- `cargo fmt --all -- --check` → exit 0。
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0（警告なし）。
- `cargo test -p task-worker scratch` → exit 0、33 passed、0 failed。
- `cargo test -p task-dispatch --lib scratch` → exit 0、10 passed、0 failed。

### 未解決事項

- 本番 host の手順（`/local` への scratch 移行・sccache 設定）は変わらず
  `docs/ops/host-sccache-reflink-targets.md` を参照。本 work unit は本番 host には触れていない。

## ADR-0129 seed の build 前 probe と既定 false（work unit `seed-probe`）— 2026-10-03

完了日 2026-10-03。現行本番の ext4 `/var/lib/celeris/scratch` で seed を全 repo 分 build しても target に共有できず、ディスクを二重に使う欠陥を修正した。
`[scratch] seed_reflink` の既定を false にし、明示的に有効にした場合も pool 内で `cp -a --reflink=auto` と FIEMAP shared の probe を build 前に 1 回行う。共有不可なら理由をログと更新結果に残し、seed ディレクトリも cargo build も作らない。task・WU の target は従来どおり空から始まる。
[ADR-0129](../adr/0129-host-sccache-reflink-targets.md) と [人向け切替手順](../../docs/ops/host-sccache-reflink-targets.md) §3 に既定値と `/local` 移行時の明示的な有効化を記録した。本番 host の設定やサービスは変更していない。

証拠（いずれも exit 0）:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test -p task-worker scratch` — 34 passed。`seed_skipped_when_pool_cannot_share` を含む。
- `cargo test -p task-dispatch --lib scratch` — 11 passed。2 repo の更新バッチでも probe 1 回、seed ディレクトリと cargo build は無し。
- `cargo test -p task-dispatch --lib seed_housekeeping_runs_on_its_interval_and_retires_stale_generations` — 1 passed。
- `cargo test -p celeris scratch_mount_falls_back_to_default_dir_and_seed_reflink_defaults_off` — 1 passed。
- `git merge-base --is-ancestor main HEAD` — main `3527c8e3` は本変更前の HEAD `9d326b05` の祖先であり、追加 merge は不要。
