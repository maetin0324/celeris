---
title: daemon-gen — daemon に generation の登録・切替後の drain・起動時の復元を結線する
tasks: [01M6FPPJN7A8H427MXFVYFY7H4]
status: done
updated: 2026-11-03
completed: 2026-11-03
---
# daemon-gen — daemon の generation の登録・切替・drain・起動時の復元

## 結論
`crates/celeris/src/daemon/script_layer.rs` に `ScriptGenerations` を作り、runtime-gen の `GenerationRegistry` を daemon に結線した（境界 ADR の付記 S5-1・S5-2・S4-14）。`[script] active_bundle_sha256` は消した（書くと読み込みエラー）。

- **登録**（`ScriptGenerationControl`、`ApiState::with_script_generations` で結線）: `<state_dir>/script-bundles/<sha256>/` の bundle を新しい isolate で検証してから `script_generations` に `loaded` で記録する。
  - 検証に落ちたら `invalid` を書き 422。登録簿の上限は 409 `script_generation_limit`。
  - store が登録を拒んだら isolate を捨てる（試験 `script_daemon_store_refusal_discards_isolate`）。
- **切替**: mode の CAS（store の 1 transaction）が行の generation と状態を変える。dispatcher は tick ごとに行を読み、その generation に固定した口で呼ぶ。
  - 切替の前に送った要求は旧 generation で終わる。後の要求は新へ行く。
- **drain**: 見回り（`RECONCILE_INTERVAL` = 1 秒ごと）が表の `draining` を登録簿で drain する。
  - in-flight が 0 になったら isolate を捨て、`retired`（actor `system:script-runtime`、`cancelled_in_flight` 0）を書く。
  - 30 秒を過ぎて残る要求は打ち切る。その要求は `GenerationGone` になり、dispatcher は Rust の参照実装で答える。打ち切った件数を `cancelled_in_flight` に書く。
  - 表で `retired`・`invalid` になった generation（API の退役・script-guard の無効化）の isolate も、同じ見回りで捨てる。
- **起動時の復元**（`ScriptGenerations::restore`）: mode の行が指す generation を新しい isolate で load する。
  - load できた generation の mode（`compare`・`canary`・`ts`）は保つ。S4 の「`needs_generation()` の行を全部 `rust` へ」は改めた。
  - 次の場合はその領域を actor `system:script-startup` で `rust` に戻す。file が無い・hash が違う・`host_api` が合わない・検証に落ちる・manifest にその領域が無い・その領域の capability が足りない・checkpoint の版が足りない・generation が `loaded`/`active` でない・script-guard が止まっている。
  - 戻すと `script_domain_mode_changed` が残る。その `reason` の先頭は code（`startup_bundle_unusable` 等）。受信箱の項目は script-inbox の経路（`script_guard`）がこの event から作る。
  - 続けて、行から指されない `draining` を drain の終わり（`script_generation_retired`、`cancelled_in_flight` 0、reason `drained: …`）として `retired` にする。
  - isolate の無い `loaded` も `retired`（reason `startup_no_isolate: …`）にする。登録し直せば `loaded` に戻る（S5-1 の 6）。
  - `[script]` の無い daemon は、generation の要る mode を全部 `rust` に戻す（code `startup_bundle_unusable`）。
- **standby**: standby の daemon は isolate を作らず、復元も登録も行わない（503 `script_runtime_unavailable`）。active になってから最初の見回りで復元する。verify は DB の写しを相手にするので active と同じに扱う。

## 設計の判断
- **trait の変更**: `PolicyScript` は変えず、`crates/task-dispatch/src/script_policy.rs` に `PolicyScriptSource`（generation → 固定した口）を足した。
  - dispatcher は `Option<Arc<dyn PolicyScriptSource>>` を持つ。
  - bundle 1 つの口（試験）は `SingleBundleSource` で包み、hash が同じときだけ返す。今までの「loaded bundle … differs」の判定はここに移した。
  - daemon は `set_policy_script_source` で `ScriptGenerations` を渡す。
- **登録簿の key**: `bundle_sha256` にした（`script_generations` で一意）。
  - 検証してから記録する順序（S5-1 の 3→4）では、検証の時点で store の id がまだ無い。
  - runtime-gen の記録の「登録簿の id = DB の id」とは違う。s5-close で ADR の付記と突き合わせる（提案 1）。
- **`ScriptPolicy` の送り先**: 2 種類にした。
  - 登録簿の 1 つの generation（daemon）: `call` を呼んだ時点で generation と in-flight の枠を取る。
  - actor 1 つ（試験の `start_policy`）。
- **op を待つ間の打ち切り**: `cos_digest_pass` は、op を op server へ写す間も actor の結果を見るようにした。
  - 前の形は op の応答を待つ間に `GenerationGone` を見ず、答えない op server を待ち続けた。drain の打ち切りの試験で見つけた。
- **停止**: 見回りを止めてから全 isolate を捨てる。表は変えない（次の起動の復元が合わせる）。
- **通知の削除**: 起動時の bundle の失敗の通知（`script_bundle_rejected` の NoticeEvent）は消した。知らせは受信箱 `script_control`（script-inbox の経路）に一本化した。

