---
title: R5 bundle-api-b — task-api の routing・管理 API・その他の試験の束ね
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
status: done
updated: 2026-11-11
completed: 2026-11-11
---
# R5 bundle-api-b — task-api の routing・管理 API・その他の試験の束ね

完了日: 2026-11-11

WorkUnit `bundle-api-b`（段 bundle）。task-api の結合試験のうち、`inventory.json` で leaf が `bundle-api-b` の 37 target を 6 つの束（`crates/task-api/tests/<bundle>/main.rs`）にした。試験関数の中身は変えていない。leaf の割り当てで `docs` を 1 件 keep に戻した（下の「docs を束ねない理由」）。基準は `$CELERIS_WU_BASE`（bundle-api-a の後）。

## task-api の test binary の数
| 時点 | test binary（`cargo nextest list -p task-api`） | 試験 | 出典 |
|---|---|---|---|
| R5 の base（`inventory.json` の base_sha） | 114 | 878 | `baseline.json`、[bundle-api-a.md](bundle-api-a.md) |
| bundle-api-a の後（この WU の base） | 99 | 878 | この run で `crates/task-api/tests` を `$CELERIS_WU_BASE` の tree に戻して数えた |
| この leaf の後 | 68 | 878 | この run の HEAD |

binary は lib の unittest 1 個を含む。この leaf で 37 target が 6 束になり、99 − 37 + 6 = 68。

## 変更
- 件名「phase 1: bundle task-api routing, admin and unmapped integration targets」の commit — 37 target の `git mv`、6 つの `main.rs`、`classify.py` の scripts の path の数え方、`test_classify.py` の試験 4 件、`inventory.json` の作り直し。
- 件名「R5 bundle-api-b: ADR に docs を keep にした付記を書き、束の表を直す」の commit — ADR の D2・D4-1・束の表・動かさない target の表と付記。ADR はこの WU の write set の外（範囲 check は `crates/task-api/tests/`・`crates/task-api/Cargo.toml`・この dir だけを許す）なので、次の commit で base の内容に戻した。
- 件名「R5 bundle-api-b: 進捗に検査の結果を書く」の commit — この file。
- 件名「R5 bundle-api-b: ADR の変更を戻し、付記の案を進捗の提案に移す」の commit — ADR を `$CELERIS_WU_BASE` の内容に戻し（差 0 行）、付記の文と表の直しを下の「提案」に移した。

束（ADR [2026-11-10-test-target-bundles.md](../../adr/2026-11-10-test-target-bundles.md) の表から `unmapped_suite_a` の `docs` を外したもの。ADR の表は直していない。下の「提案」）:

| 束 | 件数 | 試験 | target |
|---|---|---|---|
| `browser_eval_surface_suite` | 6 | 19 | api_principals・browser_restore_deliver・browser_restore_live_session・campaign_ident・campaign_variant_atomic・mcp_admin |
| `cos_delivery_suite_c` | 8 | 60 | daemon_clusters_config・integration_api・integration_principals・notify・operations・single_human_window・task_list_with・task_wait_api |
| `routing_budget_suite_a` | 7 | 20 | cron_jobs・legacy_routing_events・routing・routing_source_state・routing_stage・saved_credentials・script_exhaustive |
| `run_worker_suite` | 2 | 15 | repository_required・secrets_admin |
| `unmapped_suite_a` | 7 | 62 | approvals・cluster_settings_admin・concurrency・console・console_instruct・conversation・files |
| `unmapped_suite_b` | 7 | 28 | genres・memory・roles・skills・standby・terminals・unblocks |

- `browser_restore_deliver`・`browser_restore_live_session` は task-worker の `tests/userns_gate/mod.rs` を `#[path]` で読んでいた。`browser_eval_surface_suite/main.rs` が `#[path = "../../../task-worker/tests/userns_gate/mod.rs"] mod userns_gate;` を 1 回だけ宣言し、2 file は `use crate::userns_gate;` にした。
- 移動した 37 file の差は、`mod common;` → `use crate::common;` の 1 行（上の 2 file はそれに加えて `#[path] mod userns_gate;` の 2 行 → `use crate::userns_gate;` の 1 行）だけ。`cargo fmt` の並べ替えは起きなかった。
- `legacy_routing_events` は旧い shape の event の replay の契約の試験で、束ねたが消していない（ADR D5-5）。

動かしていない target: objective の例にある routing_eval・script_domains・script_generations・decisions・knowledge は r6（地図が path を挙げる）、llm_accounts_admin・llm_routes_admin・llm_sources は keep（`#[path]` で file を共有）で、この leaf の target ではない。R2 と競合する task-api の browser_* は browser_e2e・browser_h3_injection の 2 件で、どちらも keep（file 名の数字・process global）。この leaf の browser_restore_deliver・browser_restore_live_session は R2〜R4 のどれとも競合しない（`inventory.json` の conflicts）。adjust-api の 4 件（cos_operations_domains・cos_ops_admin_config・cos_ops_projects_cron・project_auto_integrate）には触れていない。

