---
title: S4 ts-pass — cos_digest の async pass（cosDigestPass）と fake host の pass replay
tasks: [01M6EVP9GT07K2G01EMW0E6HNC]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S4 ts-pass — cos_digest の async pass と fake host の replay

完了日: 2026-11-02（unit ts-pass、境界 ADR 付記 S4-12・S4-13・S4-16〜S4-19）。変えたのは control-ts/ とこの記録だけ。Rust・crates/ は変えていない。

## 結論
- `control-ts/src/domains/cos_digest.ts` の入口を `cosDigestPass(ctx)` にした。`cosDigestListRuns(ctx, {})` を await して一覧を読み、一覧の順に `cosDigestReadRun(ctx, { thread, run_id })` を await し、run ごとに既存の `decide()` で command 列を作って `{ runs: [{ run_id, commands }] }` を返す。
  - `read_run` の `exception` はその run を飛ばす（S4-18・規則 8。Rust の参照実装と同じ）。
  - それ以外の op の error（`denied`・`terminated`・`generation_gone`・`timeout`・`busy`、`list_runs` の全 error）は throw し、Rust の fallback に任せる。部分の出力は返さない。
  - 応答は `deny_unknown_fields` 相当で検査する（list の応答の余計な欄、read の応答の余計な欄・別の run の答えは throw）。
- S4-17 の TS の削除: run ごとの入口 `cosDigest` の export を消した。`domains/index.ts` は `cos_digest: (_input, ctx) => cosDigestPass(ctx)` を登録する（入力は使わない）。`compare.mjs` の `entry: "cosDigest"` を消した。
- `control-ts/bundle.config.json` の `cos_digest` に `cos_digest:list_runs`・`cos_digest:read_run` を宣言した。manifest の `capabilities` は `["cos_digest:list_runs", "cos_digest:read_run", "selftest:echo_task"]`。
- `control-ts/src/testing/pass-replay.ts`（新）: fake host `fakeHost(ops, HostError, token?)` と `passFromCase()`。照合は op の名前と request の JSON 直列化の byte 一致。次を不一致（`problems`）にする: 要求が記録と違う・記録に無い op（fake host は `denied` を返す）、記録の op を 1 回も呼ばない、別の request token。
- `control-ts/scripts/compare.mjs`: `celeris:host` を `module.registerHooks` で `src/testing/host-stub.ts` へ解決し、本物の入口 `cosDigestPass` を fake host で replay する。
  - `{ name, ops, output }` の行（`pass_traces.jsonl`）は、出力を行末の `"output"` の生の byte 列と比べる。
  - `{ name, input, commands }` の行（`cases.jsonl`・`traces.jsonl`）は、`decide()` の出力を `commands` の生の byte 列と比べ（従来の比較を残す）、さらに run 1 つの pass に組み直して replay する。両方が一致した行だけ一致に数える。
  - 全行を正順と逆順の 2 回流し、同じ出力を要求する（S4-13 の 4）。不一致は file・行・name・`fields`（`commands`・`output`）・`problems`・`error` を付けて最初の 5 件を出し、exit 1。
- `build-bundle.test.mjs` を 2 か所直した（S4-19 の表に無い file。bundle.config の変更で落ちるため）: manifest の capabilities の期待値、bundle の import 文の検査（領域ごとの `import ... from "celeris:host"` を esbuild が 1 文にまとめないので、import 先の集合で見る）。

## S4-19 の試験名との対応
| 付記の名前 | 実装の試験（control-ts/src/domains/cos_digest.test.ts） |
|---|---|
| `cosDigestPass reads list then each listed run` | 同名（op の順序を `calls` で確かめる） |
| `skips a run whose read_run throws` | `skips a run whose read_run throws exception`（`exception` だけを飛ばす。他の code は下の throw の試験） |
| `never reads a run outside the list` | 同名 |
| `replay forward and reverse give the same output` | 同名 |
| （足した） | `an empty list gives no runs and reads nothing`、`a %s from read_run is thrown to Rust`（5 code）、`a %s from list_runs is thrown and nothing is read`（5 code）、応答の検査 2 件、fake host の不一致の報告、request token |

compare.test.mjs に足した pass replay の試験は 7 件（2 run の pass の両順、read の `exception` の skip、要求の食い違い、呼ばれない記録の op、`list_runs` の `denied`、requeue の規則を 1 か所壊した写しで不一致、CLI の exit 1）。pass の期待値は手書きせず、Rust が書いた `cases.jsonl` の行の `commands` の生の byte 列から組む。2 つ目の run は行の run_id を機械的に置き換えて作る（cases.jsonl の run_id が全行で同じため）。

## 受け入れ条件と証拠（提出時の HEAD で実行）
- 条件 0（cosDigestPass が op を await し command を返す）: `corepack pnpm@12.6.0 -C control-ts run test` → vitest 4 files・100 passed、node --test 41 passed（exit 0）。`cosDigestPass reads list then each listed run` が list→read×3 の順と出力の一致を確かめる。
- 条件 1（compare.mjs が fake host で pass trace を replay でき、食い違いで exit 1 になる試験がある）: `node --test control-ts/scripts/compare.test.mjs` → 20 passed。`cos_digest: the CLI exits non-zero when a pass trace does not match`（status 1）と、要求の食い違い・呼ばれない op・壊した写しの試験を含む。
- 条件 2（`node control-ts/scripts/compare.mjs cos_digest` が既存 fixture で全件一致）: `cos_digest: 2083/2083 matched (fixtures/cos_digest/cases.jsonl, fixtures/cos_digest/traces.jsonl)`、exit 0。
- `corepack pnpm@12.6.0 -C control-ts run typecheck` → exit 0。
- `bash scripts/dev/test-affected.sh` → exit 0（対象は control-ts の typecheck+test だけ。`--list` の出力 `control-ts: typecheck+test (理由: control-ts/ の変更)`）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。

## 未解決事項
1. Rust 側はまだ run ごとに `cos_digest` を呼ぶ（pass-wire unit が並行で `cos_digest_pass` に置き換える）。この unit の bundle を作り直して run ごとの呼び出しの試験（`crates/celeris/tests/script_compare_cos_digest.rs`、`CELERIS_SCRIPT_REQUIRE_BUNDLE=1`）を走らせると、TS が `{ runs }` を返すので食い違う。付記 S4-17・S4-19 のとおり、この試験は real-host unit が `script_cos_digest_pass.rs` に置き換える。工程 a-wire の統合で pass-wire と一緒に入る前提である。
2. `pass_traces.jsonl` はまだ無い（traces-async unit が Rust から書き出す）。compare.mjs はその形の行を読めることを、試験の中で組んだ行で確かめた。`traces.jsonl` の削除も traces-async unit が行う。
3. `cosDigestPass` は `read_run` の応答の `thread`・`run_id` が要求と違えば throw する。`run_state` は一覧と比べない（Rust の read の応答の `run_state` の出どころを pass-wire の実装で確かめていない。未確認）。

## 提案
- なし。
