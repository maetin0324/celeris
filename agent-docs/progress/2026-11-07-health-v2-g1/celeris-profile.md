---
title: 健全性v2 G1 celeris-profile — Celeris profile と check-health.sh への接続
tasks: [01M6TYVJSRM3SFQ3VHWQE4MZ9M]
status: done
updated: 2026-11-07
completed: 2026-11-07
---
# 健全性v2 G1 celeris-profile — Celeris profile と check-health.sh への接続

完了日: 2026-11-07

WorkUnit `celeris-profile`。基準は `$CELERIS_WU_BASE`。決定は [ADR 2026-11-07 repo health の Celeris profile と単一入口](../../adr/2026-11-07-repo-health-celeris-profile.md)。

## やったこと
- `scripts/dev/check-health.sh` を repo health の単一入口にした: `repo_health.py --base auto --head worktree` の report JSON を `--out`（既定 `$TMPDIR/check-health.<pid>.json`）に書き、必須 rule が pass 以外なら非 0（1 fail・2 error/欠測）。合格なら検査器の unit 試験（test_architecture_map・test_repo_health*）とゲートの試験を走らせる。doc-only では省く。
- root・base の解決を `health/snapshot.py` の `default_base`（`--base auto`・`--print-base`）1 か所にし、6 本の script から `git rev-parse --show-toplevel`・`__file__` 由来の root・`CELERIS_WU_BASE`・`git merge-base HEAD main` を消した（`--root`・`--base` で受け取る）。
- `rules/existing_checks.py`: doc.links・adr.numbers・progress.index・human-text・source-size を profile の `commands` で走らせる rule。script の自己試験も同じ rule の command に並べた。
- `profiles/celeris.toml`: policy_paths（包んだ script・baseline・capability-map.json・source-size-report を追加）、exclude_paths（vendor）、crate の層 6 段と許可依存、size の上限、構造指標 3 件、例外 2 件（理由付き）、既存検査の command。`profiles/default.toml` は `extends = "celeris"`。
- profile の項目 `[metrics."<name>"]` と report の `metrics.structural`、fail 級 finding の stdout 表示を足した。
- `scripts/dev/tests/repo_health_gate.sh`（新規）、`scripts/tests/test_architecture_map.py`・`scripts/dev/tests/progress_index_commit_hashes.sh` を新しい入口に合わせた。

## 証拠（提出時の HEAD で実行）
- `bash scripts/dev/check-health.sh --out <artifacts>/check-health-report.json` → exit 0（verdict pass、21 rule、findings 717・fail 級 0・new 43・existing 674、policy=bootstrap）
- `sh scripts/dev/tests/repo_health_gate.sh` → exit 0（(1) pass→0、(2) 必須 rule の fail→1、(3) shallow→2、(4) profile の弱化・検査 script の弱化→1、(5) doc-only は構造の検査と unit 試験を起こさず 0）
- `repo_health.py --base auto --head HEAD --rules doc.links,adr.numbers,progress.index,human-text,source-size` を 2 回 → 両方 exit 0、report は byte 一致、ref と `.git/config` の md5 は前後で同じ
- `bash scripts/dev/test-affected.sh` → exit 0（check-health 込み。crate の試験の対象なし）
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0
- `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` → exit 0

## 未解決
- 最初の実装は commit の head で command に `GIT_DIR`・`GIT_WORK_TREE` を渡し、command の中の試験（progress_index_commit_hashes.sh）が共有の `.git/config` に `core.worktree` を書き、この branch に commit「base」と branch `task` を作った。その場で `core.worktree` を消し、branch を戻し、`task` を消した（ADR D2 の直し）。
- この branch の check-health は main に profile が無いので bootstrap（policy.changed は warning）で合格する。main に入った後は、policy の file を変える branch の check-health（と test-affected）が policy.changed で落ちる。その branch をどう通すか（policy の別 review）は G2 で決める。
