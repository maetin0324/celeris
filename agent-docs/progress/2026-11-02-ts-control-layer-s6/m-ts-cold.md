---
title: S6 m-ts-cold — cos-ts の cold を 20 試行
tasks: [01M6FPPJN7A8H427MXFVYFY7H4]
status: done
completed: 2026-11-03
updated: 2026-11-03
---
# S6 m-ts-cold — cos-ts の cold を 20 試行

完了日: 2026-11-03（unit m-ts-cold）。[境界 ADR](../../adr/2026-11-02-ts-control-layer-boundary.md) の付記 S5-10・S5-11 に従った。コードと script は変えていない。IEEE Access・K2・K4 の release・state dir・config には触れていない（測定は `$TMPDIR` の一時 state dir だけを使う）。

## 結論
- cos-ts の cold は 20 試行すべて exit 0 で、目標（p95 120 秒）に合格した。p95 は 7.795 秒で、目標より 112.205 秒短い（目標の 6.5%）。
  | scenario | 試行 | p50（秒） | p95（秒） | 最大（秒） | 最小（秒） | over_300s | busy |
  |---|---|---|---|---|---|---|---|
  | cos-ts cold（この unit） | 20 | 7.622 | 7.795 | 7.835 | 7.052 | 0 | 0 |
  | cos-ts warm（[m-ts-warm](m-ts-warm.md) の生データ） | 20 | 7.470 | 7.604 | 7.608 | 7.053 | 0 | 0 |
  | cold ÷ warm | — | 1.020 | 1.025 | — | — | — | — |
- cold と warm の差は p50 で 0.152 秒、p95 で 0.191 秒。差のほぼ全部が段 install（`pnpm install --offline --frozen-lockfile`）で、cold の install の p50 は 0.204 秒、warm は 0.002 秒だった。unit（p50 1.630 / 1.624 秒）と integration（p50 4.239 / 4.210 秒）は cold と warm で変わらない。
- 生データ: [cos-ts-cold.jsonl](cos-ts-cold.jsonl)（prime 1 行＋trial 20 行。cold は温める ts-verify を走らせないので prime は runner-ensure の 1 行だけ）。

## cold で捨てた cache（試行ごと）
- script（`scripts/dev/edit-activate-baseline.sh` の cos-ts・cold の経路）は各試行の編集の前に `rm -rf control-ts/node_modules` を実行する。消す時間は測らない。
- 消えたことは、この run の中で 1 秒ごとに `control-ts/node_modules` の inode と作成時刻を読む見張り（install が終わった印 `.modules.yaml` が在るときに `du -sb` と `find | wc -l` を 1 回だけ取る）で確かめた。CPU を焼く処理ではない。見張りの記録は run の artifacts（`nm-watch.log`）にあり、repo には入れていない。
  - 20 試行で、作成時刻の違う `control-ts/node_modules` が 20 個できた（試行ごとに 1 個。最初は 11:34:29 UTC、最後は 11:36:56 UTC）。試行ごとに作り直されたことになる。
  - 試行 1 の前: この worktree には `control-ts/node_modules` が無かった（消した物 0 B）。
  - 試行 2〜20 の前: 1 つ前の試行が作った `control-ts/node_modules` を消した。大きさは 80,527,772〜80,565,342 byte（約 80.6 MB）、entry 数 1,502〜1,508。測定後に残った物（試行 20 が作った物）は 80,565,339 byte・1,508 entry。
  - 見張りは dir の ctime も key に含めたため、install の途中の dir の更新を「消えた」と誤って記録した行がある。作成時刻（`stat %W`）で数えた上の 20 個が本当の作り直しの数である。
- 捨てていない cache: pnpm の store（`/local/.pnpm-store/v11`。`--offline` の install はここから link する）と、script-runner の cache（prime の `script-runner-cache.sh ensure` は 0.178 秒で当たった）。script の定義どおり「runner の cache は当たる状態で測る」。Rust の `CARGO_TARGET_DIR` は TS の経路では使わない（全試行で `target_kind: none`、cargo 起動 0、link 0）。

## 開始前の状態（2026-11-03 11:34 UTC、host home-dev）
- `git status --porcelain` → 空
- `uptime` → `load average: 0.67, 3.36, 5.62`
- 他の cargo・rustc: `pgrep -c -x cargo` → 0、`pgrep -c -x rustc` → 0
- `/local` の空き 131 GB
- 試行前の 1 分 load average は 0.51〜2.01。自分以外の cargo・rustc は全試行の前後と最中で 0（busy の印 0 件）

## 実行したこと
1. `bash scripts/dev/edit-activate-baseline.sh --scenario cos-ts --mode cold --reps 20 --series m-ts-cold --out agent-docs/progress/2026-11-02-ts-control-layer-s6/cos-ts-cold.jsonl` → exit 0（11:34:27〜11:37:06 UTC）。見張りだけを並べて走らせ、cargo・試験は並べていない
2. `--validate` と `--check-targets` を実行し、この file を書いて commit

## 識別（env.identity）と warm との照合
- 全試行で `clean: true`、`reverted: true`、`series: m-ts-cold`、`target_kind: none`、`class: ts-only`
- warm（cos-ts.jsonl）と同じ値: `tree.control-ts`、rustc 1.98.1、node v24.14.1、pnpm 12.6.0、host home-dev、`host_api` 1.1.0、`runner_sha256`
- 違う値: `commit`（間に生データと進捗の commit が入った。control-ts の tree hash は同じ）と `series`。比は同じ tree・toolchain・host の試行どうしで取った

## 最遅 3 試行の内訳（秒）
| rep | 合計 | install | typecheck | bundle | unit | golden | safety | integration | activate |
|---|---|---|---|---|---|---|---|---|---|
| 7 | 7.835 | 0.203 | 0.093 | 0.074 | 1.676 | 0.813 | 0.042 | 4.395 | 0.070 |
| 2 | 7.795 | 0.211 | 0.099 | 0.075 | 1.651 | 0.801 | 0.043 | 4.376 | 0.074 |
| 15 | 7.773 | 0.203 | 0.095 | 0.073 | 1.627 | 0.811 | 0.043 | 4.385 | 0.074 |

残りは classify（約 0.03 秒）・runner-cache（約 0.18 秒）。unit は node_modules が無い状態から始まるので常に全部（note `ALL`）。

## 証拠（提出時の HEAD で実行）
- `bash scripts/dev/edit-activate-baseline.sh --validate agent-docs/progress/2026-11-02-ts-control-layer-s6/cos-ts-cold.jsonl` → exit 0（records 21、prime 1・trial 20、problems 0）
- `bash scripts/dev/edit-activate-baseline.sh --check-targets agent-docs/progress/2026-11-02-ts-control-layer-s6/cos-ts-cold.jsonl` → exit 0（trials 20、p95 7.795 / 120.0、over_300s 0、busy 0、problems 0）
- 測定 leaf はコードを変えないので test-affected・clippy・fmt は走らせていない（付記 S5-11 の 3・5）

## 未解決
- この cold は node_modules だけを捨てる定義で、pnpm の store と runner の cache は残る。store が空の host（初めての clone）や runner を作り直す場合の秒は測っていない。s6-report で「cold build の増分」を書くときは、この定義の範囲（install 0.2 秒の増分）であることを明記する。

## 提案
- s6-report は、真の初回（pnpm store が空・runner の cache が無い）を別の測定として要るか判断する。要るなら store を `$TMPDIR` に向ける（`--store-dir`）と、ネットワークに出ない `--offline` が失敗するので、先に store の写しを作る手順が要る。
