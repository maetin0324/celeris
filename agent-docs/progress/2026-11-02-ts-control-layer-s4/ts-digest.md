---
title: S4 ts-digest — cos_digest 領域の TS 実装・単体試験・fixture 比較
tasks: [01M6EVP9GT07K2G01EMW0E6HNC]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S4 ts-digest — cos_digest 領域の TS 実装

完了日: 2026-11-02（unit ts-digest）。変えたのは control-ts/ とこの記録だけ。Rust・crates/ は変えていない。

## 結論
- `control-ts/src/domains/cos_digest.ts` を足した。`crates/task-core/src/chat/digest.rs` の `cos_digest_decide` と同じ command 列を、同じ文字列・同じ順・同じ欄順で返す。出力は付記 S4-2 の `CosDigestOutput`（`{ commands: [...] }`）。
- `src/domains/index.ts` と `bundle.config.json` の `domains` に `"cos_digest": []`（capability 空）を足した。bundle の manifest の `domains` は `cos_digest`・`host_selftest`・`lane_rules`・`route_priority`。
- `scripts/compare.mjs` を拡げた。fixture に dir を渡せる（直下の `*.jsonl` を名前順にすべて読む）。`cos_digest` の既定は `fixtures/cos_digest`。後から `traces.jsonl` が足されても同じ形で読む。
  - 期待値は fixture 行の生の byte 列から取る（行末の `"commands":<値>}` と一致しない行は不一致）。
  - 付記 S4-6 に従い、全行を正順と逆順の 2 回流して同じ出力を要求する。
  - 不一致は file・行番号・name を付けて最初の 5 件を出し、exit 1。

## 入力の検査の決め方
- 余計な欄・欠けた欄・型の違いは throw する。Option の欄も `null` の明示を要求する（Rust の serde より厳しい）。
- `run_state`・item の `state`・`source_kind` は拒まない。Rust でも文字列で、未知の値に決まった文面（`run は <state> で終わった`・`状態不明`・`項目`）を返す。fixture は `cancelled`（386 行）・`handled`・`task` を含み、拒むと受け入れ条件 0（全行の byte 一致）を満たせない。
- 未知の enum 値として拒むのは `route` だけにした。store（`cos_triage_notification_route`）が書く値は `escalation`・`fallback` の 2 つだけで、digest の出力にも使わないので、拒んでも一致は崩れない。
- 文字列の trim は Rust の `str::trim`（Unicode White_Space）と同じ集合で自前に行う。JS の `trim` は U+FEFF を削り U+0085 を削らないので使わない。

## 受け入れ条件と証拠（提出時の HEAD で実行）
- 条件 0（cases.jsonl の全行が TS と byte 一致）: `corepack pnpm@12.6.0 run compare cos_digest` → `cos_digest: 2066/2066 matched (fixtures/cos_digest/cases.jsonl)`、exit 0。
- 条件 1（typecheck・test・bundle が通り、manifest の domains に cos_digest がある）:
  - `corepack pnpm@12.6.0 run typecheck` → exit 0。
  - `corepack pnpm@12.6.0 run test` → vitest 81 passed（4 files）、node --test 34 passed。
  - `corepack pnpm@12.6.0 run bundle` → exit 0。`dist/manifest.json` の domains は `["cos_digest","host_selftest","lane_rules","route_priority"]`。
- 単体試験 `src/domains/cos_digest.test.ts`: run_state 4 種と未知の値、空の run、item state 8 種（未知の `handled` を含む）、source_kind の写像、理由なし（null・空・空白）、trim、title の代わり、選択肢あり（推奨・detail あり・空）、未知の欄・欠けた欄・入れ子の余計な欄・未知の route・型違い、dispatch 経由と欄の順。
- `compare.test.mjs` に cos_digest の 6 件（全行一致、dir の複数 file と後から足す traces.jsonl、requeue の欠け、入力の余計な欄、生 byte が正規形でない行、CLI の exit 1）を足した。`build-bundle.test.mjs` の domains の期待値に `cos_digest` を足した。
- `bash scripts/dev/test-affected.sh` → exit 0（対象は control-ts の typecheck+test だけ）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。

## commit（件名）
- phase s4: ts-digest: cos_digest 領域の TS 実装・単体試験・compare の dir 読み

## 付記との食い違い
- 付記 S4-8 は TS の試験の置き場所を `control-ts/test/cos_digest.test.ts` と書くが、unit の objective に従い `control-ts/src/domains/cos_digest.test.ts` にした（他の領域と同じ置き場所）。
- 付記 S4-2 は `run_state` を 4 値の enum と書くが、TS は Rust の実装（文字列）に合わせた（rust-split の記録と同じ理由）。

## 未解決事項
- 本物の deno_core host での cos_digest の実行は未確認（host-adapter unit が行う）。

## 提案
- close unit で付記 S4-8 の試験の path を実際の置き場所に合わせる。
