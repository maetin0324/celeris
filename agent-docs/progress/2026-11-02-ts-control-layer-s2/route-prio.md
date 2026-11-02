---
title: S2 route-prio — llm_route::route_priority の TS 同等実装と compare
tasks: [01M6EJYCFFSJXYBVH87Q8FSNYB]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S2 route-prio

完了日: 2026-11-02

## 結論
- S2 の 2 つ目の領域として `crates/task-core/src/llm_route.rs` の `route_priority`（候補順の決定。cos → execution class → lane → harness の `route_defaults`）を TS に同等実装し、compare を付けた。境界 ADR（`agent-docs/adr/2026-11-02-ts-control-layer-boundary.md`）に付記 S2-10（選んだ列が空なら basis は `none`。下の列へ落ちない）と S2-11（結線の場所）を足した。
- Rust は `route_priority_decide` が列と basis を返し、`route_priority` は同じ signature の薄い包みになった（挙動不変）。候補が registry に在るかの確認・許可判定・frontier 列の前置き・`order_by_headroom` は Rust に残る。
- fixture は example `dump-route-priority-fixtures` が出す `control-ts/fixtures/route_priority/cases.jsonl`（手書き 26 件 + 固定 seed の生成 2000 件 = 2026 行）。
- TS は `control-ts/src/domains/route_priority.ts`。`node control-ts/scripts/compare.mjs route_priority` で fixture 全件を比べる。
- 結線は `dispatch_run.rs` の `select_route_with` の直後の 1 か所。mode が compare のときだけ `script_compare_recorded`（domain `route_priority`、call_site `route_select.route_select_one`）を 1 判定 1 件記録する。mode rust では TS を呼ばない。TS の例外・不正出力・timeout では Rust の順序をそのまま使う。
- 本物の deno_core host の試験 `crates/celeris/tests/script_compare_route_priority.rs`（5 件）は lane_rules と同じく `CELERIS_SCRIPT_REQUIRE_BUNDLE=1` のとき dist 無しを失敗にする。
- 本番の config の変更は無い（route-prio の commit は `config/` と本番 host の file に触れていない。mode の既定は rust のまま）。

## unit の記録
- [core](route-prio/core.md) — route_priority の分割、出力 DTO `RoutePriorityOutput`、fixture 生成と試験、付記 S2-10
- [ts](route-prio/ts.md) — TS 実装・単体試験・`compare route_priority`
- [hook](route-prio/hook.md) — `PolicyScript::route_priority`・`classify_route_priority`・dispatcher の結線、付記 S2-11
- [real-host](route-prio/real-host.md) — 本物の host の試験と共通 helper の `tests/support/script_compare.rs` への移動

## commit（件名）
1. `phase s2: route-prio core: route_priority を列と basis を返す route_priority_decide と薄い包みに分ける（挙動不変）・付記 S2-10`
2. `phase s2: route-prio core: RoutePriorityOutput・basis の enum と route_priority の fixture（example dump-route-priority-fixtures・cases.jsonl・試験）`
3. `phase s2: route-prio core: 進捗の記録`
4. `phase s2: route-prio ts: route_priority の TS 実装・単体試験・compare route_priority`
5. `phase s2: route-prio ts: 進捗の記録`
6. `phase s2: route-prio hook: PolicyScript に route_priority・classify_route_priority・領域を持つ CompareDraft・dispatch_run の select_route_with の直後の compare・試験・付記 S2-11`
7. `phase s2: route-prio hook: 進捗の記録`
8. `phase s2: route-prio hook: 進捗の記録（検査の結果）`
9. `phase s2: route-prio hook: 範囲外の tests/mod.rs・review_route.rs の変更を戻し、route の試験を script_compare の子 module にする`
10. `phase s2: route-prio real-host: 本物の host の試験 script_compare_route_priority と共通 helper の tests/support への移動・進捗の記録`
11. `phase s2: route-prio close: 進捗のまとめ`（この記録）

