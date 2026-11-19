---
title: アーキ v4 G control-build — 印の付いた health:S2 の before を同じ host で 3 回測り直す
tasks: [01M7MZTKBKXKEVBWS87PS5J9NK]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# control-build（WorkUnit）

[flags.tsv](flags.tsv) で `flag=control` の check_*・test_build_*・health:* の組は **health:S2 の 1 組だけ**（compare.md §0）。この組の before
（main bc665168、A の検査器 × A の tree）を、A の手順（baseline.md §5.5・§7 C8）で同じ host（home-dev、24 core）で 3 回測り直した。
手順は [rules.md](rules.md) の「## BEFORE」（この leaf が足した）、TSV の列と valid の規則は同じ file の「## TSV」。
各回の行は [control-build.tsv](control-build.tsv)（`rev=before`）。同じ時間帯の after の参考の 3 回は [control-build-after.tsv](control-build-after.tsv)。

## 0. 結論

- before の 3 回は 87.26・107.83・86.19 s、**中央値 87.26 s**。A の中央値 81.92 s より **+5.34 s（+6.5 %）**。3 回とも valid
  （開始時の 1 分 load 0.75・1.47・2.71、待った秒 0）。user の CPU 時間の中央値は 68.81 s で、A（69.46 s）の −0.9 %。
  同じコード・同じ検査器でも、いまの host では wall が A より 5〜6 % 長い（2 回目は 107.83 s の外れ値）。これが host の変化の大きさ。
- 同じ時間帯（各 before の回の直後）に after を測った参考の 3 回は 88.38・93.50・88.89 s、中央値 88.89 s。before の中央値より +1.63 s（+1.9 %）、
  user は +2.09 s（+3.0 %）。A の中央値との比は 1.085 で、印の閾値（A の +10 % = 90.11 s）を下回った。
- compare が印を付けた差（scope の after の中央値 96.09 s − A の 81.92 s = +14.17 s）は、次の 3 つの和になる（値の分け方だけ。原因の判定は cause の leaf）:
  - host の変化（before のいま − A）: +5.34 s
  - コードの変化（同じ時間帯の after − before）: +1.63 s
  - after を測った時間帯の差（scope の 05:20Z〜05:30Z の after 96.09 s − いまの after 88.89 s）: +7.20 s
- before の findings は A と同じ（702 件、new 5・worsened 2・existing 695、fail の行は head の sha を除いて A の health-S2-1.out と同じ）。
- 測り終えた後に before の worktree を消した。`git worktree list` に残っておらず、`git status --porcelain -- crates` と
  `git diff --stat "$CELERIS_WU_BASE" -- crates` は空。

## 1. health:S2 の before の各回（A の値との差）

command（before の worktree を cwd にして）: `python3 scripts/dev/repo-health/repo_health.py --repo . --base bc665168c9caf1898adc4c611404bbee056daf9a --head worktree --policy-version 1 --out <json>`、
S2 の 4 file に const の probe（A §7 C8）を足した状態。`/usr/bin/time -f '%e %U %S %M'`。測った時刻は 2026-11-19T08:31Z〜08:40Z。

| 回 | wall | A の中央値 81.92 s との差 | user | sys | wall − user − sys | max RSS | 開始時の load（1・5・15 分） | exit | valid |
|---|---|---|---|---|---|---|---|---|---|
| 1 | 87.26 s | +5.34 s（+6.5 %） | 69.28 s | 8.39 s | 9.59 s | 611.7 MB | 0.75・1.56・4.49 | 1 | yes |
| 2 | 107.83 s | +25.91 s（+31.6 %） | 68.70 s | 8.45 s | 30.68 s | 611.7 MB | 1.47・1.58・3.99 | 1 | yes |
| 3 | 86.19 s | +4.27 s（+5.2 %） | 68.81 s | 8.24 s | 9.14 s | 611.6 MB | 2.71・2.10・3.71 | 1 | yes |
| **中央値** | **87.26 s** | **+5.34 s（+6.5 %）** | 68.81 s（A 69.46 s、−0.9 %） | 8.39 s（A 8.14 s） | 9.59 s | | | | |
| A の 3 回 | 81.76・81.92・85.96 s | — | 69.42・69.76・69.46 s | 8.14・8.05・8.24 s | 4.20・4.11・8.26 s | 約 611 MB | 1 分 0.3〜1.3（A §1） | 1 | — |

- exit 1 は const の probe で fail になる手順どおりの値（rules.md）。fail の rule は 3 回とも `size.responsibility-growth`・`source-size`（A と同じ）。
- user・sys・max RSS は A とほぼ同じ（user −0.9 %、RSS の差 0.1 MB 以下）。wall の差は wall − user − sys（CPU を使っていない待ちの時間）に出た:
  A の 4.1〜8.3 s に対し、いまは 9.1〜9.6 s、2 回目は 30.68 s。2 回目の原因（I/O の待ちか、ほかの process との CPU の取り合いか）は未確認。

## 2. 同じ時間帯の after の参考（control-build-after.tsv）

各 before の回の直後に、この WorkUnit の worktree（crates/ の tree は scenarios.env の `AFTER_CRATES_TREE` と同じ）で
`--base HEAD --head worktree`（after の検査器、S2 の 4 file に const の probe）を 1 回ずつ測った（B→A を 3 組）。

