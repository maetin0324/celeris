---
title: アーキ v4 H R3 adr — 4 file の参照の棚卸し・数え方・review_verdict.rs と work_units.rs の決定
tasks: [01M7THYAPKESR1BG6BQZPJ749P]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# adr（WorkUnit）

完了日: 2026-11-19

## 変更

- `arch-v4 H R3 adr: …`（件名で参照。hash は書かない）— [ADR 2026-11-19 dispatcher-narrow-ops-round2](../../adr/2026-11-19-dispatcher-narrow-ops-round2.md) を足した
  （節: 4 file の参照の表・file ごとの減る見込み・数え方と数えないもの・review_verdict.rs と work_units.rs の設計と決定・反例試験と領域の試験・撤去対象・実装前 brief）。
- [進捗](../2026-11-19-arch-v4-h-r3.md) を status: running で作った。
- `crates/`・`scripts/`・`web/` は変えていない。

## 棚卸しの方法

- 参照の一覧: 4 file に `self\s*\.\s*(store|config)\b` を当て、行・囲む関数・呼ぶ method を出した（E1 と同じ正規表現。C3 は 1 行の中の字面）。
- 置き換え先の候補: `task_ops` の `tree`・`tree_plan`・`plan_gate`・`delegate`・`human_inbox`・`root_done` の pub 関数と、private 関数の呼び出し元
  （`git grep -n "\.<名>("`）を調べた。候補の 3 つ（`plan_children`・`open_decision`・task-api の `goal_guard`）は入力か意味が違った。
- 設定の書き換え: `cos_chat/` に `self.config =` は無い（1 回の呼び出しの中で設定の値は変わらない）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE` と `git merge-base HEAD main`）

- C3 を `git merge-base HEAD main` に当てた → 非試験 69 file、self.store 38 file / 633、self.config 31 file / 220。
  `git diff --stat $(git merge-base HEAD main) HEAD -- crates scripts web` → 空。
- 対象 4 file と参考 2 file の C3・E1（HEAD の作業ツリー）→ tree_units 56/19・63/20、root_done 16/5・22/6、triage 13/4・24/4、fallback 13/9・14/9、
  review_verdict 32/3・33/3、work_units 58/14・65/15（ADR の背景の表）。
- 反例試験 12 本の存在: `git grep -l "fn <名>\b" -- crates/task-dispatch` → 全部見つかった（cex_lease_restart.rs 8・cex_review_authority.rs 1・cex_late_completion.rs 3）。

## 検査

検査の結果は下に追記する。