## docs を束ねない理由
- `scripts/dev/check-doc-links.sh` の `FOREIGN_DOCS`（31 行目）が `crates/task-api/tests/docs.rs` を path で挙げ、この file の素の `docs/…`（試験の一時 repo の page。例 `docs/research/fs.md`）を「この repo の参照」から外している。`tests/unmapped_suite_a/docs.rs` に動かすと一覧に当たらなくなり、doc-link 検査（R5 の check の 1 つ）が落ちる。script は R5 の write set の外にあるので、一覧の path を直せない。
- 原因は `classify.py` の `external_refs` が `scripts/` の `--test`・`-E` しか読まず、path で名指しする script を見落としたこと。owner の `classify.py` を直した: `#` comment でない行が target の path を語として含むとき（`./` は可、`$root/repo/crates/…` のような一時 repo の path と、`.rs.bak` のような長い名前は数えない）、`scripts` の参照に数える。D4-1 の named_suite になり keep。
- comment の行は数えない。script の動作は変わらないため。数えると、完了した bundle-celeris が束ねた celeris の `script_compare_report`（`scripts/dev/tests/script-compare.sh` の 3 行目の comment）も keep になり、leaf をまたいで割り当てが食い違う。
- `inventory.json` は reclass と同じ方法で作り直した: 各 record の `external_refs` を base_sha の再走査で置き換え、`conflicts`・`branches`・`main_at_generation` は生成時のまま、`assign` と `make_bundles` をやり直し、`summary` と `rules.leaf_order` を作り直した（一時の Python。repo には置かない）。`external_refs.scripts` が変わったのは 4 target: `docs`（bundle-api-b → keep）、`ops_r2_session`（keep のまま、理由に named_suite が増えた）、`long_horizon_bench`・`long_run_soak`（keep のまま、参照の行が 1 つずつ増えた）。leaf が変わったのは `docs` だけで、`unmapped_suite_a` は 8 件から 7 件になった。task-api の summary は bundle-api-b 38 → 37、keep 17 → 18。

## 実装前 brief
1. 根本原因: 同じ fixture（`tests/common/mod.rs`）を使う試験が別々の target になり、target ごとに common と依存の link が走る。docs については、classify.py が script の path での名指しを見落とし、動かせない target を bundle leaf に入れていた。
2. 責務 owner: `crates/task-api/tests` の target 構成と、leaf を決める `classify.py` の `external_refs`。
3. 既存の再利用先: `tests/common/mod.rs`、`inventory.json` の leaf と束、bundle-api-a の束の形、reclass の inventory の作り直しの方法、`test_classify.py` の `FakeTree`。
4. 選択肢: (a) docs を動かし `check-doc-links.sh` の一覧を直す — write set の外なので採らない。(b) `inventory.json` の docs だけを手で keep にする — 段 final の sync-main が `classify.py --write` をやり直すと docs が束に戻るので採らない。(c) `classify.py` が script の path を数える — 採った。comment まで数える案は、script の動作に関係しない参照で完了済みの leaf の割り当てを変えるので採らない。
5. 新しい状態・分岐・fallback・port: 無し（keep の理由の検出を 1 つ足しただけ）。
6. 撤去対象: 担当 target の旧い path と、file ごとの `mod common;`・`#[path] mod userns_gate;` の宣言（束の `main.rs` に 1 回だけ置く）。
7. 不変条件: 試験関数・表の case・不変条件の試験が base と同じ。keep・r6 と数字を含む file は元の path に残る。script が path で名指しする target は keep になる。`cases.py`・`classify.py --verify-leaf`・`cargo nextest list`・`test_classify.py` で確かめた。

