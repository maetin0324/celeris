---
title: S2 host-adapter — PolicyScript を ScriptRuntimeHandle で実装し、本物の host で fixture 全件と dispatcher 経路を試す
tasks: [01M6E6HEH7CPZX9KX4Y2SAZKTG]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S2 host-adapter

## 結論
- `crates/celeris/src/daemon/script_policy.rs` に `ScriptPolicy` を作った。`PolicyScript` を `ScriptRuntimeHandle` で実装する。要求の scope は空で送り（S2-7）、`ScriptError` を `PolicyScriptError` に写す。`isolate_epoch` は actor の generation 番号 +1。`celeris::ScriptPolicy` として公開した（結合試験が使う）。
- `daemon/script_layer.rs`: `[script]` が在れば判定の口を作る。`run.rs` が `dispatcher.set_policy_script` で注入する。`[script]` が無ければ注入しない。
- `daemon/bootstrap.rs`: `build_dispatcher` が `[script]` の有無によらず `set_script_domain_store` を呼ぶ。
- `run.rs`: actor を止める前に `wait_script_compares` を待つ。先に止めると走っている compare が `terminated` と記録される。
- Objective に明記は無いが、これが無いと本物の経路で compare にできないので、次の 2 つを `script_layer` に入れた。どちらも付記 S2-4・S2-5 の定めで、担当の unit が他に無い。
  - 検証に通った bundle の `loaded` の行を `script_generations` に作る。mode API はこの行が無いと 422 を返す。
  - `compare` の領域の generation の bundle が、読み込めた bundle と違えば、actor `system:script-startup` で `rust` へ戻す。`[script]` が無い場合と bundle を読めない場合を含む。

## 付記との違い
- S2-4 の「検証に落ちたら `rejected`」は行を作らないことにした（付記 S2-9）。理由: 同じ hash の行は再利用され、`rejected` を `loaded` に戻す経路が store に無い。一時的な欠損で作ると、その hash を二度と compare にできない。
- S2-6 の「最大 50 ms 待つ」は dispatch-hook で既に 0 ms（待たない）になっている。修正は close unit の担当（dispatch-hook の提案 1）。

## 試験
`crates/celeris/tests/script_compare_lane_rules.rs`（本物の deno_core host。userns なし。外部ネットワークなし）。dist を使う試験は、dist が無ければ SKIP を出して抜け、`CELERIS_SCRIPT_REQUIRE_BUNDLE=1` なら失敗する（dist を退避して両方を確かめた）。
| 受け入れの項目 | 試験 | 確かめること |
|---|---|---|
| (1) fixture 全件 | `script_compare_real_host_matches_rust_on_all_fixtures` | 19,683 件を同じ isolate に正順と逆順で流し、すべて `match`。fixture の期待値と `match_rule` の一致も見る。isolate の作り直しは 0 回 |
| (2) dispatch で match | `script_compare_dispatch_match_rollback_and_replay` | 記録 1 件で `match`。`routing_decided` は基準と byte 一致（dispatch ごとの id 3 欄を除く） |
| (3) rust に戻す | 同上 | 戻した後は記録 0 件、actor の完了数が増えない。mode rust の間も呼ばない |
| (4) 例外の bundle | `script_compare_throwing_bundle_keeps_rust_lane` | `ts_error`（`exception`）、`routing_decided` は基準と byte 一致 |
| (5) replay | `script_compare_dispatch_match_rollback_and_replay` | 記録の入力は 9 軸だけで digest が合う。TS に通し直すと記録と同じ byte 列 |

追加の試験:
- `script_compare_different_answer_keeps_rust_decision_bytes`: 常に frontier を返す bundle で `mismatch`。Rust の lane は cheap のままで、`routing_decided` も基準と byte 一致（付記 S2-6 がこの unit に求めた固定）。
- `script_compare_daemon_injects_records_and_restores_rust`: `celeris::run` を通す。起動で `loaded` の行ができ、compare の dispatch が `match` を記録し、同じ bundle なら compare のまま。`[script]` を外すと、または配備先に無い bundle を指すと、`system:script-startup` が rust へ戻す。
- `script_policy_error_kind_matches_script_error_serialization`・`script_policy_denied_maps_to_safety_violation_code`（`script_policy.rs` の単体試験）。

待ちの方針: dispatcher の deadline は 50 ms の固定で（差し替えの口は範囲外の task-dispatch にある）、host が忙しいと正しい bundle でも `timeout` になりうる。試験は outcome が `timeout` のときだけ新しい task でやり直す（最大 20 回）。他の outcome はやり直さない。全件比較と replay は `PolicyScript` を 30 秒の deadline で直接呼ぶ。

## commit（件名）
1. `phase s2: host-adapter: PolicyScript を ScriptRuntimeHandle で実装し daemon から注入、本物の host で試す`

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
EVIDENCE

## 未解決
1. `release.sh` と `test-affected.sh` は `control-ts/dist` を作らないので、dist が無い環境では dist を使う 3 試験が SKIP になる。release gate で `CELERIS_SCRIPT_REQUIRE_BUNDLE=1` にするかは close unit で決める（下の提案 1）。範囲外なのでここでは変えていない。
2. `rejected` の行を作らないので、S2-8 の集計に拒否の件数は入らない。拒否の記録は `script_bundle_rejected` の通知だけ。
3. `crates/celeris/tests/script_runtime.rs` の `script_runtime_valid_bundle_loads` の flaky（dispatch-hook の未解決 4）は follow-up に起票済みで、ここでは直していない。

## 提案
1. close unit で、bundle を作る段と `CELERIS_SCRIPT_REQUIRE_BUNDLE=1` の試験を release gate に入れる。
2. S5 で state の遷移を足すときに、`rejected` の行の作成と `loaded` への戻しの store 関数を一緒に作る。

## 本番への移行
config の変更は無い。`[script]` の無い本番 daemon では TS を呼ばない。全領域が `rust` のままなので、起動時の戻しも何も変えない。compare を本番で有効にする手順は close unit が書く。
