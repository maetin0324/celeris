---
title: S2 route-prio real-host — 本物の deno_core host の試験 script_compare_route_priority
tasks: [01M6EJYCFFSJXYBVH87Q8FSNYB]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S2 route-prio real-host

完了日: 2026-11-02

## 結論
- `crates/celeris/tests/script_compare_route_priority.rs` を `script_compare_lane_rules.rs` と同じ形で作った（userns なし、外部ネットワークなし）。本番コードは変えていない。
- 共通の helper（dist の読み込みと SKIP、host の起動、config、`Env`、`hinted_task`、`timeout` だけのやり直し）を `crates/celeris/tests/support/script_compare.rs` に移した。`Env` は領域（`lane_rules`／`route_priority`）を持ち、mode の行と generation の manifest をその領域で作る。`script_compare_lane_rules.rs` は support を使う形に直しただけで、試験の中身は変えていない。
- 不具合は見つからなかった（plan_issue なし）。

## 試験（`script_compare_route_priority.rs`、5 件）
| 項目 | 試験 | 確かめること |
| --- | --- | --- |
| (1) | `script_compare_route_priority_real_host_matches_rust_on_all_fixtures` | `cases.jsonl` 2026 件を同じ isolate に正順と逆順で流し（30 秒の deadline で `PolicyScript` を直接呼ぶ）、4052 回とも `match`・安全違反なし。isolate の作り直し 0 回。各行の期待値は入力から組み直した registry で `route_priority_decide` に当て直して一致を先に確かめる |
| (2)(3)(5) | `script_compare_route_priority_dispatch_match_rollback_and_replay` | mode rust では bundle が在っても呼ばない。compare で `script_compare_recorded` が 1 件（domain `route_priority`、call_site `route_select.route_select_one`、run_id はこの task の run、`match`、epoch 1）。`harness_selected`・`llm_route_selected` の直列化は mode rust・PolicyScript 無しの基準と byte 一致（task・run の ULID だけ伏せる）。rust に戻すと記録 0 件、actor の完了数は増えない |
| (5) | 同上 | source に `endpoint`（base URL）と `credential_ref`（環境変数名）を書いた config で、記録の入力にそれらと選ばれた account・task の title が無い。入力 DTO に戻すと同じ JSON、`recorded_input` の digest が記録と一致、TS に通し直すと同じ byte 列 |
| (4) | `script_compare_route_priority_throwing_bundle_keeps_rust_route` | file 内の本文の例外 bundle で `ts_error`（`exception`）、`ts` は null |
| (4) | `script_compare_route_priority_different_answer_keeps_rust_route` | file 内の本文で basis が常に `lane`（候補は宣言済み）→ `mismatch`、安全違反なし |
| (4) | `script_compare_route_priority_undeclared_candidate_keeps_rust_route` | file 内の本文で table に無い候補 → `mismatch`、`safety_violations` が `["undeclared_candidate"]` |

(4) の 3 件はどれも route の event が基準と byte 一致（Rust の route のまま）。dist が無くても走る。
待ちの方針は lane_rules と同じ: outcome が `timeout`（50 ms の deadline 超え）のときだけ新しい task でやり直す（最大 20 回）。他の outcome はやり直さない。

## commit（件名）
1. `phase s2: route-prio real-host: 本物の host の試験 script_compare_route_priority と共通 helper の tests/support への移動・進捗の記録`

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cd control-ts && corepack pnpm@12.6.0 install --offline --frozen-lockfile && corepack pnpm@12.6.0 run bundle` → dist/bundle.mjs と manifest.json を生成（dist は git の追跡外）。
- `CELERIS_SCRIPT_REQUIRE_BUNDLE=1 cargo nextest run -p celeris --test script_compare_lane_rules --test script_compare_route_priority` → exit 0、10 passed、0 skipped（受け入れ条件 0）。
- dist を `$TMPDIR` へ退避して手で確かめた（受け入れ条件 1）:
  - env 無し → exit 0、10 passed。dist を使う 5 件（lane_rules 3・route_priority 2）が `SKIP <試験名>: control-ts/dist is not usable: …manifest.json: No such file or directory` を出して抜けた。
  - `CELERIS_SCRIPT_REQUIRE_BUNDLE=1` → exit 100、5 passed・5 failed（失敗は同じ 5 件）。
  - 退避後に dist を戻した。
- `CELERIS_SCRIPT_REQUIRE_BUNDLE=1 bash scripts/dev/test-affected.sh` → exit 0（対象 celeris・celerisctl、866 passed・0 skipped）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo clippy -p celeris --test script_compare_lane_rules --test script_compare_route_priority -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- 範囲（`sh "$CELERIS_WU_SCOPE_PATHS"`）→ 2 つの試験 file、`tests/support/` の 2 file、本記録だけ。

## 未解決事項
- `cargo clippy -p celeris --tests` は既存の `campaign_lock_e2e`（`../../campaign/tests/support_cases/mod.rs` を複数回読む）で落ちる。この unit の変更とは無関係で、直していない。

## 提案
- `release.sh`・`test-affected.sh` は `control-ts/dist` を作らないので、dist が無い環境では dist を使う 5 件が SKIP で通る。release gate で bundle を作り `CELERIS_SCRIPT_REQUIRE_BUNDLE=1` で走らせるかは close unit で決める（host-adapter の提案 1 と同じ）。
- 上の `campaign_lock_e2e` の clippy の失敗は `--tests` を付けた lint を妨げる。別 task で直すとよい。
