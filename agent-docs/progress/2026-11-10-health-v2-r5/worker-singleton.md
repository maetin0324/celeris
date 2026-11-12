---
title: R5 worker-singleton — 1 件だけの束 browser_action_recovery を元の test target に戻す
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
status: done
updated: 2026-11-12
---
# R5 worker-singleton — 1 件だけの束 browser_action_recovery を元の test target に戻す

完了日: 2026-11-12

結論: adjust-worker が作った 1 件だけの束 `browser_eval_surface_browser_action_recovery_py_suite` を消し、
`crates/task-worker/tests/browser_action_recovery.rs` を元の test target に戻した。
戻した file は inventory の base_sha の file と同じ内容（`git diff <inventory の base_sha> -- <path>` の差が 0）。
inventory は keep（理由 `singleton_bundle_group:adjust-worker`）にした。

## 実装前 brief
1. 根本原因: classify.py の束ねの規則は、bundle-* の 1 件だけの組を keep にするが、adjust-* の 1 件だけの組は
   「段 final で組み直す」として残す。adjust-worker はこの組をそのまま 1 件の束にした。1 件の束は binary の数を減らさず、
   ADR D4-1（1 件だけの組は keep）に反する。
2. 責務 owner: R5 の束ねの実装（`crates/task-worker/tests/` の束の dir）と inventory（`inventory.json` の leaf・bundle）。
3. 既存の再利用先: classify.py の `--check` と `--verify-leaf`、cases.py（試験関数の数の比較）、
   adjust-worker が ssh_cluster_manual を keep にした時の理由の書き方（`task_worker_bundled_after_r2` の後に keep の理由を足す）。
4. 選択肢: (a) 他の task-worker の束（`browser_eval_surface_suite`）に混ぜる → fixture_key が違い、束ねの鍵
   （crate + leaf + capability_key + fixture_key）に反するので採らない。(b) 元の target に戻して keep にする → 採用。ADR D4-1 どおり。
5. 新しい状態・分岐・fallback・port: 無し。
6. 撤去対象: 束の `main.rs` と dir、inventory の bundle 名。
7. 不変条件: 試験関数と assert は変えない（cases.py で task-worker の試験関数 1294 → 1294、表の行 1106 → 1106）。
   Python の fixture は動かさない。keep の target は元の path にある（`--verify-leaf` が確かめる）。

## 変更
- `phase 1: worker-singleton: 1 件だけの束 browser_action_recovery を元の test target に戻す` —
  - `git mv` で `crates/task-worker/tests/browser_eval_surface_browser_action_recovery_py_suite/browser_action_recovery.rs`
    を `crates/task-worker/tests/browser_action_recovery.rs` に戻した。
  - 束の `main.rs`（`mod browser_action_recovery;` の 1 行）と dir を消した。
  - `include_str!("../browser_action_recovery.py")` を `include_str!("browser_action_recovery.py")` に戻した。試験関数と assert は変えていない。
  - `crates/task-worker/tests/browser_action_recovery.py` は動かしていない。
  - `inventory.json` の browser_action_recovery を leaf `keep`、leaf_reasons `task_worker_bundled_after_r2`・`singleton_bundle_group:adjust-worker`、
    bundle `null` にし、summary の task-worker を adjust-worker 6 → 5、keep 15 → 16 にした。
- task-worker の test target は 17 のまま（束が 2 → 1、元の target が 1 本戻る）。1 件の束は binary を減らしていなかった。全体は 285（`python3 scripts/dev/test-target-inventory.py --json` の totals.test_targets）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/classify.py --check agent-docs/progress/2026-11-10-health-v2-r5/inventory.json` → exit 0（352 targets、17 bundles、0 error）
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/classify.py --verify-leaf adjust-worker agent-docs/progress/2026-11-10-health-v2-r5/inventory.json` → exit 0（moved 5、keep/r6 267 を元の path で確認、0 error）
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/cases.py --crate task-worker --base $(git merge-base HEAD main)` → exit 0（試験関数 1294 → 1294、removals 0 件）
- `python3 -m unittest discover -s agent-docs/progress/2026-11-10-health-v2-r5 -p test_classify.py` → OK（16 件）
- `cargo test -p task-worker --test browser_action_recovery` → exit 0（1 passed）
- 検査の結果は下の「検査」に書く。

## 検査

## 未解決
- ADR `2026-11-10-test-target-bundles.md` の表と adjust-worker.md の束の記述は、並行する WU adr-sync が直す。この WU では触っていない。
- classify.py の docstring と make_bundles は、adjust-* の 1 件だけの組を残す規則のまま。inventory は手で keep にした
  （adjust-worker が ssh_cluster_manual などを keep にした時と同じ扱い）。`--write` で作り直すと、この target は adjust-worker に戻る。

## 提案
- classify.py の make_bundles で、adjust-* の 1 件だけの組も keep（`singleton_bundle_group:<leaf>`）にする。段 final は済んだので、
  「段 final で組み直す」例外は要らない。直せば inventory と生成器の分類が一致する。