統合の commit（`integrate wu/…`）は celeris が作る。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `node control-ts/scripts/compare.mjs route_priority` → `route_priority: 2026/2026 matched`、exit 0。
- `node control-ts/scripts/compare.mjs lane_rules` → `lane_rules: 19683/19683 matched`、exit 0。
- `cd control-ts && corepack pnpm@12.6.0 install --offline --frozen-lockfile && corepack pnpm@12.6.0 run bundle` → dist を生成（git の追跡外）。
- `CELERIS_SCRIPT_REQUIRE_BUNDLE=1 cargo nextest run -p celeris --test script_compare_lane_rules --test script_compare_route_priority` → exit 0、10 passed・0 skipped（lane_rules 5・route_priority 5）。
- `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check` → 下の「検査」節。
- 各 unit が自分の HEAD で実行した結果（byte 一致・task-core 8 passed・task-dispatch script_compare 29 passed など）は unit の記録にある。

## 未解決事項
- 入出力の型が手書きのまま。`route_priority.ts` の入出力の型と検査は Rust の `RoutePriorityInput`・`RoutePriorityOutput` から生成していない（lane_rules と同じ未解決）。Rust の DTO を変えると fixture の比較と本物の host の試験で食い違いが出るが、型検査では検出しない。
- review_spawn の run は compare しない（付記 S2-11）。compare の対象は worker の run の `select_route_with` だけで、cos の判定・review_spawn・lane_assist の試走・frontier 列の前置き後の候補列は比べない。
- compare が比べるのは `route_priority` の列で、frontier lane で frontier 列を前置きした後の `RouteTrace.candidates` ではない。前置きを TS に移すなら別の領域として DTO を決める（hook の提案）。
- `release.sh`・`test-affected.sh` は `control-ts/dist` を作らないため、dist が無い環境では dist を使う 5 件（lane_rules 3・route_priority 2）が SKIP で通る。release gate で bundle を作り `CELERIS_SCRIPT_REQUIRE_BUNDLE=1` で走らせるかは未決定（host-adapter の提案 1 と同じ）。
- `cargo clippy -p celeris --tests` は既存の `campaign_lock_e2e`（`../../campaign/tests/support_cases/mod.rs` を複数回読む）で落ちる。route-prio とは無関係で、直していない。

## 提案（各 unit が見つけた現行の挙動。どれも直していない）
1. lane が `None` の run は `unwrap_or_default` で空文字 `""` の lane 行を引く（core、手書き例 `hand/lane-null-takes-empty-string-row`）。config が `""` の lane を許すと、lane の無い run が意図せずその行を使う。config の読み込みで空文字の lane を拒むか、`None` を「lane 行を引かない」にするかを決める。
2. 空の宣言の列（class・lane・cos）は下の列へ落ちず `NoDeclaredDefault` で fail-closed になる。harness の `route_defaults` が空配列のときと未宣言のときは区別できない（どちらも basis `none`）。誤設定で run が止まるので、config の読み込みで空の列を拒むかを検討する（core）。
3. Rust の `RoutePriorityInput` は `lane`・`execution_class` が `Option<String>` なので欄の欠けた入力を `None` として受けるが、TS は throw する。Rust が書く入力は常に両欄を持つので compare には影響しない。試験で「Rust が出す入力は常に欄を持つ」ことを固定するとよい（ts）。
4. `route_select` が全 harness で断ったとき、`RouteTrace.harness` は最後に試した harness になり、`RouteTrace.candidates` は空のまま残る（成功の分岐だけが設定する）。bug かは未確認（hook）。
5. 入出力の型を Rust の DTO から生成する（lane_rules と合わせて 1 つの task にする）。
6. release gate で bundle を作り `CELERIS_SCRIPT_REQUIRE_BUNDLE=1` で本物の host の試験を走らせる（real-host、host-adapter と共通）。
7. `campaign_lock_e2e` の `--tests` clippy の失敗を別 task で直す（real-host）。

## 検査
提出時の HEAD で実行した。
- `bash scripts/dev/test-affected.sh` → exit 0。「試験対象の変更なし（docs のみ等）」（この unit の差分は本記録だけ。文書の検査と check-doc-links の self-test は通った）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/progress-index.sh --check` → ok。
- コードの差分: `git diff --name-only $CELERIS_WU_BASE` は本記録だけ。
