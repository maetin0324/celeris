---
title: S6 m-cosd-a — 旧 Rust 経路 cos-digest の warm 試行（前半）
tasks: [01M6FPPJN7A8H427MXFVYFY7H4]
status: done
completed: 2026-11-03
updated: 2026-11-03
---
# S6 m-cosd-a — 旧 Rust 経路 cos-digest の warm 試行（前半）

unit m-cosd-a。[境界 ADR](../../adr/2026-11-02-ts-control-layer-boundary.md) の付記 S5-10・S5-11 に従う。コードと script は変えない。IEEE Access・K2・K4 の release・state dir・config には触れない。

## 編集が cos-ts と同じ論理であること（測る前に確認）
結論: 同じ。変える判定は「`reason` が `edit-activate-baseline-<nonce>` を含む run には command を 1 つも出さない」で、両方とも当たらない分岐である。期待する出力は、どちらも編集前と同じ（全 fixture・golden で command 列が変わらない）。違いは無かったので測定に進んだ。

`scripts/dev/edit-activate-baseline.sh` の編集の定義（`apply_edit` の python、396〜411 行）:
```python
elif scenario == "cos-ts":
    # cos-digest と同じ論理: reason が nonce 入りの印を含む run には command を出さない（当たらない分岐）
    anchor = '  const { text, cards } = digestMessage(input.run_id, input.run_state, input.reason, input.items);\n'
    add = (f'  // edit-activate-baseline probe {nonce}: a branch that is never taken\n'
           f'  if (input.reason !== null && input.reason.includes("edit-activate-baseline-{nonce}")) {{\n'
           '    return { commands: [] };\n'
           '  }\n')
elif scenario == "cos-digest":
    anchor = '    let (text, cards) = cos_digest_message(\n'
    add = (f'    // edit-activate-baseline probe {nonce}: a branch that is never taken\n'
           '    if input\n'
           '        .reason\n'
           '        .as_deref()\n'
           f'        .is_some_and(|r| r.contains("edit-activate-baseline-{nonce}"))\n'
           '    {\n'
           '        return Vec::new();\n'
           '    }\n')
```

突き合わせ:

| 項目 | cos-ts（`control-ts/src/domains/cos_digest.ts`） | cos-digest（`crates/task-core/src/chat/digest.rs`） |
|---|---|---|
| 入れる関数 | `decide(input: CosDigestInput): CosDigestOutput`（370 行） | `cos_digest_decide(input: &CosDigestInput) -> Vec<CosDigestCommand>`（268 行） |
| 入れる位置 | 関数の先頭、`digestMessage(...)` を呼ぶ行の直前 | 関数の先頭、`cos_digest_message(` を呼ぶ行の直前 |
| 入力の型 | `reason: string \| null`（63 行） | `pub reason: Option<String>`（32 行） |
| 条件 | `reason !== null` かつ `reason.includes("edit-activate-baseline-<nonce>")` | `reason.as_deref().is_some_and(\|r\| r.contains("edit-activate-baseline-<nonce>"))` |
| 当たったときの出力 | `{ commands: [] }`（`CosDigestOutput` の command 列が空） | `Vec::new()`（command 列が空） |
| `reason` が無いとき | 当たらない（`null`） | 当たらない（`None`） |
| nonce | 試行ごとの 8 桁の 16 進数（同じ `apply_edit` が作る） | 同じ |

- `null`／`None` の扱い・部分文字列の判定・空の command 列の 3 点が一致する。TS の出力は `{commands}` の object、Rust は列そのものだが、境界の変換（S4-2）で同じ command 列に当たる。
- fixture・golden・試験の `reason` に `edit-activate-baseline-` を含むものは無い（nonce は試行ごとに乱数）。どちらの経路でも分岐は当たらず、出力は編集前と同じになる。
- 検証責任の揃え方（Rust 側は `-p task-core -p task-dispatch` の 6 段、TS 側は ts-verify の 10 段）は [cos-ts-scenario](cos-ts-scenario.md) のとおりで、ここでは変えない。

