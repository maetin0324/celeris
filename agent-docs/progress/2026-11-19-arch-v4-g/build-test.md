---
title: アーキ v4 G build-test — after で nextest --no-run の cold と warm（S1・S2）を各 3 回測る
tasks: [01M7MZTKBKXKEVBWS87PS5J9NK]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# build-test（WorkUnit）

after は `git merge-base HEAD main`（この WorkUnit の `$CELERIS_WU_BASE` と同じ tree）。before は A の baseline.md §5.3 の値（test build の表）。
command は `cargo nextest run --workspace --no-run --offline`（全部の test binary を build する。試験は走らせない）。
手順は A §7 C8 の test cold・test warm、TSV の列・valid の規則・probe の戻し方は [rules.md](rules.md) の「## TSV」、file は [scenarios.env](scenarios.env)。
各回の行は [build-test.tsv](build-test.tsv)。

## 0. 結論

- 9 回とも開始時の 1 分 load が 4 以下で valid（測り直しは 0 回）。exit はすべて 0。
- 中央値は 3 組とも A の +10 % 以内だった。compare で印が付く組は無い見込み。

| 計測 | A の中央値 | after の中央値 | 差 | A の +10 % | user の中央値（A → after） |
|---|---|---|---|---|---|
| test build cold | 142.39 s | 147.03 s | +4.64 s（+3.3 %） | 156.63 s | 978.8 → 979.88 s（+0.1 %） |
| test build warm S1 | 138.69 s | 138.10 s | −0.59 s（−0.4 %） | 152.56 s | 754.0 → 754.29 s（+0.0 %） |
| test build warm S2 | 77.64 s | 81.60 s | +3.96 s（+5.1 %） | 85.40 s | 306.3 → 296.62 s（−3.2 %） |

- cold と S2 の wall は A より 3〜5 % 長いが、user の CPU 時間は cold で +0.1 %、S2 で −3.2 % だった。
  after の各回は、直前の自分の build の余波で 5 分 load が 6〜13 の中で始まった（1 分 load は 4 以下）。A の開始時の 5 分 load は記録が無く、
  wall の差が host の状態によるものかコードによるものかは、この値からは分けられない（未確認。cause の leaf が扱う）。
- S2 の 2 回目（84.99 s）は A の +10 %（85.40 s）に 0.41 s まで近い。中央値（81.60 s）は +10 % を超えない。

## 1. 各回の値

| 計測 | run | wall | user | sys | exit | 開始時の load（1・5・15 分） | 待った秒 | 計らない test build（戻しの build） |
|---|---|---|---|---|---|---|---|---|
| test build cold | 1 | 143.36 s | 979.88 s | 166.77 s | 0 | 0.62 / 1.51 / 3.47 | 0 | — |
| test build cold | 2 | 148.53 s | 979.58 s | 166.91 s | 0 | 3.61 / 6.20 / 5.23 | 90 | — |
| test build cold | 3 | 147.03 s | 986.10 s | 166.43 s | 0 | 3.69 / 9.67 / 7.36 | 110 | — |
| test build warm S1 | 1 | 134.76 s | 749.78 s | 114.18 s | 0 | 3.57 / 11.25 / 9.16 | 130 | 2.9 s（cold の後で既に base の状態） |
| test build warm S1 | 2 | 138.10 s | 765.44 s | 112.96 s | 0 | 3.38 / 11.26 / 10.40 | 110 | 135.9 s |
| test build warm S1 | 3 | 141.75 s | 754.29 s | 113.83 s | 0 | 3.41 / 12.68 / 12.00 | 130 | 140.3 s |
| test build warm S2 | 1 | 81.60 s | 308.33 s | 57.66 s | 0 | 3.77 / 12.96 / 12.93 | 130 | 131.7 s（S1 の戻し） |
| test build warm S2 | 2 | 84.99 s | 296.26 s | 57.22 s | 0 | 3.91 / 11.96 / 12.89 | 120 | 76.7 s |
| test build warm S2 | 3 | 80.95 s | 296.62 s | 57.47 s | 0 | 3.83 / 10.06 / 12.09 | 80 | 80.3 s |

- 測った時刻は 2026-11-19T05:51Z〜06:35Z（UTC）。host home-dev、24 core、cargo 1.98.1 / rustc 1.98.1 / cargo-nextest 0.9.146。
- `CARGO_TARGET_DIR` は Celeris が渡した値、`CARGO_INCREMENTAL=0`・`CARGO_PROFILE_DEV_DEBUG=line-tables-only`、rustc-wrapper は host の設定
  （`~/.cargo/config.toml` の `rustc-wrapper.sh`。build-check と同じ。したがって cold は「target dir が空で compiler cache は host のまま」）。
- 全 9 回の `src_tree`（`git rev-parse HEAD:crates` の tree の id。値は TSV の列）は scenarios.env の `AFTER_CRATES_TREE` と一致した（script は不一致なら log に書く。書かれなかった）。

## 2. A と条件が違う点

- S1 の file は 4 個（`org_write.rs` を足した。選び直しの理由は [scope.md](scope.md) §1）。A は 3 個。再 build される crate の組は A と同じ（scope.md §2）。
- A は test build の開始時の load を 1 分値だけ記録した。after は 1・5・15 分の 3 値を TSV の note に残した。
- A の warm の待ち時間は 70〜130 s、after は 80〜130 s（cold は 0〜110 s）。

## 3. probe の戻し

- 各 warm の回は `git checkout -- <file>` → 計らない test build（同じ command）→ `raw/probe-const.sh <file>`（各 file の末尾に `pub const SCENARIO_PROBE_<k>`）
  → waitload → 計測 → `git checkout -- <file>` の順で行った。測り終えた後の `git status --porcelain -- crates` は空だった。

## 4. 再現コマンド

script は run の成果物 `artifacts/raw/build-test.sh`（build-check の `build-check.sh` の `cargo check --workspace --offline -q` を
`cargo nextest run --workspace --no-run --offline` に置き換えたもの。戻しの build も同じ command）。中身の要点:

```sh
waitload() { s=0; while awk '{exit !($1>4)}' /proc/loadavg && [ $s -lt 600 ]; do sleep 10; s=$((s+10)); done; }
. agent-docs/progress/2026-11-19-arch-v4-g/scenarios.env
# cold（3 回）
cargo clean; waitload; /usr/bin/time -f '%e %U %S %M' cargo nextest run --workspace --no-run --offline
# warm（FILES は $S1_FILES か $S2_FILES、3 回ずつ）
git checkout -- $FILES; cargo nextest run --workspace --no-run --offline
k=0; for f in $FILES; do k=$((k+1)); printf '\n#[allow(dead_code)]\npub const SCENARIO_PROBE_%s: u64 = %s;\n' $k $(date +%s%N) >> $f; done
waitload; /usr/bin/time -f '%e %U %S %M' cargo nextest run --workspace --no-run --offline; git checkout -- $FILES
```

実行の順: `build-test.sh cold` → `build-test.sh warm S1` → `build-test.sh warm S2`（環境変数 `TSV`・`OUT`・`DEADLINE`）。
生の log（各回の `/usr/bin/time` の出力・cargo の出力・`timing-raw.log`）は run の成果物 `artifacts/raw/` に置いた（repo には置かない）。
