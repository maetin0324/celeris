---
title: 健全性v2 R5 sync-main — R2・R3 の入った main の取り込みと、束ねとの衝突の解決
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
status: done
updated: 2026-11-11
completed: 2026-11-11
---
# 健全性v2 R5 sync-main — R2・R3 の入った main の取り込みと、束ねとの衝突の解決

完了日: 2026-11-11

WorkUnit `sync-main`（段 final）。`git merge main` で main を取り込んだ（rebase・checkout はしていない）。基準は `git merge-base HEAD main`（取り込みの後は main の先頭）と、R5 の base（`inventory.json` の `base_sha` 2166a26556364c94851dc30f5f6a6cf5ad0b98bc）。

## 結論
- main は HEAD の祖先になった（`git merge-base --is-ancestor main HEAD` → exit 0）。git の衝突は task-api の 2 file の `use` の行だけで、両側の変更を保って解いた。
- 取り込んだ main には R2（task 01M6TYVJT0Z1CBGNK8VTNK8ZJH）と R3（task 01M6TYVJT2HG9NY5FW9FMCZS9A）が入っている。R4（task 01M6TYVJT4S29ZMP6P0D4831JG）の branch は main の先頭と同じ commit で、R4 の変更はまだ無い。人の決定 `r-landed`（b、今の main で始める）のとおり、段 measure の計測には R2・R3 が入り、R4 は入らない。R4 の後の調整は R6 に回す。
- R2・R3 は test target を足しておらず、名前も変えていない（main の先頭でも test target は 352 件で、path の集合は R5 の base と同じ）。`inventory.json` に足す target は 0 件で、file は変えていない。
- R2・R3 が変えた test target 20 件の変更は、束ねた先も含めてすべて HEAD に残っている（下の表）。`cases.py` は task-api・celeris・celerisctl で exit 0。

## 実装前 brief
1. 根本原因: R5 の段 bundle・dedupe は R2・R3 が main に入る前の base で試験 file を動かし、helper を `tests/common` に寄せた。R2・R3 は同じ試験 file の元の path を変えたので、取り込みでは git の衝突と、名前の変わった API（`TaskStore` から `OrgDirectory` へ移った method など）を呼ぶ意味の衝突が起こりうる。
2. 責務 owner: 衝突した試験 file（`crates/task-api/tests/` の 2 file）。分類は `classify.py` と `inventory.json`。
3. 既存の再利用先: git の rename 検出（束へ動かした file に main の変更を写す）、`classify.py --check`・`--verify-leaf`、`cases.py`、`cargo check --workspace --tests --keep-going`（意味の衝突を全部一度に出す）。
4. 選択肢: (a) `classify.py --write` の出力で `inventory.json` を置き換える — 採らない（下の「`classify.py --write` のやり直し」）。(b) `inventory.json` を変えず、新しい main で作り直した一時の inventory と比べて差を記録する — 採った。
5. 新しい状態・分岐・fallback・port: 無し。
6. 撤去対象: 無し（衝突の解決だけ。dedupe-api が消した helper の写しは消したまま）。
7. 不変条件: R2・R3 の試験の変更が HEAD に残る。試験関数と表の行が減らない（`cases.py`）。keep・r6 の target は元の path にある（`classify.py --verify-leaf`）。

## 変更
- 件名「phase 1: sync-main: R2・R3 の入った main を取り込み、task-api の 2 file の import の衝突を両側の意図で解く」の merge commit。
  - `crates/task-api/tests/cos_operations_domains.rs`: HEAD（dedupe-api）は `raise_decision` を `tests/common/cos_ops.rs` に寄せ、`task_core::decision` の `use` を消していた。main（R3 の move-org）は `use task_core::{…}` に `OrgDirectory` を足した。`decision` の `use` は消したまま、`OrgDirectory` を足した。
  - `crates/task-api/tests/project_auto_integrate.rs`: HEAD は `auth`・`patch` の写しを消して `serde_json::Value` を使わなくなった。main は `use task_core::TaskStore;` を `OrgDirectory` に替えた。`use serde_json::json;` と `use task_core::OrgDirectory;` にした。
- 件名「phase 2: sync-main: 取り込みの結果と検査を進捗に書く」の commit — この file。

