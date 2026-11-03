---
title: grade-cos-digest — script-host-runner の採点に cos_digest を足す
tasks: [01M6DCMB75G5F0DSE58ZNZMBAK]
status: done
updated: 2026-11-03
completed: 2026-11-03
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

提出の run（計画 v3、提出時の HEAD で実行）の結果:
- `bash scripts/dev/test-ts-verify.sh` → exit 0、18 passed、0 failed（場合 K: 壊した `cos_digest` の bundle を runner は exit 1、mismatch 404 件 / 2086 行）
- `bash scripts/dev/test-affected.sh` → exit 0、946 tests run: 946 passed、`test-ts-verify` 18 passed
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `bash scripts/dev/tests/script-runner-cache-key.sh` → exit 0、35 passed、0 failed
- `bash scripts/dev/tests/ts-verify-classify.sh` → exit 0、45 passed、0 failed
- `bash scripts/dev/tests/test-affected-list.sh` → exit 0
- `cargo nextest run -p celeris-script --test script_host_runner` → 13 passed
- S4 の bundle（今の `control-ts/` を `build-bundle.mjs` で作った物）を runner に直に渡した: `run --suite golden` exit 0（`cos_digest` 2086/2086 一致、mismatch 0、denied 0、passes 2）、`run --suite safety` exit 0（壊した op の応答で `outcome: exception`）、`activate --state-dir` exit 0（`cos_digest` は `valid`）
- 範囲 check（計画 v3 の許可一覧）→ 範囲外 0 件

## 範囲 check の不合格（計画 v3 で解消）
前の run では範囲 check が `scripts/dev/test-affected.sh`、`scripts/dev/tests/script-runner-cache-key.sh`、`scripts/dev/tests/ts-verify-classify.sh` を拒否し、この unit は `plan_issue` で返した。計画 v3 が 3 file を許可一覧に足したので、成果は作り直さずに同じ branch のまま提出した。3 file を変えた理由は次のとおり。
- `script-runner-cache-key.sh`: `GOLDEN_SPECS` に足した `dump-cos-digest-fixtures` を偽の cargo が知らず、変更前の版は exit 1 になる。
- `ts-verify-classify.sh`: 偽の runner の `domains` が旧い `fixture` 欄を返し、変更前の版は 40 passed・5 failed になる。
- `test-affected.sh`: `pass_traces.jsonl` は key の入力になったので、変更時に `test-ts-verify.sh` を選ぶ 1 行を足した。場合 L がこの選択を固定する。

fixture の無い golden を飛ばす分岐を製品の script に足して試験を通す方法は、検査を弱めるので採らなかった。

## 作業中の事故
patch の heredoc の終端を誤り、場合 K・L の破壊の 2 行が作業ツリーで実行された。`control-ts/src/domains/cos_digest.ts` と `control-ts/fixtures/cos_digest/pass_traces.jsonl` が変わったので、`git restore` で戻した。提出の差分に `control-ts/` は無い。

## 未解決
1. runner は TS の出力の key の順を見ない（付記 grade-cos-digest-7）。`serde_json` の `preserve_order` を外した S1 の決定による制約で、既存の 2 領域も同じである。Rust の apply は型で受けるので順に依存しない。
2. `pass_traces.jsonl` の golden は候補の file の写しで、`cmp` は違いを検出しない（付記 grade-cos-digest-6）。cargo 側の `cos_digest_trace.rs` の byte 一致が守る。
3. `pass_traces.jsonl` に `read_run` の失敗の場面は無い（20 行とも op の答えは `ok`）。境界 ADR 付記 S4-16 は `read_run` の失敗を場面に挙げている。runner は記録に error があれば答えられるが、今の fixture では「`exception` の run を飛ばす」動作を golden で採点していない。fixture を変えない条件なので、この unit では足していない。

## 提案
- `cos_digest_trace.rs` の場面に `read_run` の失敗（`exception` で run を飛ばす）を足す（未解決 3）。
