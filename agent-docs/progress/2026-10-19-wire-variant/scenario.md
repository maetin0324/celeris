---
task: 01M5B84ZY6H4001ZYPSGN9VR34
tasks: [01M5B84ZY6H4001ZYPSGN9VR34]
title: 専門分担 spec-off/on の通常 adoption・子孫生成・retry を実経路で検証（scenario）
work_unit: scenario
status: done
updated: 2026-10-20
---
# scenario: spec-off / spec-on の適用シナリオ

記録の本体は同じ dir の `application.md`（record 葉で統合する）。fixture は `infra.md`。

## 変更
- `crates/campaign/tests/ablation_application.rs`: `spec_off_and_on_apply_through_real_dispatcher_and_worker` を追加（約 22s）。
  - 同一 B0 資料: org の block（`cos`・`coding`・`coding-spec`）を 1 か所に書く。off = cos+coding、on = 3 つ全部。coding 課の block は共通なので、実効 profile hash は off / on で同じ（assert している）。専門課は coding 課と同じ harness 許可に skill `spec-skill` を 1 つ足しただけ。課が分かれるのは既存 matching の skill の重なりだけによる。
  - 同一非対象設定: `[execution] gate = "on"`・`[execution.tree] enabled = true` とスタブの仕様（計画・失敗）を off / on で同じ const から渡す。daemon と DB は別々に所有する（instance id と DB path が違うことを assert）。
  - 経路: GET `/org` → `OrgSnapshot::from_get_org`・`org_pin("coding")`（既存の公開 API）で variant pin を組む → 通常の `POST /tasks`（variant 付き campaign 識別、`execution: compound`）を 1 件だけ送る → 実 dispatcher → 実 fake adapter → スタブ process。root の planner run が計画（子 2 件。一般 `gen-skill`/atomic、専門 `spec-skill`/compound）を書き、それが通常どおり採用される。専門の子の planner run が孫 1 件（`spec-skill`/atomic）の計画を書く。孫は最初の 1 回だけ決定的に `retryable` で失敗するので、retry が実際に起きる。
  - 同期: 一般の子と、retry 後の孫の run が done になり、その `worker_finished` が届くまで `wait_events`（SSE ＋期限 120s）で待つ。sleep は使わない。
  - assert（環境ごと）:
    - 木は 4 task。`created` は 4 件で、root だけ origin なし（POST）、他は `origin = plan_unit`。親の `execution_planned{origin: planner}` と `child_task_created{child_task_id, depth 2/3, unit_key}`、`parent_id` を確認する。planner の起動はスタブ。
    - retry: 孫の run は 2 つで、error → done。`worker_finished` の outcome は `error(retryable=true)` と `done`。`running→ready (worker_error)` の遷移がある。`assigned` は 1 回だけで、retry 後の assignee も同じ。2 run が受け取った `RunRequest.context.campaign` は完全に同一。
    - 全 run の pin が、投入した variant / org pin と、この daemon の `GET /health` の instance id・`GET /config` の config sha256・org digest・selected harness `stub` に一致する。
    - PID: スタブの親 PID は daemon、終了後に `/proc` は無い、1 run 1 process、`worker_finished` と `GET /tasks/{id}/runs` に同じ run がある。
    - 全子孫の `assigned` が 1 件ずつで、木の外への割当は無い。
  - assert（off / on 比較）: off は全 4 task が `coding`（他 node への割当 0）。on は root と一般の子が `coding`、専門の子と孫が `coding-spec`（割当先の集合は 2 課）。差は専門の子と孫だけ。
  - 変異確認: on の期待値を `coding` に書き換えると失敗した（left `coding-spec`）。戻し済み。
- `crates/campaign/tests/ablation_application_support/mod.rs`:
  - スタブの起動記録に `campaign`（受け取った `context.campaign`）と `artifacts_dir` を足した。`read_launches(dir)` を自由関数にした（待ちの述語から読むため）。
  - planner run には `CELERIS_RUN_ID` が渡らない（`followups_env` は worker run だけ）。そのためスタブは PID 入りの `result.md`（`result_md_body`）を書く。`launch_runs()` は、その sha256 と一致する daemon の `artifact_produced{run_id}` で起動と run id を対応付ける（error の run は env の run id を使う）。どちらの場合も、その run の `worker_started` が同じ task にあることを確かめる。`assert_only_stub_launches` もこの対応を使う。これで planner run も含めて「起動 = worker_started」の 1 対 1 を見る。

## 証拠
- `cargo test -p celeris-campaign --test ablation_application` → 3 passed（22.5s）
- `cargo clippy -p celeris-campaign --all-targets -- -D warnings` → exit 0
- `bash scripts/dev/test-parallel.sh` → exit 0、5920 passed / 0 failed / 15 ignored
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0
- `grep -nE 'thread::sleep|tokio::time::sleep\(' <両 file>` → 該当なし

## 未解決・注意
- 試験 process が SIGKILL された場合（外側の timeout など）は `Drop` が走らず、`$TMPDIR` に fixture dir が残る（process は daemon の終了とともに消えた）。通常の失敗（panic）では回収される。
- root task はこの段では完了まで待たない（子孫の割当・retry・pin が対象）。root の最終 review・統合は範囲外。
- 負例（hash 改変・欠落の拒否、投入・再開・冪等照会）・非対象同一性・canary は negative 葉で扱う。