| 回 | wall | user | sys | wall − user − sys | max RSS | 開始時の load（1・5・15 分） | valid |
|---|---|---|---|---|---|---|---|
| 1 | 88.38 s | 70.75 s | 8.45 s | 9.18 s | 645.8 MB | 1.35・1.56・4.21 | yes |
| 2 | 93.50 s | 71.62 s | 8.92 s | 12.96 s | 646.0 MB | 2.54・1.83・3.80 | yes |
| 3 | 88.89 s | 70.90 s | 8.49 s | 9.50 s | 646.1 MB | 1.92・2.06・3.55 | yes |
| 中央値 | **88.89 s** | 70.90 s | 8.49 s | 9.50 s | | | |

| 比べる組 | wall の中央値の差 | user の中央値の差 | max RSS |
|---|---|---|---|
| after（同じ時間帯）− before（いま） | +1.63 s（+1.9 %） | +2.09 s（+3.0 %） | +34 MB |
| after（同じ時間帯）− A | +6.97 s（+8.5 %） | +1.44 s（+2.1 %） | +35 MB |
| after（同じ時間帯）− after（scope、05:20Z〜05:30Z） | −7.20 s（−7.5 %） | −0.96 s（−1.3 %） | 同じ |

- after の参考の findings は 699 件（new 4・worsened 2・existing 693）で、scope の after（700 件、new 5）より `link.unlinked` の new が 1 件少ない。
  scope.md（HEAD に入っている）の本文に `SCENARIO_PROBE_1` の名前が書かれているので、probe の名前が「定義の 1 回しか出てこない」に当たらなくなった。
  `--base` も scope の after の rev（`git merge-base HEAD main`）でなくこの WorkUnit の HEAD（G の進捗の file だけが違う。crates/ の tree は同じ）にした。
  fail の rule（`size.responsibility-growth`・`source-size`）と worsened 2 は scope と同じ。
- scope の after は 5 分 load 4.4〜4.7（S2 の 3 回。scope.tsv）で始まった。いまの 3 組は 5 分 load 1.56〜2.10 で始まった。

## 3. 印の無かった組

flags.tsv の check_*・test_build_* の 6 組と health:S1 は `flag=ok` なので、before を測っていない。test_affected の 2 組は control-s1・control-s2 の担当（どちらも `flag=ok`）。

## 4. test-affected の `--list`（「## BEFORE」の手順の確認）

before の worktree で comment の probe を入れ、`env -u CELERIS_WU_BASE bash scripts/dev/test-affected.sh --list` を S1（3 file、3 行の差分）・S2（4 file、4 行の差分）で
走らせた。exit 0、出力は A の ta-list.txt と一致した（S1: 14 crate ＋ `web: typecheck+unit` ＋ `ts-verify: test-ts-verify`、S2: 5 crate。`diff` の差 0）。
走らせた後の `git status --porcelain -- crates` は空。test-affected の本体は走らせていない（印が無いので測らない）。

## 5. 再現コマンド（repo の root で。`ART` はこの run の成果物 dir）

```sh
B0=bc665168c9caf1898adc4c611404bbee056daf9a
S1_BEFORE_FILES="crates/task-api/src/handlers/org.rs crates/task-api/src/cos/ops/admin.rs crates/task-core/src/store/org.rs"
S2_BEFORE_FILES="crates/task-dispatch/src/run_settlement/work_unit.rs crates/task-dispatch/src/run_settlement.rs crates/task-dispatch/src/dispatcher/worker_finish.rs crates/task-dispatch/src/dispatcher/work_units.rs"
git worktree add --detach "$TMPDIR/wt-before" $B0
git -C "$TMPDIR/wt-before" rev-parse HEAD:crates      # control-build.tsv の src_tree の列と同じ値
# --list の確認（S1 も同じ形で $S1_BEFORE_FILES に替える）
(cd "$TMPDIR/wt-before" && $ART/raw/probe-comment.sh $S2_BEFORE_FILES && env -u CELERIS_WU_BASE bash scripts/dev/test-affected.sh --list; git checkout -- $S2_BEFORE_FILES)
# health:S2 の before を valid 3 回、各回の直後に同じ時間帯の after を 1 回（waitload・const の probe・戻し・TSV の行）
WT="$TMPDIR/wt-before" AFT=$(pwd) OUT=$ART/raw DEADLINE=$(( $(date +%s) + 1400 )) bash $ART/raw/control-health.sh
git worktree remove --force "$TMPDIR/wt-before"; git worktree prune
git status --porcelain -- crates; git diff --stat "$CELERIS_WU_BASE" -- crates      # どちらも空
```

生の記録（この run の成果物 dir の `raw/`）: `control-health.sh`・`probe-const.sh`・`probe-comment.sh`・`health-S2-{before,after}-{1,2,3}.{json,out,time}`・
`control-build-{before,after}.tsv`・`timing-raw.log`（各回の時刻・load・verdict・戻した後の `git status`）・`ta-list-before.txt`。

## 6. 未達・要判断

| 項目 | 状態 |
|---|---|
| health:S2 の +17.3 % がコード由来か host 由来か | この leaf は値を分けただけ（§0 の 3 つの和）。判定は cause の leaf |
| before の 2 回目（107.83 s、待ちの時間 30.68 s）の原因 | 未確認（I/O の待ちか、CPU の取り合いか） |
| A の測定の時の host の状態（5 分 load など） | A の記録に無いので、host の変化の中身（何が変わったか）は未確認。大きさは §1 の +5.34 s（+6.5 %） |