## 試験（付記 S5-12 の名前との対応）
| S5-12 の名前 | 実装 | 確かめること |
|---|---|---|
| `daemon_registers_generation_in_new_isolate` | `script_generation_daemon.rs` の `script_daemon_registers_generation_in_new_isolate` | 新しい isolate（module の状態を共有しない）、`loaded` の行、同じ hash は今の行、3 つ目は上限、入口の無い bundle は `invalid` で isolate を残さない |
| （追加） | `script_daemon_store_refusal_discards_isolate` | store が拒んだら isolate を捨てる |
| `daemon_switch_drains_old_generation` | `script_daemon_switch_drains_old_generation` | 旧の in-flight（答えを待つ op）は旧で終わる。新しい要求は新へ行き、drain 中の旧は `generation_gone`。in-flight 0 で `retired`（`cancelled_in_flight` 0） |
| （追加） | `script_daemon_drain_cutoff_30s_generation_gone`（`ManualClock`） | 29.999 秒では打ち切らず、30 秒で `GenerationGone`・`cancelled_in_flight` 1・reason `drain_cutoff` |
| （追加） | `script_daemon_api_registers_and_switches_while_running` | 動いている daemon に API で 2 つ登録し、A→B の CAS で A が見回りで `retired` になる。再起動で B の compare を復元する。起動の待ちは `daemon_port::wait_api_ready` |
| `daemon_restore_keeps_loaded_modes` | `script_runtime.rs` の `script_runtime_daemon_restore_keeps_loaded_modes` | 2 回の起動とも compare と `active` を保ち、isolate thread 1 本、停止で join |
| `daemon_restore_rolls_back_missing_or_invalid_generation` | `script_runtime_restore_{missing_bundle,tampered_bundle,host_api_mismatch,missing_capability,throwing_bundle,pending_top_level_await}_rolls_back` | 4 つの失敗（file が無い・hash 違い・`host_api` 違い・capability 不足）と評価の失敗 2 つで `rust`。`system:script-startup` の event（code と理由）と受信箱の項目（`code` が同じ）が残る |
| （追加） | `script_runtime_daemon_restore_retires_unpointed_draining` | 指されない `draining` → `retired`（actor `system:script-startup`、`cancelled_in_flight` 0、reason `drained`）。isolate の無い `loaded` → `retired`（`startup_no_isolate`） |
| （追加） | `script_layer_tests.rs` の `script_restore_reason_*` 3 件 | 戻す理由の code の表と、作り物の V での checkpoint の照合（S5-5） |
| `config_active_bundle_sha256_is_rejected` | `config/script.rs` の同名 | 消した欄は読み込みエラー |

既存の試験の追従:
- `script_compare_lane_rules.rs`・`script_compare_cos_digest.rs`: 起動で行を作る代わりに `Env::deployed_generation` で登録と同じ行を作る。「配備先に無い bundle」は配備先の dir を消して作る。
- `support/script_compare.rs` の `Env::rearm_generation`: `draining` と `retired` の両方を受ける（daemon の起動時の復元が `retired` にする）。
- `script_runtime.rs`: 起動時の通知の検査を、mode・event・受信箱の検査に替えた。

## 証拠（基準は `$CELERIS_WU_BASE`、提出時の HEAD で実行）
- `cargo nextest run -p celeris -E 'binary(script_generation_daemon)'` → 5 passed。
- `cargo nextest run -p celeris -E 'binary(script_runtime) | binary(/^script_compare_/)'` → 43 passed。
- `bash scripts/dev/test-affected.sh` → exit 0。
  - 対象: celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker、web の typecheck と unit。
  - nextest 5070 passed・20 skipped。control-ts の source から bundle を作り、script bundle の SKIP は 0 件。
  - web は vitest 697 passed。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- 範囲 check（計画の `scope:true` のコマンド）→ 範囲外の path なし。`check-architecture-map.py` → OK、`check-doc-links.sh` → ok、`progress-index.sh --check` → ok。
- `docs/architecture-map.md` の TS 制御層の行に `script_generation_daemon.rs` と S5-1・S5-2 の不変条件を足した。

## 未解決
1. 登録で bundle の file が無い hash も `invalid` になる（S5-1 の 4 と trait の文面どおり）。`invalid` は同じ hash で登録し直せないので、`script-deploy` の前に登録を頼むと、その hash は使えなくなる。
2. 登録簿の経路の失敗の `isolate_epoch` は、最後に応答した actor の番号から作る。timeout の作り直しの直後の 1 件は 1 小さい値になりうる（登録簿が actor の stats を出さないため。`crates/script-runtime` は範囲外）。
3. drain の 30 秒は、登録簿が見回りで drain を始めた時刻から数える。CAS の時刻より最大 `RECONCILE_INTERVAL`（1 秒）遅れる。

## 提案
1. s5-close で付記 S5-1・S5-2 に次を書く: 登録簿の key は `bundle_sha256`、起動時に isolate の無い `loaded` を `retired` にすること、drain の 30 秒の起点は見回り。
2. 未解決 1 は「file が無い（`ManifestError` の読み出しの誤り）は `invalid` にせず 422 だけ返す」へ改める案を、s5-close で決める。