## 結論（測定）
- 未達（7/10）。warm の 7 試行はすべて全段 exit 0 で、編集もすべて戻った（`reverted: true`）。1 試行が約 345 秒かかり、prime（約 415 秒）と合わせて run の壁時計 3600 秒に 10 試行が入らない。付記 S5-11 の 6 に従い、終了時刻から数えて収まる 7 試行に絞った。足りない 3 試行を推定で埋めない。
- 7 試行の合計の秒（rep 1〜7）: 343.005・344.788・344.824・343.619・345.115・344.341・344.839。最近傍順位で p50 344.788、p95 345.115（n=7 なので最大値）。p50・p95 の確定は後半と連結した後に s6-report が行う。
- 7 試行とも `over_300s: true`（[cos-ts-scenario](cos-ts-scenario.md) の 1 試行 345.217 秒と同じ水準）。busy は 0 件。
- 生データ: [cos-digest-warm-a.jsonl](cos-digest-warm-a.jsonl)（prime 6 行＋step 42 行＋estimate 40 行）。rep は 1〜7。

| 段 | 中央値 | 最小〜最大 |
|---|---|---|
| check | 30.492 | 30.252〜30.825 |
| build | 55.946 | 55.260〜56.344 |
| clippy | 34.081 | 33.851〜34.193 |
| test-build | 80.166 | 79.755〜80.900 |
| test-run | 50.397 | 50.224〜51.965 |
| release-build | 93.070 | 92.622〜94.132 |

- prime（集計に入れない）: check 39.4・build 65.8・clippy 34.5・test-build 83.5・test-run 51.3・release-build 141.1 秒。

## 実行したこと
1. 上の突き合わせを書いて commit（`git status --porcelain` が空になったことを確認）
2. 開始前（2026-11-03 13:12 UTC、host home-dev）: `uptime` → load average 2.35, 5.43, 6.77、`pgrep -c -x cargo` → 0、`pgrep -c -x rustc` → 0、`/local` の空き 129 GB
3. `timeout -s TERM 3131 bash scripts/dev/edit-activate-baseline.sh --scenario cos-digest --mode warm --reps 7 --series m-cosd-a --out agent-docs/progress/2026-11-02-ts-control-layer-s6/cos-digest-warm-a.jsonl` → exit 0（13:12:49〜14:00:02 UTC。timeout は働いていない）。同じ run の中で cargo・試験を並べていない
   - objective の `--reps 10` を 7 にした理由は上の結論。`--series` は付記 S5-10 の系列名。`--rep-start` は既定の 1

## 識別と負荷
- 全 48 行（prime・step）で `env.identity` は 1 種類（`clean: true`、`series: m-cosd-a`、`target_kind: warm-shared`、host home-dev）。値は jsonl にある。m-cosd-b と連結するときは tree hash（`crates/task-core`・`crates/task-dispatch`）を照合する
- 試行の前後の 1 分 load average は 12.49〜14.85。測定自身の cargo が含まれる。試行の前後と最中の他の cargo・rustc は全試行で 0
- estimate の 40 行は release 記録の読み取りで、測定値ではない

## 証拠（提出時の HEAD で実行）
- `bash scripts/dev/edit-activate-baseline.sh --validate agent-docs/progress/2026-11-02-ts-control-layer-s6/cos-digest-warm-a.jsonl` → exit 0（records 88、prime 6・step 42・estimate 40、problems 0）
- `bash scripts/dev/edit-activate-baseline.sh --check-targets agent-docs/progress/2026-11-02-ts-control-layer-s6/cos-digest-warm-a.jsonl` → trials 7、over_300s 7、busy 0、p50 344.788、p95 345.115、problems 0
- 測定 leaf はコードを変えないので test-affected・clippy・fmt は走らせていない（付記 S5-11 の 3・5）

## 未解決
- 前半が 7 試行で止まった。後半（m-cosd-b、`--rep-start 11`）も 1 run に入るのは 7 試行前後で、合わせて 20 試行に届かない見込み（約 14）。

## 提案
- 計画で cos-digest の測定 leaf を 1 本足すか、前後半を 3 分割（各 7 試行、rep 1・8・15 から）にする。cos-digest の 1 試行は 345 秒で、prime 415 秒と合わせると 1 run に 7 試行が上限である。後半の rep の始まりを 8 にすれば rep の番号に穴が開かない。
