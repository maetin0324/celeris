---
title: S5 adr-s5 — 境界 ADR に generation・canary・drain・checkpoint・監視・S6 の測定の定義（付記 S5-1〜S5-12）
tasks: [01M6FPPJN7A8H427MXFVYFY7H4]
status: done
completed: 2026-11-03
updated: 2026-11-03
---
# S5 adr-s5 — 境界 ADR に S5・S6 の付記

完了日: 2026-11-03（unit adr-s5）。コードは変えていない。

## 結論
[境界 ADR](../../adr/2026-11-02-ts-control-layer-boundary.md) に「付記 2026-11-03: S5 の generation 管理・段階切替・rollback と S6 の測定の定義」（S5-1〜S5-12、12 節）を足した。(f) の数値は変えていない。
主な決定: 新 bundle は API で登録し新しい isolate で検証する。生きた generation は最大 2 つ。canary は決定的な hash で選ぶ（`cos_digest` は pass 単位）。`compare`→`canary`・`canary`→`ts` は人の token だけ。停止基準の計数は表 `script_guard_state` に持ち、再起動の後も残す。

## 目的の項目と節の対応
| 目的の項目 | 節 | 要点 |
|---|---|---|
| (1) 新 bundle の登録経路と generation の状態 | S5-1 | `POST /api/v1/script/generations`、trait `ScriptGenerationControl` は task-api、daemon が実装、未接続は 503。状態は `loaded`・`active`・`draining`・`retired`・`invalid`（`rejected` は `invalid` へ移す）。生きた generation は最大 2。`[script] active_bundle_sha256` を消す |
| (2) canary の選び方と権限 | S5-3・S5-4 | `canary_bps`（1〜9999）と `sha256("celeris.canary/1"‖domain‖generation‖unit_key)`。`lane_rules`・`route_priority` は task 単位、`cos_digest` は pass 単位（理由: 検証と apply が pass 全体）。`→canary`・`→ts` は人の token だけ、戻しは誰でも・自動 |
| (3) 要求ごとの固定・in-flight・30 秒の打ち切り | S5-2 | generation ごとに in-flight 64。drain は CAS から 30 秒で打ち切り `GenerationGone`→Rust 参照。起動時の復元は load できた generation の mode を保つ |
| (4) checkpoint の形と照合 | S5-5 | `celeris.script-checkpoint/1` の欄、`checkpoint_versions` ⊇ 保存済みの版。S5 の 3 領域には長期 workflow が無いので表は作らず、照合の関数と拒否の経路だけを作る |
| (5) 副作用の前後・認可の error | S5-6 | 副作用前は Rust へ fallback。副作用後は冪等 key と記録で照合、できなければ止めて受信箱と `rust` への戻し。認可の error で fail-open しない |
| (6) 停止基準の監視 | S5-7 | `script_guard_state`（再起動の後も残す）、actor `system:script-guard`、安全違反で generation を `invalid` にして受信箱、検証を通らない副作用で全体を `halted` |
| (7) S4 未解決 7（tick の遅れ） | S5-8 | 1 tick で TS を同期で待つ合計を 200 ms まで。超えた分は `tick_budget`（`unavailable`）で Rust。p50・p95 を数えて S6 で報告。停止基準には入れない |
| (8) S4 未解決 8（bundle の無い環境） | S5-9 | `CELERIS_SCRIPT_BUNDLE_DIR`、dist の `inputs.sha256` で古さを判定して SKIP、REQUIRE か BUNDLE_DIR があれば SKIP しない、test-affected が `$TMPDIR` に bundle を作る、SKIP を数える |
| (9) S6 の測定の定義 | S5-10・S5-11 | 対 `policy`↔`policy-ts`・`cos-digest`↔`cos-ts`、検証責任、1 試行 300 秒、leaf ごとの試行数、識別の欄。測定規則 7 項 |
| (10) 後続 unit の作業範囲と試験名 | S5-12 | bundle-skip・runtime-gen・store-gen・api-canary・daemon-gen・canary-dispatch・guard・gen-e2e・cos-ts-scenario・s5-close の 10 行 |

## 受け入れ条件ごとの証拠（提出時の HEAD で実行）
- 条件 0（`### S5-` の節が 8 つ以上）: `grep -c '^### S5-' agent-docs/adr/2026-11-02-ts-control-layer-boundary.md` → 12
- 条件 1（(f) の数値を変えていない）: `git diff "$CELERIS_WU_BASE" -- agent-docs/adr/2026-11-02-ts-control-layer-boundary.md | grep -c '^-[^-]'` → 0（追記だけで、既存の行を 1 行も消していない）
- 条件 2（後続 unit の表）: S5-12 の表に 9 unit と s5-close の行がある
- 条件 3（コードの差分なし）: `git diff --name-only "$CELERIS_WU_BASE" -- crates web control-ts scripts` → 出力なし
- 文書の検査とビルド検査の結果は [S5 の進捗](../2026-11-02-ts-control-layer-s5.md) の「検査」に書く

## 未解決
1. S5-9 の `inputs.sha256` は `build-bundle.mjs` が作る新しい file で、入力の範囲（`control-ts/src/`・`bundle.config.json`・`package.json`・`pnpm-lock.yaml`）で十分かは bundle-skip unit が確かめる（未確認）。
2. S5-8 の 1 tick 200 ms の予算は、`lane_rules`・`route_priority` を `canary`・`ts` にしたときの 1 tick の判定数で足りるかが未計測。足りなければ多くの判定が `tick_budget` で Rust になる（停止基準には入らない）。S6 の overhead で数える。
3. S5-10 の `cos-digest` と `cos-ts` の編集の anchor は未定（cos-ts-scenario unit が決める）。
