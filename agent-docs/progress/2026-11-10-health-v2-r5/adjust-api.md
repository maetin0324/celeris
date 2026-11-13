---
title: 健全性v2 R5 adjust-api — R3 と競合して残した task-api の CoS 試験 4 件の束ね
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
status: done
updated: 2026-11-11
completed: 2026-11-11
---
# 健全性v2 R5 adjust-api — R3 と競合して残した task-api の CoS 試験 4 件の束ね

完了日: 2026-11-11

WorkUnit `adjust-api`（段 final）。`inventory.json` で leaf が `adjust-api` の task-api の 4 target を、ADR [2026-11-10-test-target-bundles.md](../../adr/2026-11-10-test-target-bundles.md) の「束ねの手順」で 1 つの束 `crates/task-api/tests/cos_delivery_suite/` にした。試験関数の中身は変えていない。基準は `$CELERIS_WU_BASE`（sync-main の後の HEAD と同じ tree）と `git merge-base HEAD main`。

## 結論
- 4 target（cos_operations_domains・cos_ops_admin_config・cos_ops_projects_cron・project_auto_integrate）を `cos_delivery_suite` に移した。task-api の test binary は 68 → 65、試験は 879 → 879 で、(target 名::試験名) の組は base と HEAD で一致した。
- 束の試験 19 件は、nextest（試験ごとに別 process）と `cargo test --test cos_delivery_suite`（1 process）の両方で通った。D3 で束から外す target は無かった。
- 決定 `r-landed`（b、今の main で始める）のとおり、R2・R3 は取り込んだ main に入っていて、R4 は入っていない。この leaf の 4 件は R3 との競合で残したもので、R3 は main に入ったので今の main を基準に束ねた。

## 束
| 束 | 件数 | capability group | fixture | target |
|---|---|---|---|---|
| `cos_delivery_suite` | 4 | cap-cos-delivery | common | cos_operations_domains・cos_ops_admin_config・cos_ops_projects_cron・project_auto_integrate |

- 束の名前は `inventory.json` の 4 件の `bundle` 欄の `cos_delivery_suite`。束の鍵（D1）は crate・leaf・capability group・fixture なので、同じ cap-cos-delivery・common でも leaf の違う bundle-api-a・bundle-api-b の `cos_delivery_suite_a`〜`_c` とは別の束になる。名前は既存の target 名・`tests/` の dir 名と重ならない。
- D2 の上限の内: 4 target（上限 8）、束の本体の合計 1450 行（上限 8000）、最大の file は `cos_ops_admin_config.rs` の 619 行（上限 2000）。

## 実装前 brief
1. 根本原因: 同じ fixture（`tests/common/mod.rs` と `common/cos_ops.rs`）を使う CoS 操作の試験 4 件が別々の target で、target ごとに common と依存を link している。段 bundle では R3（task 01M6TYVJT2HG9NY5FW9FMCZS9A）が同じ file を変えていたので動かさなかった。
2. 責務 owner: `crates/task-api/tests` の target 構成。束と leaf の正本は `inventory.json`。
3. 既存の再利用先: `tests/common/mod.rs`、bundle-api-a の束の形（`#[path = "../common/mod.rs"] mod common;` と各 file の `use crate::common;`）、`classify.py --verify-leaf`・`cases.py`。
4. 選択肢: (a) `inventory.json` の `bundle` 欄のとおり 4 件で `cos_delivery_suite` を作る — 採った。(b) ADR の段 final の表の「R3 の競合が解ければ `cos_delivery_suite_*` へ」に従い、既存の `_a`（8 件）・`_b`（7 件）・`_c`（8 件）に足す — 採らない。足すと上限 8 を超えるので束を組み直す必要があり、bundle-api-a・bundle-api-b（完了済みの他の leaf）の target を動かし、`inventory.json` の record を書き換えることになる。sync-main も `inventory.json` を作り直さないと決めた（[sync-main.md](sync-main.md) の「未解決」）。
5. 新しい状態・分岐・fallback・port: 無し。
6. 撤去対象: 4 target の元の path（`crates/task-api/tests/<target>.rs`）と、各 file の `mod common;` の宣言（束の `main.rs` に 1 回だけ置く）。
7. 不変条件: 試験関数・表の case・不変条件の試験が base と同じ（`cases.py`）。keep・r6 と数字を含む file は元の path に残る（`classify.py --verify-leaf`）。束の試験は 1 process でも通る（`cargo test --test cos_delivery_suite`）。

## 変更
- 件名「phase 1: adjust-api: task-api の cos_operations_domains・cos_ops_admin_config・cos_ops_projects_cron・project_auto_integrate を cos_delivery_suite に束ねる」の commit — 4 file を `git mv` で `tests/cos_delivery_suite/` へ移し、`main.rs` を作った。各 file の差は `mod common;` → `use crate::common;` の 1 行だけ（base の中身と Python の difflib で比べた）。
- 件名「phase 1: adjust-api: sync-main.md の ADR 向けの相対リンクを inline code にして doc.links の壊れた参照を直す」の commit — [sync-main.md](sync-main.md) の「提案」の ADR に足す文の中のリンク（ADR から見た相対 path `../progress/…`）が、sync-main.md の位置からは壊れていて、`check-health.sh` の `doc.links`（fail-grade）で落ちた。base（sync-main の 2 つめの commit）にある不備で、この leaf の束ねとは関係ない。文の意味は変えず、リンクを inline code にし、ADR に書くときは backtick を外すと書き添えた。
- この file。