## R2・R3 が変えた test target（R5 の base から main の先頭まで、`crates/*/tests/*.rs` の 20 件）
| leaf（`inventory.json`） | 件 | target | HEAD での扱い |
|---|---|---|---|
| bundle-api-b（束へ移動済み） | 1 | task-api `approvals` | git の rename 検出で `tests/unmapped_suite_a/approvals.rs` に写った |
| bundle-celeris（束へ移動済み） | 1 | celeris `variant_propagation` | git の rename 検出で `tests/unmapped_suite/variant_propagation.rs` に写った |
| adjust-api | 4 | task-api `cos_operations_domains`・`cos_ops_admin_config`・`cos_ops_projects_cron`・`project_auto_integrate` | 元の path のまま。2 件は上の衝突を解いた |
| keep | 10 | campaign `preflight_deny_file`、celeris-credentiald `injection_ipc`、celeris-mcp `mcp_integration`、celeris `trial_context_canary_e2e`、celerisctl `org_migrate_v2`、task-api `browser_e2e`・`browser_h3_injection`・`project_cos_guard`・`project_plan`、task-ops `browser_policy_autoattach` | 元の path のまま（自動 merge） |
| r6 | 4 | celeris `daily_curation`・`notify`、task-api `browser_site_policy_db`・`cos_triage` | 元の path のまま（自動 merge） |

- 束へ動かした 2 件は、R5 の base から main までの元の path の差（`+`・`-` の行）と、merge commit が束の file に入れた差（merge commit の第 1 親との `git diff`）を比べ、行が一致した。
- ほかに main は `crates/task-dispatch/src/dispatcher/tests/` の 15 file と src の中の試験を変えた。どれも R5 の束ねの対象ではなく、自動 merge で衝突は無かった。

## `classify.py --write` のやり直し（新しい main で作り、`inventory.json` と比べた）
`classify.py` の `build()` は `cargo metadata` を作業ツリーで走らせ、base の tree と test target の集合が同じでなければ止まる。作業ツリーは束ねた後なので、main の先頭の tree を `git archive` で `$TMPDIR` に出し、`cargo metadata` だけをそこで走らせる一時の Python（repo には置かない）で `build("<main の先頭>")` を呼んだ。`cargo metadata` と ls-tree の集合は一致した（352 件）。

- target の path の集合: 差なし（352 件）。R2・R3 が足した target・名前を変えた target は 0 件。
- base だけで決まる欄（支援 module・fixture・process global・外からの参照・数字・capability）: 束ねた・adjust の target には差が無い。差は keep の 4 件だけで、leaf は変わらない。
  - campaign `preflight_deny_file`: fixture に `fixtures/frozen-manifests/ops-r2-k2.json` が増えた（R3 の probe-flake）。
  - celeris `trial_context_canary_e2e`、task-api `browser_e2e`・`browser_h3_injection`: process global の行番号だけが動いた。
