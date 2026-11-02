---
title: S2 route-prio ts — route_priority の TS 実装・単体試験・compare route_priority
tasks: [01M6EJYCFFSJXYBVH87Q8FSNYB]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S2 route-prio ts

完了日: 2026-11-02

## 結論
- `control-ts/src/domains/route_priority.ts` に `route_priority_decide` と同じ順で列を選ぶ純粋関数を書いた: cos → その harness の class 列 → lane 列（lane が null なら空文字で引く）→ `route_defaults` → 空。選んだ列が空なら basis は `none`（付記 S2-10）。並べ替え・重複除去・registry の確認・許可判定・frontier 列の組み立ては入れていない。op を呼ばず、時刻・乱数を使わない。入出力の型は lane_rules.ts と同じく手で書いた。
- lane・class の照合は配列を前から `===` で探す（`__proto__`・`constructor` を object の key にしない）。同じ lane の行が 2 つあれば前の行を使う（Rust の写しは BTreeMap 由来なので重複しない）。
- 入力の余計な欄・欠けた欄・型の違い（`cos` が boolean でない、id が string でない、列が配列でない等）は throw する。
- `src/domains/index.ts` と `bundle.config.json` の domains に `route_priority`（capability は空）を足し、`build-bundle.test.mjs` の manifest.domains の期待を `["host_selftest", "lane_rules", "route_priority"]` に直した。manifest.capabilities は `["selftest:echo_task"]` のまま。
- `scripts/compare.mjs` の DOMAINS に `route_priority`（fixture `fixtures/route_priority/cases.jsonl`、入力 `row.input`、比較欄 `basis`・`candidates`）を足した。`compare.test.mjs` に全件一致・順序違いの検出・入力拒否の 3 件を足した。
- `src/domains/route_priority.test.ts`: 各 basis（cos・execution_class・lane・harness・none）、空の列から落ちないこと、lane の大小文字、null lane と空文字の行、prototype 名、順序と重複の保持、dispatcher 経由、入力拒否 19 種。
- `crates/` は変えていない。

## commit（件名）
1. `phase s2: route-prio ts: route_priority の TS 実装・単体試験・compare route_priority`
2. `phase s2: route-prio ts: 進捗の記録`

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `node control-ts/scripts/compare.mjs route_priority` → `route_priority: 2026/2026 matched`、exit 0。
- `node control-ts/scripts/compare.mjs lane_rules` → `lane_rules: 19683/19683 matched`、exit 0。
- `pnpm typecheck`（control-ts）→ exit 0。`pnpm test` → vitest 48 passed、node --test 28 passed。
- `bash scripts/dev/test-affected.sh` → exit 0（対象は control-ts の typecheck+test だけ）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- 範囲: `git diff --name-only $CELERIS_WU_BASE` は `control-ts/` の 7 file とこの記録だけ。

## 未解決事項
- なし。

## 提案
- Rust の `RoutePriorityInput` は `lane`・`execution_class` が `Option<String>` なので、serde は欄が欠けた入力を `None` として受ける（`deny_unknown_fields` は余計な欄だけを拒む）。TS は Objective に合わせて欠けた欄を throw するため、受け入れ範囲は TS の方が狭い。Rust が書く入力は常に両欄を持つので compare には影響しない。揃えるなら Rust 側に `#[serde(default)]` を付けない方針の確認か、試験で「Rust が出す入力は常に欄を持つ」ことを固定する。
- 現行の bug は見つけていない。
