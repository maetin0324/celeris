---
title: grade-cos-digest — script-host-runner の採点に cos_digest を足す
tasks: [01M6DCMB75G5F0DSE58ZNZMBAK]
status: blocked
updated: 2026-11-03
---
# grade-cos-digest — script-host-runner の採点に cos_digest を足す

## 結論
`script-host-runner` は `cos_digest` を本物の deno_core host で採点する。S3 と S4 の統合で落ちていた `test-ts-verify.sh` の場合 A・B（runner）・J（無害）は通るようになった。
設計の記録は [ADR ts-fast-verify 付記 grade-cos-digest](../../adr/2026-11-02-ts-fast-verify.md)。fixture と `control-ts/` は変えていない。依存 crate も足していない。

## 変更
- `phase s3-s4: grade-cos-digest: runner の採点に cos_digest を足し、pass を本物の host で replay する`
  - `crates/script-runtime/src/bin/script-host-runner.rs`: 場面（入力と op の記録）、runner 内の op server、壊した op の応答、1 領域複数 file
  - `crates/script-runtime/tests/script_host_runner.rs`: 試験を 8 件足した（計 13 件）
  - `scripts/dev/script-runner-cache.sh`: `cos_digest/cases.jsonl` の golden と、key に内容を入れて写す `pass_traces.jsonl`
  - `scripts/dev/ts-verify.sh`: 段 `golden` を 1 領域複数 file に合わせ、key の入力の判定に `pass_traces.jsonl` を足した
  - `scripts/dev/test-affected.sh`: `pass_traces.jsonl` の変更で `test-ts-verify.sh` を選ぶ
  - `scripts/dev/test-ts-verify.sh`: 場合 K・L を足した（計 18 件）
  - `scripts/dev/tests/{script-runner-cache-key.sh,ts-verify-classify.sh}`: 6 入力と `fixtures` 欄に合わせた
  - ADR 付記、`docs/architecture-map.md` の根拠の欄

## 受け入れ条件と証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
| 条件 | コマンド | 結果 |
|---|---|---|
| 0. `test-ts-verify.sh` が全場合 pass | `bash scripts/dev/test-ts-verify.sh` | exit 0、18 passed、0 failed（A・B runner・J 無害を含む） |
| 1. domains に `cos_digest`、S4 の bundle で ok | `script-host-runner domains`、`run --suite golden`、`run --suite safety`、`activate` | `cos_digest` の min_rows 2086。golden exit 0（2086/2086 一致、call p50 213 µs、p99 359 µs）。safety exit 0（`cos_digest` 7 件）。activate exit 0（`cos_digest` は `valid`） |
| 2. 規則を壊した bundle が mismatch で exit 1 | 場合 K（`requeues` から `interrupted` を落とす） | runner exit 1、mismatch 404 件 / 2086 行 |
| 3. ADR の付記と記録、弱めた点が無い | 付記 grade-cos-digest、この file | 既存の場合の期待値は変えていない。`cos_digest` は manifest に残した |

検査コマンド:
- `bash scripts/dev/test-affected.sh` → exit 0。945 tests passed（`celeris`・`celeris-script`・`celerisctl`）、`test-ts-verify.sh` 18 passed
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `bash scripts/dev/tests/script-runner-cache-key.sh` → 35 passed、0 failed
- `bash scripts/dev/tests/ts-verify-classify.sh` → 45 passed、0 failed
- `cargo nextest run -p celeris-script --test script_host_runner` → 13 passed
- `sh scripts/dev/progress-index.sh --check` → exit 0

再確認の run（提出時の HEAD）の結果: `test-affected.sh` は exit 0、946 tests passed、`test-ts-verify.sh` は 18 passed。S4 の bundle を runner に直に渡すと golden・safety・activate はすべて exit 0 で、`cos_digest` は 2086/2086 一致、mismatch 0、passes 2。

## 範囲 check の不合格（計画への申告）
範囲 check が `scripts/dev/test-affected.sh`、`scripts/dev/tests/script-runner-cache-key.sh`、`scripts/dev/tests/ts-verify-classify.sh` を拒否する。この unit は `plan_issue` で返した。
- `script-runner-cache-key.sh`: 目的の 4 で `GOLDEN_SPECS` に足す example を偽の cargo が知らない。変更前の版は `script-runner-cache.sh` の新しい版で exit 1 になる（途中で止まる）。
- `ts-verify-classify.sh`: 偽の runner の `domains` が旧い `fixture` 欄を返す。変更前の版は 40 passed、5 failed。
- `test-affected.sh`: `pass_traces.jsonl` は key の入力になったので、変更時に `test-ts-verify.sh` を選ぶ 1 行を足した。場合 L がこの選択を固定する。

再確認の run でも同じ check が同じ 3 file で exit 1 になった。原因は成果ではなく check の許可一覧である。基準の版の試験を今の実装に流すと、`script-runner-cache-key.sh` は exit 1、`ts-verify-classify.sh` は 40 passed・5 failed になる。`script-runner-cache-key.sh` の偽の cargo は `dump-cos-digest-fixtures` で exit 1 を返すので、目的の 4 を実装したまま試験を変えずに通す方法は無い。fixture が無い golden を飛ばす分岐を製品の script に足せば通せるが、試験に合わせて検査を弱めることになるので入れなかった。

計画に求める修正: 範囲 check の `grep -vE` の許可に `scripts/dev/test-affected\.sh` と `scripts/dev/tests/(script-runner-cache-key|ts-verify-classify)\.sh` を足す。この一覧で今の木を流すと範囲外は 0 件だった。成果の作り直しは要らない。

## 作業中の事故
patch の heredoc の終端を誤り、場合 K・L の破壊の 2 行が作業ツリーで実行された。`control-ts/src/domains/cos_digest.ts` と `control-ts/fixtures/cos_digest/pass_traces.jsonl` が変わったので、`git restore` で戻した。提出の差分に `control-ts/` は無い。

## 未解決
1. runner は TS の出力の key の順を見ない（付記 grade-cos-digest-7）。`serde_json` の `preserve_order` を外した S1 の決定による制約で、既存の 2 領域も同じである。Rust の apply は型で受けるので順に依存しない。
2. `pass_traces.jsonl` の golden は候補の file の写しで、`cmp` は違いを検出しない（付記 grade-cos-digest-6）。cargo 側の `cos_digest_trace.rs` の byte 一致が守る。
3. `pass_traces.jsonl` に `read_run` の失敗の場面は無い（20 行とも op の答えは `ok`）。境界 ADR 付記 S4-16 は `read_run` の失敗を場面に挙げている。runner は記録に error があれば答えられるが、今の fixture では「`exception` の run を飛ばす」動作を golden で採点していない。fixture を変えない条件なので、この unit では足していない。

## 提案
- `cos_digest_trace.rs` の場面に `read_run` の失敗（`exception` で run を飛ばす）を足す（未解決 3）。
