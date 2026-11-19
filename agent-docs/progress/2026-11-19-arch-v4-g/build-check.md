---
title: アーキ v4 G build-check — after で cargo check の cold と warm（S1・S2）を各 3 回測る
tasks: [01M7MZTKBKXKEVBWS87PS5J9NK]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# build-check（WorkUnit）

after は `git merge-base HEAD main`（この WorkUnit の `$CELERIS_WU_BASE` と同じ tree）。before は A の baseline.md §5.3 の値。
手順は A §7 C8、TSV の列・valid の規則・probe の戻し方は [rules.md](rules.md) の「## TSV」、file は [scenarios.env](scenarios.env)。
各回の行は [build-check.tsv](build-check.tsv)。

## 0. 結論

- 9 回とも開始時の 1 分 load が 4 以下で valid（測り直しは 0 回）。exit はすべて 0。
- 中央値は A の +10 % 以内だった。compare で印が付く組は無い見込み。

| 計測 | A の中央値 | after の中央値 | 差 | A の +10 % | user の中央値（A → after） |
|---|---|---|---|---|---|
| check cold | 53.65 s | 53.90 s | +0.25 s（+0.5 %） | 59.02 s | 216.0 → 216.48 s（+0.2 %） |
| check warm S1 | 42.47 s | 42.64 s | +0.17 s（+0.4 %） | 46.72 s | 58.7 → 59.15 s（+0.8 %） |
| check warm S2 | 14.67 s | 15.56 s | +0.89 s（+6.1 %） | 16.14 s | 18.9 → 18.92 s（+0.1 %） |

- S2 の wall は +6.1 % だが、user の CPU 時間は +0.1 % で、各回の幅（14.62〜15.75 s）は A の幅（14.63〜15.41 s）と重なる。
  コードの差（E は `work_units.rs` を +51 −58 行変えた）による悪化は、この値からは読み取れない（未確認。cause の leaf が扱う）。
- S1 は D の warm check の中央値（4 file 43.95 s）より 1.31 s 速かった。A の +10 %（46.7 s）を超えないので、D の申し送りの「原因を絞る」条件に当たらない。

## 1. 各回の値

| 計測 | run | wall | user | sys | exit | 開始時の load（1・5・15 分） | 待った秒 | 計らない check（戻しの build） |
|---|---|---|---|---|---|---|---|---|
| check cold | 1 | 54.90 s | 216.84 s | 50.12 s | 0 | 1.89 / 2.99 / 5.57 | 0 | — |
| check cold | 2 | 53.52 s | 215.82 s | 49.77 s | 0 | 3.80 / 3.45 / 5.58 | 0 | — |
| check cold | 3 | 53.90 s | 216.48 s | 50.18 s | 0 | 3.66 / 3.56 / 5.51 | 0 | — |
| check warm S1 | 1 | 42.75 s | 59.21 s | 5.87 s | 0 | 3.61 / 3.85 / 5.48 | 10 | 1.2 s（cold の後で既に base の状態） |
| check warm S1 | 2 | 42.64 s | 59.15 s | 5.77 s | 0 | 2.89 / 3.54 / 5.23 | 0 | 43.2 s |
| check warm S1 | 3 | 42.64 s | 58.96 s | 6.03 s | 0 | 2.15 / 3.14 / 4.94 | 0 | 43.1 s |
| check warm S2 | 1 | 15.75 s | 18.94 s | 2.17 s | 0 | 2.04 / 2.83 / 4.66 | 0 | 43.1 s（S1 の戻し） |
| check warm S2 | 2 | 14.62 s | 18.82 s | 2.18 s | 0 | 1.77 / 2.68 / 4.55 | 0 | 14.6 s |
| check warm S2 | 3 | 15.56 s | 18.92 s | 2.17 s | 0 | 1.81 / 2.60 / 4.46 | 0 | 14.7 s |

- 測った時刻は 2026-11-19T05:38Z〜05:46Z（UTC）。host home-dev、24 core、cargo 1.98.1 / rustc 1.98.1（A と同じ版）。
- `CARGO_TARGET_DIR` は Celeris が渡した値、`CARGO_INCREMENTAL=0`・`CARGO_PROFILE_DEV_DEBUG=line-tables-only`、rustc-wrapper は host の sccache（A と同じ。
  したがって cold は「target dir が空で sccache の cache は host のまま」）。
- 全 9 回の `src_tree`（`git rev-parse HEAD:crates`）は `a4f737a01c6040dd1761648a19c6aaa38c1383e0` で、scenarios.env の `AFTER_CRATES_TREE` と一致した。

## 2. A と条件が違う点

- S1 の file は 4 個（`org_write.rs` を足した。選び直しの理由は [scope.md](scope.md) §1）。A は 3 個。4 個とも task-api か task-core の file で、
  再 build される crate の組は A と同じ（scope.md §2）。
- A は warm の開始時の load を記録していない（A の baseline.md §1）。after は各回の直前に A §7 C8 の waitload を通した。
- A の cold は attempt 1 の run 3（開始時 4.02）を捨てて測り直した。after は捨てた回が無い。

## 3. probe の戻し

- 各 warm の回は `git checkout -- <file>` → 計らない check → `raw/probe-const.sh <file>`（各 file の末尾に `pub const SCENARIO_PROBE_<k>`）→ waitload
  → 計測 → `git checkout -- <file>` の順で行った。測り終えた後の `git status --porcelain -- crates` は空だった。

## 4. 再現コマンド

script は run の成果物 `artifacts/raw/build-check.sh`（A §7 C8 の手順をそのまま書いたもの）。中身の要点:

```sh
waitload() { s=0; while awk '{exit !($1>4)}' /proc/loadavg && [ $s -lt 600 ]; do sleep 10; s=$((s+10)); done; }
. agent-docs/progress/2026-11-19-arch-v4-g/scenarios.env
# cold（3 回）
cargo clean; waitload; /usr/bin/time -f '%e %U %S %M' cargo check --workspace --offline -q
# warm（FILES は $S1_FILES か $S2_FILES、3 回ずつ）
git checkout -- $FILES; cargo check --workspace --offline -q
k=0; for f in $FILES; do k=$((k+1)); printf '\n#[allow(dead_code)]\npub const SCENARIO_PROBE_%s: u64 = %s;\n' $k $(date +%s%N) >> $f; done
waitload; /usr/bin/time -f '%e %U %S %M' cargo check --workspace --offline -q; git checkout -- $FILES
```

生の log（各回の `/usr/bin/time` の出力・cargo の出力・`timing-raw.log`）は run の成果物 `artifacts/raw/` に置いた（repo には置かない）。