## 証拠（提出時の HEAD で実行）
- 条件 0: `python3 agent-docs/progress/2026-11-10-health-v2-r5/classify.py --verify-leaf adjust-api agent-docs/progress/2026-11-10-health-v2-r5/inventory.json` → exit 0。「4 moved target(s) of the leaf, 263 keep/r6 target(s) checked in place, 0 error(s)」。変更の前は 12 error（4 件とも元の path にある）で落ちた。
- 同じ `--verify-leaf` を bundle-api-a（18 件）・bundle-api-b（37 件）・bundle-celeris（15 件）・bundle-ctl（6 件）で → 各 exit 0、0 error。
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/classify.py --check agent-docs/progress/2026-11-10-health-v2-r5/inventory.json` → exit 0（352 targets、20 bundles、0 error）。inventory は変えていない。
- 条件 1: `python3 agent-docs/progress/2026-11-10-health-v2-r5/cases.py --crate task-api --base "$CELERIS_WU_BASE"` → exit 0。試験関数 856 → 856、表の行 757 → 757、名前で追った移動 19、不変条件の試験 230（33 file）、removals 0 件。`--base "$(git merge-base HEAD main)"` でも exit 0。
- `cargo nextest list -p task-api --message-format json` → base・HEAD とも exit 0。binary 68 → 65、試験 879 → 879、(target 名::試験名) の組は一致（base だけ・HEAD だけは 0 件）。base は同じ worktree で `crates/task-api/tests` を `$CELERIS_WU_BASE` の tree に戻して取り、取った後に `git reset --hard HEAD` で戻した。
- `cargo nextest run -p task-api -E 'binary_id(task-api::cos_delivery_suite)'` → exit 0、19 passed、0 skipped。
- `cargo test -p task-api --test cos_delivery_suite`（1 process の中で並行に実行）→ exit 0、19 passed。
- `bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE"` → exit 0（verdict=pass、fail_grade 0、new 0、worsened 0）。sync-main.md のリンクを直す前は `doc.links` の 1 件で exit 1 だった。
- 条件 1: `bash scripts/dev/test-affected.sh` → exit 0（963 秒。この file を足した commit の HEAD で実行。その前の束ねの commit の HEAD でも exit 0、694 秒）。対象は celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker と web。nextest 5872 tests run: 5872 passed（27 slow）、22 skipped。web は tsc -b が通り、vitest 104 file・712 tests passed。health gate は verdict=pass（fail_grade 0、worsened 0）、「all checks passed」。
- 条件 1: `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0（出力なし）。
- `python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R5 agent-docs/progress/2026-11-07-health-v2-b0/slices.json`（`git diff --no-renames --name-only "$CELERIS_WU_BASE" HEAD` と未追跡 path を標準入力）→ exit 0、「write-set R5: ok」。`sh "$CELERIS_WU_SCOPE_PATHS"` の出力は束の 4 file・`main.rs`・sync-main.md・この file だけ。
- `sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-adr-numbers.sh`・`sh scripts/dev/progress-index.sh --check` → 各 exit 0。

## 未解決
- B0 の `capability-map.json`・`capability-map/cap-cos-delivery.json` は `project_auto_integrate` の path を元の `crates/task-api/tests/project_auto_integrate.rs` で記録している。B0 の記録は R5 の write set の外で、段 bundle で動かした他の target（`cos_auth`・`routing` など）と同じく書き換えていない。`cases.py` は base の tree に path があるかで照合するので、`git merge-base HEAD main` を base にする統合でも通る（note が 1 行増えるだけ）。
- adjust-worker も同じ base から始まるので、sync-main.md の壊れたリンクを別の形で直すと integrate-final で衝突しうる（未確認）。衝突したらこの branch の形（inline code）に合わせればよい。

## 提案
- 段 record が ADR の付記「実装との突き合わせ」に次の 1 行を足す（この unit は並行する adjust-worker と ADR の末尾で衝突しないよう、ADR を変えていない。リンクは ADR から見た相対 path なので、ここでは inline code にした）: 「2026-11-11（unit adjust-api）: R3 の入った main を基準に、task-api の adjust-api の 4 件を `inventory.json` の `bundle` 欄のとおり 4 件の `cos_delivery_suite` に束ねた。段 final の表の『`cos_delivery_suite_*` へ』は採らなかった。既存の `_a`〜`_c` は 8・7・8 件で、足すと上限 8 を超えて完了済みの bundle-api-a・bundle-api-b の束を組み直すことになるため。記録は `[adjust-api.md](../progress/2026-11-10-health-v2-r5/adjust-api.md)`。」
