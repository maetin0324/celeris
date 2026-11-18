---
title: アーキ v4 D timing — S1 の warm check と repo health の時間を A と同じ手順で 3 回ずつ測る
tasks: [01M7MZTKB4RYM80S6CT3RST2N9]
status: done
updated: 2026-11-18
completed: 2026-11-18
---
# timing（WorkUnit）

完了日: 2026-11-18

統合後の HEAD（件名「arch-v4 D metrics: A と同じ計測で org の集約の before/after を metrics.md に書く」の commit）で、
S1（新しい CoS 業務操作）の warm check と repo health の時間を、A（[2026-11-17-arch-v4-a](../2026-11-17-arch-v4-a.md) と
A の run の `artifacts/baseline.md` §5.3・§5.5・§6 の 9・§7 C1・C8）と同じ手順で 3 回ずつ測った。
source は変えていない（probe は測った後に戻した。この WorkUnit の変更はこの file だけ）。

## 0. 結論

- warm check（S1）の中央値は A の 42.47 s に対して 43.95 s（after の 4 file）、43.58 s（A と同じ 3 file）。差は +1.1〜+1.5 s（+2.6〜+3.5 %）。
  速くはなっていない。差の原因は確かめていない（未確認。A の 3 回の幅 41.99〜43.34 s を少し超える）。
- repo health（S1 の probe を入れた状態）の中央値は A の 86.56 s に対して 83.70 s（−2.86 s、−3.3 %）。verdict は A と同じく fail（exit 1）。
- probe を `org_write.rs` に入れても入れなくても warm check の時間は変わらない（43.95 s 対 43.58 s）。
  時間を決めるのは `crates/task-core/src/store/org.rs` の変更で、task-core に依存する crate をすべて check し直すからである（A の §5.3 の観察と同じ）。
- 未測定: test-affected の 3 回、`cargo nextest run --workspace --no-run` の 3 回、reviewer の時間と費用。子 G が測る。この WorkUnit では測っていない。

## 1. 計測の条件

- host・env は A と同じ: nproc 24、cargo 1.98.1、rustc 1.98.1、`CARGO_INCREMENTAL=0`、`CARGO_PROFILE_DEV_DEBUG=line-tables-only`、
  rustc-wrapper（`~/.cargo/config.toml` の `/home/rmaeda/.local/bin/rustc-wrapper.sh`、sccache）。`CARGO_TARGET_DIR` は Celeris が渡した WU の scratch。
- 各回の前に `/proc/loadavg` の 1 分値が 4 以下になるまで待った（A の §7 C8 の waitload: 10 s ごとに見て最大 600 s）。
  すべての回が 4 以下で始まった（600 s で打ち切った回は無い）。開始時の load は §2・§3 の表に書いた。
- S1 の file の組:
  - after（4 file）: `crates/task-api/src/org_write.rs`、`crates/task-api/src/handlers/org.rs`、`crates/task-api/src/cos/ops/admin.rs`、
    `crates/task-core/src/store/org.rs`。metrics.md §5 の after の組（同じ意味の変更を HEAD で行うときに変える file）と同じ。
  - A と同じ組（3 file）: 上から `org_write.rs` を除いた 3 file（A の §7 C8 の S1 の file）。
- probe は A の §7 C8 と同じ: 各 file の末尾に `printf '\n#[allow(dead_code)]\npub const SCENARIO_PROBE_%s: u64 = %s;\n' <file の番号> $(date +%s%N)` を足す。
  値を毎回変えるので sccache は当たらない。
- 生の記録は run の artifacts の `raw/`（`timing-raw.log`、`health-S1after-{1,2,3}.{out,json}`、script の `c8-check-warm.sh`・`c1-health.sh`）。

## 2. warm check（`cargo check --workspace --offline -q`、S1 の probe を入れた状態）

| 計測 | run 1 | run 2 | run 3 | 中央値 | user（中央値） | 開始時の 1 分 load（run 1 / 2 / 3） | 待った時間 |
|---|---|---|---|---|---|---|---|
| A（before、3 file、bc665168） | 41.99 s | 42.47 s | 43.34 s | **42.47 s** | 58.7 s | 記録なし（A の §5.3） | — |
| after・4 file（org_write.rs を含む） | 43.57 s | 43.95 s | 44.00 s | **43.95 s** | 59.6 s | 3.77 / 3.13 / 3.68 | 430 / 0 / 0 s |
| after・A と同じ 3 file | 43.58 s | 44.05 s | 42.58 s | **43.58 s** | 59.5 s | 2.93 / 3.67 / 3.51 | 0 / 570 / 0 s |

- 手順（各回）: `git checkout -- <files>` → 計らない `cargo check --workspace --offline -q`（同じ起点を作る。全回 exit 0）→ probe を足す →
  waitload → `/usr/bin/time` で計る → `git checkout -- <files>`。全回 exit 0。