- `capability_key` に差は無い。B0 の capability-map は main でも変わっておらず、`unmapped_*` の束の鍵も変わらない（ADR 2026-11-10-test-target-bundles の「`group:none` の束」の項）。
- 競合（`conflicts`）: R2・R3 の branch はすべて main に入ったので、`main...branch` の差が 0 になり、R2・R3 の競合は 12 target すべてで消えた。R4 の branch には差が無く、競合は 0 件。
- leaf: 競合が消えたことで、adjust-api の 4 件が名前の規則で bundle-api-a に入り、`make_bundles` が task-api の cos-delivery の束を組み直した（bundle-api-a の `cos_delivery_suite_a`・`_b` の 15 件が 19 件の `_a`〜`_c`（7・7・5 件）になり、bundle-api-b の `cos_delivery_suite_c` が `cos_delivery_suite` になる）。この出力を採ると、移動済みの束（bundle-api-a・bundle-api-b）の名前と中身が作業ツリーと食い違い、`--verify-leaf` が落ちる。
- 採った扱い: `inventory.json` は変えない。`conflicts` は生成時（R2・R3 が main に入る前）の記録で、adjust-api・adjust-worker の担当を決めた理由として残す。R3 の競合は main への取り込みで解けたので、adjust-api の 4 件は今の main を基準に束ねてよい（ADR の段 final の表の「R3 の競合が解ければ `cos_delivery_suite_*` へ」）。

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`）
- `git merge-base --is-ancestor main HEAD` → exit 0。
- `cargo check --workspace --tests --keep-going` → exit 0（取り込みの直後。意味の衝突は 0 件。警告は main と同じ内容の `crates/celeris/tests/script_exhaustive.rs` の未使用 import 1 件と vendor の deno_core の 1 件）。
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/classify.py --check agent-docs/progress/2026-11-10-health-v2-r5/inventory.json` → exit 0（352 targets、20 bundles、0 error）。
- `classify.py --verify-leaf <leaf> …inventory.json` → bundle-api-a（18 件移動済み）・bundle-api-b（37 件）・bundle-celeris（15 件）・bundle-ctl（6 件）・keep・r6 のすべてで exit 0（keep・r6 の 263 target が元の path にある、0 error）。
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/cases.py --crate <crate> --base "$(git merge-base HEAD main)"` → 3 crate とも exit 0。
  - task-api: 試験関数 856 → 856、表の行 757 → 757、不変条件の試験 278（42 file）、removals 0 件。
  - celeris: 試験関数 806 → 805、表の行 561 → 561、removals 1 件（dedupe-celeris の記録）、不変条件の試験 321（35 file）。
  - celerisctl: 試験関数 205 → 205、表の行 118 → 118。
- 同じ 3 crate を R5 の base（`--base 2166a26556364c94851dc30f5f6a6cf5ad0b98bc`）で → exit 0（task-api 855 → 856。増えた 1 本は R3 の table-writers が r6 の `cos_triage.rs` に足した `cos_chat_triage_mark_and_audit_commit_together`）。
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/contracts.py --check agent-docs/progress/2026-11-10-health-v2-r5/contracts.json` → exit 0（記録した tree で tests=1864）。
- `bash scripts/dev/test-affected.sh` → exit 0（12 分 9 秒）。対象は workspace の 14 crate すべて（取り込んだ main が task-core などを変えたため）と web・test-ts-verify・scripts/dev の一時領域の試験。nextest は 8213 tests run: 8213 passed（12 slow）、23 skipped。web は tsc -b が通り、vitest 104 file・712 tests passed。test-ts-verify は 20 passed。health gate は verdict=pass（fail_grade 0、new 3・worsened 0）。
- `cargo fmt --all -- --check && cargo clippy --workspace -- -D warnings` → exit 0。
- `git diff --name-only $(git merge-base HEAD main) | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R5 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` → exit 0（「write-set R5: ok」、238 path）。
- `python3 scripts/dev/check-architecture-map.py` → exit 0（993 件）。

## 未解決
- adjust-api・adjust-worker と後の unit は、`classify.py --write` で `inventory.json` を作り直さない。R2・R3 が main に入った後は競合が消え、adjust-api の 4 件が bundle-api-a に入り、移動済みの束の名前が変わる（上の節）。束の名前を変えるときは `inventory.json` の該当の record を手で直し、`--check`・`--verify-leaf` を通す。
- 段 measure の base（`git merge-base HEAD main`）は、この取り込みで R5 の base から main の先頭に移った。base と HEAD の両方に R2・R3 が入るので、R5 の変更だけの前後比較になる。R5 の base で測った `baseline.json` の値とは R2・R3 の分だけ違いうる。
- `scripts/dev/test-affected.sh` の `js_step` は、段が数秒で終わっても「web install が 300 秒で終わらない。止まっている process:」を出すことがある（watchdog の `sleep` を `pkill -P` で止めた後、subshell が `kill` より先に次の行に進む）。今回の log でも install が 879 ms で終わった後に出た。exit code は正しく 0 で、表示だけの問題。この unit の objective の外なので直していない。

## 提案
- 段 record が ADR [2026-11-10-test-target-bundles.md](../../adr/2026-11-10-test-target-bundles.md) の付記「実装との突き合わせ」に次の 1 行を足す（この unit は並行する adjust-api・adjust-worker と ADR の末尾で衝突しないよう、ADR を変えていない）: 「2026-11-11（unit sync-main）: R2・R3 の入った main を取り込んだ。R4 は main に変更が無く、計測に入らない（決定 `r-landed` b）。R2・R3 は test target を足しておらず、名前も変えていない。新しい main で `classify.py --write` をやり直すと、R2・R3 の競合が消えて adjust-api の 4 件が bundle-api-a に入り、移動済みの束の名前が変わるので、`inventory.json` は生成時の `conflicts` のまま変えなかった。記録は `[sync-main.md](../progress/2026-11-10-health-v2-r5/sync-main.md)`（ADR から見た相対 path。ADR に書くときは backtick を外す）。」
- `js_step` の誤った表示は、watchdog の subshell で `sleep` の終わり方（`wait` の終了 status）を見てから表示するように直す。R6 か別の task で行う。
