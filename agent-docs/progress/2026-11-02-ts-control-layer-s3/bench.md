---
title: S3 bench — edit-activate-baseline.sh の scenario policy-ts と目標判定
tasks: [01M6EVP95S9VM3DBMZFRD8FS35]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S3 bench — edit-activate-baseline.sh の scenario policy-ts と目標判定

完了日: 2026-11-02（task 01M6EVP95S9VM3DBMZFRD8FS35、unit bench。親 task は 01M6DCMB75G5F0DSE58ZNZMBAK）

## 結論
- `scripts/dev/edit-activate-baseline.sh` に scenario `policy-ts` と `--check-targets <file.jsonl>` を足した。S0 の `policy`・`cos` の経路は変えていない。
- 試験 `scripts/dev/tests/edit-activate-policy-ts.sh`（引数なし）は 90 件すべて通る。
- 確認のために warm 2 試行・cold 2 試行を実際に走らせた。warm は 6.8〜6.9 秒、cold は 7.3〜8.6 秒で、全試行とも cargo 0 回・link 0 回だった。
  これは動作の確認で、目標の照合に使う本番の計測ではない（20 試行は unit measure-warm・measure-cold が測る）。

## 変更
- `phase s3: bench: edit-activate-baseline.sh に TS 経路の scenario policy-ts と目標判定を足す`
  - `scripts/dev/edit-activate-baseline.sh`:
    - 編集: `control-ts/src/domains/lane_rules.ts` の `RULES` の `standard/default` の前に、当たらない規則を 1 件足す。nonce は試行ごとに変える。
    - 計測: `env CELERIS_WU_BASE=HEAD bash scripts/dev/ts-verify.sh --record <file>` を 1 試行に 1 回走らせる。基準を HEAD にするので、分類はこの編集だけで決まる。
    - 記録: 1 試行 1 行（`kind=trial`）。欄は `secs`（編集を書いてから activate が終わるまで）、`stages`（10 段の秒と exit）、
      `typecheck_secs`、`bundle_secs`、`cargo_invocations`、`cargo_compile_invocations`、`link_count`、`class`、`edit`、`reverted`、`env`。
    - prime: `kind=prime` の `step=runner-ensure`（秒を `runner_build_secs` にも書く）。warm はさらに `step=ts-verify` を 1 行足す。
    - cold: 試行ごとに編集の前に `control-ts/node_modules` を消す（消す時間は測らない）。段 install の `pnpm install --offline --frozen-lockfile` から測る。
    - 開始時の拒否（exit 2）: 基準 HEAD の分類が `none` でないとき、`--out` が空でないとき。
    - `--validate`: `kind=trial` と、`step=runner-ensure` の `runner_build_secs` を検査に足した。
    - `--check-targets`: warm は p50 ≤ 30 秒かつ p95 ≤ 60 秒、cold は p95 ≤ 120 秒（最近傍順位）。全試行で exit 0・`class` が `ts-only`・10 段すべて exit 0・
      cargo 起動 0・`link_count` 0・編集を戻せた、のときだけ exit 0。欄の欠け・rep の重複・試行以外の未知の行・試行 0 件は exit 1、読めない file は exit 2。
  - `scripts/dev/tests/edit-activate-policy-ts.sh`: 新規。`--dry-run`・`--validate`・`--check-targets` を小さな記録で確かめ、
    git fixture（本物の `lane_rules.ts` の写し・偽の ts-verify）で warm・cold・失敗・作業ツリーの拒否を確かめる。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
| 条件 | コマンド | 結果 |
|---|---|---|
| 0. dry-run が手順を出す | `bash scripts/dev/edit-activate-baseline.sh --dry-run --scenario policy-ts` | exit 0、10 段を含む手順を出す。試験で file の不変と cargo 0 回も確かめた |
| 1. 目標の内外で 0/非 0 | `bash scripts/dev/tests/edit-activate-policy-ts.sh` | exit 0、`90 passed, 0 failed`。warm の 10 番目 30 秒・19 番目 60 秒は exit 0、30.5 秒・60.5 秒は exit 1。cold の 19 番目 120 秒は exit 0、120.5 秒は exit 1。1 試行の cargo 1・link 1 で exit 1 |
| 動作の確認 warm | `--scenario policy-ts --mode warm --reps 2 --out $TMPDIR/smoke/warm.jsonl` と `--validate`・`--check-targets` | いずれも exit 0。6.875 秒・6.815 秒、`class` ts-only、cargo 0、link 0 |
| 動作の確認 cold | `--scenario policy-ts --mode cold --reps 2 --out $TMPDIR/smoke/cold.jsonl` と `--validate`・`--check-targets` | いずれも exit 0。7.284 秒・8.644 秒、install 0.251 秒・0.527 秒、cargo 0、link 0 |
| 影響試験 | `bash scripts/dev/test-affected.sh` | exit 0、`試験対象の変更なし`（この unit の変更は crate と control-ts に写らない。文書検査は実行された） |
| clippy | `cargo clippy --workspace -- -D warnings` | exit 0 |
| fmt | `cargo fmt --all -- --check` | exit 0 |

動作の確認で最も時間がかかった段は `integration`（4.4〜5.1 秒）だった。`tsc --version` は 7.0.2 で、型検査は約 0.1 秒だった。
この速さでも型を見ていることは、型の誤りを 1 行足して exit 1（TS2322）になることで確かめた。

## 未解決
- `--check-targets` は試行数の下限を持たない（小さな記録で試験するため）。報告の JSON の `modes.<mode>.trials` が試行数で、20 試行かどうかは measure の unit が確かめる。
- cold はページキャッシュを落とさない。消すのは `control-ts/node_modules` だけで、store も OS のキャッシュも残る。
- cold の単体試験は常に全部になる。段 classify の時点で `node_modules` が無く、import グラフを計算できないため（ADR (1)-1 の「追えないときは全部」）。今の試験は 3 file すべてが選ばれるので、warm との差は出ない。

## 提案
- close unit で ADR (7) に付記する: warm の prime は「編集なしで 1 回」ではなく、編集を当てて 1 回走らせる。編集なしでは ts-verify が分類 `none` で抜け、何も温まらないため。
  この unit の範囲に ADR が入っていないので、ここには書かなかった。
- close unit で ADR (7) に付記する: 記録は 1 試行 1 行（`kind=trial`）で、段の秒は `stages` の欄に入れた。S0 の 1 行 1 段と形が違う理由は、ts-verify の cargo と link の数が試行全体の値で、段ごとに分けられないため。