- 待った時間が長い回（430 s・570 s）は、host の他の処理で load が 4 を超えていたため。A の規則どおり 4 以下になってから計った。
- A の表の load の欄: A の §5.3 の check warm の表は開始時の load を書いていない（A の `timing-raw.log` にも check warm の回の load は無い）。

## 3. repo health（C1、S1 の probe を入れた状態）

コマンド: `python3 scripts/dev/repo-health/repo_health.py --repo . --base <HEAD> --head worktree --policy-version 1 --out <json>`
（A の §5.5 は `--base bc665168 --head worktree`。§7 の「after では HEAD の sha」に従い、after は base を提出時の HEAD にした）。

| 計測 | run 1 | run 2 | run 3 | 中央値 | 開始時の 1 分 load（run 1 / 2 / 3） | verdict |
|---|---|---|---|---|---|---|
| A（before、3 file） | 87.82 s | 86.56 s | 81.48 s | **86.56 s** | 記録なし（A の §5.5） | fail（exit 1）: new 3・worsened 1 |
| after・4 file | 90.22 s | 83.32 s | 83.70 s | **83.70 s** | 3.25 / 2.73 / 1.74 | fail（exit 1）: new 4・worsened 1 |

- 待った時間は 3 回とも 0 s。user は 71.43 / 70.90 / 72.04 s（A は run 1 で 70.01 s）、max RSS 約 642 MB（A 611 MB）。
- fail の中身は A と同じ種類: probe が足した名前の `link.unlinked`（file ごとに new 1。after は 4 file なので 4）と、
  `size.responsibility-growth` の worsened 1（単位 `crates/task-core/src/store`、本体 16,930 行が閾値 8,000 を超える既存の負債に probe の行が加わった）。
- A と同じ 3 file の組の health は測っていない（時間の予算のため）。health の時間は tree 全体の走査で決まり、probe の file の数（3 か 4）にはほとんど依存しないと見込むが、未確認。

## 4. 未測定の項目（done ではない）

| 項目 | 状態 |
|---|---|
| test-affected（`bash scripts/dev/test-affected.sh`）の 3 回 | 未測定。子 G が測る |
| `cargo nextest run --workspace --no-run --offline` の cold・warm の 3 回 | 未測定。子 G が測る |
| reviewer の時間と費用（A の §7 C11） | 未測定。子 G が測る |
| repo health の A と同じ 3 file の組の 3 回 | 未測定（§3 の最後の項） |

## 5. 再現コマンド（repo の root で）

```sh
ART=<run の artifacts>; mkdir -p $ART/raw
# warm check（script は A の §7 C8 の手順をそのまま書いたもの）
RAW=$ART/raw/timing-raw.log $ART/raw/c8-check-warm.sh S1after crates/task-api/src/org_write.rs crates/task-api/src/handlers/org.rs crates/task-api/src/cos/ops/admin.rs crates/task-core/src/store/org.rs
RAW=$ART/raw/timing-raw.log $ART/raw/c8-check-warm.sh S1A crates/task-api/src/handlers/org.rs crates/task-api/src/cos/ops/admin.rs crates/task-core/src/store/org.rs
# health（base は HEAD）
RAW=$ART/raw/timing-raw.log OUT=$ART/raw $ART/raw/c1-health.sh S1after crates/task-api/src/org_write.rs crates/task-api/src/handlers/org.rs crates/task-api/src/cos/ops/admin.rs crates/task-core/src/store/org.rs
# 戻したことの確認
git status --porcelain -- crates    # 空
```

script の中身（要点）: `waitload(){ s=0; while awk '{exit !($1>4)}' /proc/loadavg && [ $s -lt 600 ]; do sleep 10; s=$((s+10)); done; }`、
各回 `git checkout -- <files>` → 計らない build（check のみ）→ probe → waitload → `/usr/bin/time -f 'wall=%e user=%U sys=%S maxrss_kb=%M'` → `git checkout -- <files>`。

## 6. 回帰条件・release gate

- 回帰条件: 同じ host・env・手順で、S1 の warm check の中央値が A の 42.47 s を大きく超える（例: +10 % の 46.7 s 超）か、health の中央値が 86.56 s を
  大きく超えたら、子 G で原因を絞る。今回の +3.5 % はこの目安の内にあるが、原因は未確認。
- release gate: この WorkUnit では走らせていない（全体の試験は release gate だけで走らせる規則）。本番への昇格はしていない。

## 7. 未解決事項

- warm check の +1.1〜+1.5 s の差の原因は未確認（task-api の `org_write` の単位の追加か、host の状態か）。子 G の 3 回の比較で確かめる候補。