## 証拠（提出時の HEAD で実行）
- 条件 0: `python3 agent-docs/progress/2026-11-10-health-v2-r5/classify.py --verify-leaf bundle-api-b agent-docs/progress/2026-11-10-health-v2-r5/inventory.json` → exit 0。「37 moved target(s) of the leaf, 263 keep/r6 target(s) checked in place, 0 error(s)」。
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/classify.py --check agent-docs/progress/2026-11-10-health-v2-r5/inventory.json` → exit 0（352 targets、20 bundles、0 error）。
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/test_classify.py` → 16 tests OK（新しい `ScriptPathRule` 4 件を含む）。
- 条件 1: `python3 agent-docs/progress/2026-11-10-health-v2-r5/cases.py --crate task-api --base "$CELERIS_WU_BASE"` → exit 0。試験関数 855 → 855、表の行 757 → 757、名前で追った移動 204、不変条件の試験 257（39 file。bundle-api-a が動かした 3 file は capability-map の path が base に無く照合しない）、removals 0 件。R5 の base（`--base 2166a26556364c94851dc30f5f6a6cf5ad0b98bc`）に対しても exit 0（855 → 855、757 → 757、移動 306、不変条件の試験 277（42 file））。
- `cargo nextest list -p task-api --message-format json` → HEAD・WU の base とも exit 0。試験 878 → 878、binary 99 → 68。束の試験名 `<target>::<fn>` を元の `(task-api::<target>, <fn>)` に戻すと、878 組が base と一致した。base は同じ worktree で `crates/task-api/tests` を `$CELERIS_WU_BASE` の tree に戻して取り、取った後に `git reset --hard HEAD` で戻した。build の警告は 0 件。
- `cargo nextest run -p task-api -E 'binary_id(task-api::browser_eval_surface_suite) | binary_id(task-api::cos_delivery_suite_c) | binary_id(task-api::routing_budget_suite_a) | binary_id(task-api::run_worker_suite) | binary_id(task-api::unmapped_suite_a) | binary_id(task-api::unmapped_suite_b)'` → exit 0、204 passed、0 skipped。
- `cargo test -p task-api --test browser_eval_surface_suite --test cos_delivery_suite_c --test routing_budget_suite_a --test run_worker_suite --test unmapped_suite_a --test unmapped_suite_b`（1 process の中で並行に実行）→ exit 0、19・60・20・15・62・28 passed。D3 で束から外す target は無かった。
- `bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE"` → exit 0（「repo_health_gate: all checks passed」）。
- 条件 2: `bash scripts/dev/test-affected.sh` → exit 0（15 分 39 秒）。対象は celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker と web。nextest 5860 tests run: 5860 passed、22 skipped。web は tsc -b が通り、vitest 104 file・712 tests passed。health gate は verdict=pass（fail_grade 0。new 3 件は R5 の台本 cases.py・contracts.py・classify.py の `dup.contract` の warning で、この WU の base にある file）。
- 条件 2: `cargo clippy --workspace -- -D warnings` → exit 0（53 秒）。`cargo fmt --all -- --check` → exit 0（出力なし）。
- `python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R5 agent-docs/progress/2026-11-07-health-v2-b0/slices.json`（`git diff --no-renames --name-only "$CELERIS_WU_BASE" HEAD` と未追跡 path を標準入力）→ exit 0、「write-set R5: ok」。`sh "$CELERIS_WU_SCOPE_PATHS"` の出力は `crates/task-api/tests/` の下の束の file と、この dir の classify.py・inventory.json・test_classify.py・この file、ADR だけ。
- `sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-adr-numbers.sh`・`sh scripts/dev/progress-index.sh --check` → 各 exit 0。

## 未解決
- ADR [2026-11-10-test-target-bundles.md](../../adr/2026-11-10-test-target-bundles.md) は `docs` を keep にしたことをまだ反映していない（D2 の最大の束の行数、D4-1 の named_suite の条件、段 bundle の表の `unmapped_suite_a` 8 件、動かさない target の表の task-api keep 17 件）。`inventory.json` と束は 7 件で正しい。ADR を直す文は下の「提案」の 1 つ目にある。

## 提案
- ADR を書き換えられる unit（段 record、または段 bundle の統合）が、ADR [2026-11-10-test-target-bundles.md](../../adr/2026-11-10-test-target-bundles.md) に次の 5 点を入れる。この WU は ADR を write set の外として変えなかった。
  1. D2: 最大の束を「task-api `unmapped_suite_a` の 7 file 3736 行（注釈・空行込み。付記 bundle-api-b で `docs` を外した後）」にする（base_sha の tree で数えた。直す前は 8 file 4627 行）。
  2. D4-1 の named_suite の説明に「`scripts/` の `#` comment でない行が file の path を名指しする target も含む。付記 bundle-api-b」を足す。
  3. 段 bundle の見出しを「76 target → 15 束。task-api 55 → 9、celeris 15 → 4、celerisctl 6 → 2」にし、表の `unmapped_suite_a` を 7 件（approvals・cluster_settings_admin・concurrency・console・console_instruct・conversation・files）にする。
  4. 動かさない target の表の task-api を「keep 18、名前で呼ばれる suite 6」にする。
  5. 付記「実装との突き合わせ」に 1 行足す: 「2026-11-10（unit bundle-api-b）: task-api の `docs` を束から外して keep にした。`scripts/dev/check-doc-links.sh` の `FOREIGN_DOCS` が `crates/task-api/tests/docs.rs` を path で挙げ、束へ動かすと doc-link 検査が落ちる（script は R5 の write set の外）。`classify.py` の `external_refs` は `scripts/` の `#` comment でない行が target の path を含むときも `scripts` の参照に数える（`test_classify.py` の `ScriptPathRule`）。comment の行は数えない。leaf が変わったのは `docs` だけ。記録は bundle-api-b.md（ADR からの相対 path `../progress/2026-11-10-health-v2-r5/bundle-api-b.md` のリンクにする）。」
- `docs` を束ねるには、`scripts/dev/check-doc-links.sh` の `FOREIGN_DOCS` の path を束の path に直す変更が要る。script は R5 の write set の外なので、R6 か別の task で、一覧の path の変更と `docs` の移動を同じ変更で行う（`classify.py` の keep の理由はその変更で消える）。
- `scripts/dev/tests/script-compare.sh` の 3 行目の comment は celeris の `crates/celeris/tests/script_compare_report.rs` を挙げるが、bundle-celeris がこの file を `tests/routing_budget_suite/` に動かしたので、段 bundle の統合の後は古い path になる。script は R5 の write set の外なので、R6 で comment の path を直す（検査は落ちない。comment は doc-link 検査の対象外）。
