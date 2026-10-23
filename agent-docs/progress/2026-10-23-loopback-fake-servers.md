---
title: loopback fake server の迷い込み対策
updated: 2026-10-23
status: done
tasks: [01M5M07Y3X78AQ7V0AM1347EXW]
---

# loopback fake server の迷い込み対策

`classify_tick::e2e::jev_server` は EOF や別 path も requests に追加していた。
完全な `POST /decisions` だけを記録し、空接続・GET /・header/body の途中 EOF を冒頭に注入して
count=0 を確認してから既存の 429 の一連試験を行う。正常要求の count=1 の assertion は維持する。
通知 webhook、Jev client、CLI、モデル一覧の同種 reader は tests/support に集約した。
共有 helper の変更も test-affected の全 crate 検査対象に含める。

## 走査一覧

`rg 'TcpListener' crates tests`、`rg 'accept\(|incoming\(' crates tests` と JS/Python の
`createServer` / `HTTPServer` も確認。以下は Rust の TcpListener を含む全ファイル（本番・予約用途も除外理由を掲載）。

| ファイル | 対応 | 根拠 |
| --- | --- | --- |
| `crates/campaign/tests/ablation_application_support/mod.rs` | 変更不要 | port 予約または実 API router の起動。accept ごとの記録・有限台本なし。 |
| `crates/campaign/tests/arm_set_cli_normal.rs` | 修正 | 既存の非 API・token 無し要求の除外は維持。共通 reader に移し途中 body EOF も除外。 |
| `crates/campaign/tests/env_lock.rs` | 変更不要 | 実 task-api/MCP/axum router が HTTP framing と routing を実施。記録・状態更新は対象 route 内（JSON body の読取後）。空接続や GET / はその handler に入らない。 |
| `crates/campaign/tests/launcher.rs` | 変更不要 | port 予約または実 API router の起動。accept ごとの記録・有限台本なし。 |
| `crates/campaign/tests/preflight.rs` | 変更不要 | GET path の応答表を毎回参照し、未知 path は 404。記録・有限応答台本・単発 accept がない。 |
| `crates/campaign/tests/runner_checker.rs` | 変更不要 | TcpListener は禁止語の検査文字列。listener を作らない。 |
| `crates/campaign/tests/trial_state_cli_e2e.rs` | 変更不要 | 実 task-api/MCP/axum router が HTTP framing と routing を実施。記録・状態更新は対象 route 内（JSON body の読取後）。空接続や GET / はその handler に入らない。 |
| `crates/celeris-mcp/src/call/tests.rs` | 変更不要 | 実 task-api/MCP/axum router が HTTP framing と routing を実施。記録・状態更新は対象 route 内（JSON body の読取後）。空接続や GET / はその handler に入らない。 |
| `crates/celeris-mcp/src/lib.rs` | 対象外 | 本番 listener。fake の接続数・応答台本を持たない。 |
| `crates/celeris-mcp/src/stdio.rs` | 変更不要 | port 予約または実 API router の起動。accept ごとの記録・有限台本なし。 |
| `crates/celeris-mcp/tests/mcp_integration.rs` | 変更不要 | 実 task-api/MCP/axum router が HTTP framing と routing を実施。記録・状態更新は対象 route 内（JSON body の読取後）。空接続や GET / はその handler に入らない。 |
| `crates/celeris/src/daemon/api.rs` | 対象外 | 本番 listener。fake の接続数・応答台本を持たない。 |
| `crates/celeris/src/daemon/api_account_limits_tests.rs` | 変更不要 | 実 task-api/MCP/axum router が HTTP framing と routing を実施。記録・状態更新は対象 route 内（JSON body の読取後）。空接続や GET / はその handler に入らない。 |
| `crates/celeris/src/daemon/api_chat_tests.rs` | 変更不要 | 実 task-api/MCP/axum router が HTTP framing と routing を実施。記録・状態更新は対象 route 内（JSON body の読取後）。空接続や GET / はその handler に入らない。 |
| `crates/celeris/src/daemon/classify_tick/e2e.rs` | 修正 | 共通 reader で完全な期待 method/path の要求だけを記録・応答。旧 reader は削除。 |
| `crates/celeris/src/daemon/jev_key.rs` | 修正 | 共通 reader で完全な期待 method/path の要求だけを記録・応答。旧 reader は削除。 |
| `crates/celeris/src/local_settings_wire.rs` | 変更不要 | 実 task-api/MCP/axum router が HTTP framing と routing を実施。記録・状態更新は対象 route 内（JSON body の読取後）。空接続や GET / はその handler に入らない。 |
| `crates/celeris/src/model_discovery/tests.rs` | 修正 | 共通 reader で完全な期待 method/path の要求だけを記録・応答。旧 reader は削除。 |
| `crates/celeris/tests/campaign_acceptance_e2e.rs` | 変更不要 | 実 task-api/MCP/axum router が HTTP framing と routing を実施。記録・状態更新は対象 route 内（JSON body の読取後）。空接続や GET / はその handler に入らない。 |
| `crates/celeris/tests/campaign_lock_e2e.rs` | 変更不要 | 実 task-api/MCP/axum router が HTTP framing と routing を実施。記録・状態更新は対象 route 内（JSON body の読取後）。空接続や GET / はその handler に入らない。 |
| `crates/celeris/tests/campaign_wiring_e2e.rs` | 変更不要 | 実 task-api/MCP/axum router が HTTP framing と routing を実施。記録・状態更新は対象 route 内（JSON body の読取後）。空接続や GET / はその handler に入らない。 |
| `crates/celeris/tests/cap_probe_real_daemon.rs` | 変更不要 | port/origin の予約、接続拒否・隔離・bind 可否の検査。accept した要求を記録しない。 |
| `crates/celeris/tests/cos_chat_triage_notify.rs` | 修正 | 共通 reader で完全な期待 method/path の要求だけを記録・応答。旧 reader は削除。 |
| `crates/celeris/tests/cos_chat_triage_scenarios.rs` | 修正 | 共通 reader で完全な期待 method/path の要求だけを記録・応答。旧 reader は削除。 |
| `crates/celeris/tests/credential_wait_notify.rs` | 修正 | 共通 reader で完全な期待 method/path の要求だけを記録・応答。旧 reader は削除。 |
| `crates/celeris/tests/evidence_mode_e2e.rs` | 変更不要 | 実 task-api/MCP/axum router が HTTP framing と routing を実施。記録・状態更新は対象 route 内（JSON body の読取後）。空接続や GET / はその handler に入らない。 |
| `crates/celeris/tests/fake_http.rs` | 追加 | 共通 framing と sync/async 単発 server の迷い込み回帰試験。 |
| `crates/celeris/tests/instance_handoff.rs` | 変更不要 | port/origin の予約、接続拒否・隔離・bind 可否の検査。accept した要求を記録しない。 |
| `crates/celeris/tests/integration_api.rs` | 変更不要 | 実 task-api/MCP/axum router が HTTP framing と routing を実施。記録・状態更新は対象 route 内（JSON body の読取後）。空接続や GET / はその handler に入らない。 |
| `crates/celeris/tests/notify.rs` | 修正 | 共通 reader で完全な期待 method/path の要求だけを記録・応答。旧 reader は削除。 |
| `crates/celeris/tests/releases_api.rs` | 変更不要 | 実 task-api/MCP/axum router が HTTP framing と routing を実施。記録・状態更新は対象 route 内（JSON body の読取後）。空接続や GET / はその handler に入らない。 |
| `crates/celeris/tests/root_done_summary.rs` | 修正 | 共通 reader で完全な期待 method/path の要求だけを記録・応答。旧 reader は削除。 |
| `crates/celeris/tests/routing_eval_run.rs` | 変更不要 | port/origin の予約、接続拒否・隔離・bind 可否の検査。accept した要求を記録しない。 |
| `crates/celeris/tests/trial_context_canary_e2e.rs` | 変更不要 | 実 task-api/MCP/axum router が HTTP framing と routing を実施。記録・状態更新は対象 route 内（JSON body の読取後）。空接続や GET / はその handler に入らない。 |
| `crates/celeris/tests/variant_propagation.rs` | 変更不要 | 実 task-api/MCP/axum router が HTTP framing と routing を実施。記録・状態更新は対象 route 内（JSON body の読取後）。空接続や GET / はその handler に入らない。 |
| `crates/celeris/tests/workspace_roots.rs` | 変更不要 | 実 task-api/MCP/axum router が HTTP framing と routing を実施。記録・状態更新は対象 route 内（JSON body の読取後）。空接続や GET / はその handler に入らない。 |
| `crates/celerisctl/src/commands/cos_ops_tests.rs` | 修正 | 共通 reader で完全な期待 method/path の要求だけを記録・応答。旧 reader は削除。 |
| `crates/celerisctl/src/commands/cron_tests.rs` | 修正 | 共通 reader で完全な期待 method/path の要求だけを記録・応答。旧 reader は削除。 |
| `crates/celerisctl/src/commands/integration_tests.rs` | 修正 | 共通 reader で完全な期待 method/path の要求だけを記録・応答。旧 reader は削除。 |
| `crates/celerisctl/tests/browser_doctor.rs` | 修正 | 共通 reader で完全な期待 method/path の要求だけを記録・応答。旧 reader は削除。 |
| `crates/celerisctl/tests/cos_ops.rs` | 修正 | 共通 reader で完全な期待 method/path の要求だけを記録・応答。旧 reader は削除。 |
| `crates/jev-classifier/src/client.rs` | 修正 | 共通 reader で完全な期待 method/path の要求だけを記録・応答。旧 reader は削除。 |
| `crates/jev-classifier/tests/jev_fake_server.rs` | 修正 | 共通 reader で完全な期待 method/path の要求だけを記録・応答。旧 reader は削除。 |
| `crates/llm-proxy/src/server.rs` | 対象外 | 本番 listener。fake の接続数・応答台本を持たない。 |
| `crates/llm-proxy/tests/proxy_integration.rs` | 変更不要 | 実 task-api/MCP/axum router が HTTP framing と routing を実施。記録・状態更新は対象 route 内（JSON body の読取後）。空接続や GET / はその handler に入らない。 |
| `crates/llm-proxy/tests/proxy_routing_context.rs` | 変更不要 | 実 task-api/MCP/axum router が HTTP framing と routing を実施。記録・状態更新は対象 route 内（JSON body の読取後）。空接続や GET / はその handler に入らない。 |
| `crates/routing-eval/src/run.rs` | 修正 | 共通 reader で完全な期待 method/path の要求だけを記録・応答。旧 reader は削除。 |
| `crates/routing-eval/tests/pipeline_stub.rs` | 修正 | 共通 reader で完全な期待 method/path の要求だけを記録・応答。旧 reader は削除。 |
| `crates/task-api/src/lib.rs` | 対象外 | 本番 listener。fake の接続数・応答台本を持たない。 |
| `crates/task-api/tests/browser_h3_injection.rs` | 変更不要 | 隔離 network namespace 内の DNS/TLS fixture。host の loopback ephemeral port から到達せず、DNS は完全 packet を読んで処理。 |
| `crates/task-api/tests/browser_restore_deliver.rs` | 変更不要 | port/origin の予約、接続拒否・隔離・bind 可否の検査。accept した要求を記録しない。 |
| `crates/task-api/tests/stream.rs` | 変更不要 | 実 task-api/MCP/axum router が HTTP framing と routing を実施。記録・状態更新は対象 route 内（JSON body の読取後）。空接続や GET / はその handler に入らない。 |
| `crates/task-worker/src/aider/tests.rs` | 修正 | 共通 reader で完全な期待 method/path の要求だけを記録・応答。旧 reader は削除。 |
| `crates/task-worker/src/bin/celeris-browser-sandboxd.rs` | 対象外 | 本番 listener。fake の接続数・応答台本を持たない。 |
| `crates/task-worker/src/browser_egress/tests.rs` | 修正 | DNS は完全な A/AAAA question、echo は x のみで完了。HTTP は期待 method/path、TCP pair は既知 peer を選別。 |
| `crates/task-worker/src/browser_post_login_tests.rs` | 変更不要 | TCP→Unix の継続 bridge。接続ごとに relay し、記録数や応答台本を消費しない。 |
| `crates/task-worker/src/browser_tests.rs` | 変更不要 | 隔離 network namespace 内の DNS/TLS fixture。host の loopback ephemeral port から到達せず、DNS は完全 packet を読んで処理。 |
| `crates/task-worker/src/opencode_account/tests.rs` | 修正 | 共通 reader で完全な期待 method/path の要求だけを記録・応答。旧 reader は削除。 |
| `crates/task-worker/src/probe/tests.rs` | 修正 | 共通 reader で完全な期待 method/path の要求だけを記録・応答。旧 reader は削除。 |
| `crates/task-worker/tests/browser_cdp_sink.rs` | 変更不要 | 隔離 network namespace 内の DNS/TLS fixture。host の loopback ephemeral port から到達せず、DNS は完全 packet を読んで処理。 |
| `crates/task-worker/tests/browser_egress_relay.rs` | 変更不要 | 隔離 network namespace 内の DNS/TLS fixture。host の loopback ephemeral port から到達せず、DNS は完全 packet を読んで処理。 |
| `crates/task-worker/tests/browser_h3_wire.rs` | 変更不要 | 隔離 network namespace 内の DNS/TLS fixture。host の loopback ephemeral port から到達せず、DNS は完全 packet を読んで処理。 |
| `crates/task-worker/tests/browser_injection_attacks.rs` | 変更不要 | 隔離 network namespace 内の DNS/TLS fixture。host の loopback ephemeral port から到達せず、DNS は完全 packet を読んで処理。 |
| `crates/task-worker/tests/browser_injection_wire.rs` | 変更不要 | 隔離 network namespace 内の DNS/TLS fixture。host の loopback ephemeral port から到達せず、DNS は完全 packet を読んで処理。 |
| `crates/task-worker/tests/browser_restore_deliver.rs` | 変更不要 | port/origin の予約、接続拒否・隔離・bind 可否の検査。accept した要求を記録しない。 |
| `crates/task-worker/tests/browser_runtime_isolated.rs` | 変更不要 | port/origin の予約、接続拒否・隔離・bind 可否の検査。accept した要求を記録しない。 |
| `crates/task-worker/tests/browser_runtime_supervisor.rs` | 変更不要 | 隔離 network namespace 内の DNS/TLS fixture。host の loopback ephemeral port から到達せず、DNS は完全 packet を読んで処理。 |
| `crates/task-worker/tests/browser_sandbox_artifacts.rs` | 変更不要 | 隔離 network namespace 内の DNS/TLS fixture。host の loopback ephemeral port から到達せず、DNS は完全 packet を読んで処理。 |
| `crates/task-worker/tests/browser_shared_cdp.rs` | 修正 | TCP pair は client.local_addr と accept peer の一致を確認。namespace 内 DNS は変更不要。 |

補足: `tests/e2e/src/lib.rs` は socket2 で port を予約する helper（accept・要求記録なし）。
`browser_launcher*`、`browser_action`、`browser_doctor_tests`、`celerisctl` の browser mock、
`campaign/tests/dagbench_{ipc,process}.rs` の残る accept は一時 path の Unix socket であり host TCP ではない。
Python の browser fixture は HTTPS/browser の page handler（または namespace 内 origin）で、
HTTP 無しの接続を POST の記録に変えない。web の Node HTTP fixture も、記録配列を持つ以下 2 箇所を修正した。フレーム解析は Node が担当。

| ファイル | 対応 | 根拠 |
| --- | --- | --- |
| `web/e2e/support/fake-daemon.mjs` | 修正 | API/fixture/登録済み path 以外を記録前に 404。body 付き要求は end と complete の後に記録。正規要求の応答 abort 観測は維持。 |
| `web/e2e/support/browser-gateway.ts` | 修正 | dashboard の GET/固定 path と gateway の Origin ヘッダを識別。未知 path・body 付き GET・無関係な upgrade を記録しない。 |
| `web/e2e/support/fake-server-noise.test.ts` | 追加 | 空接続・GET /・途中 body EOF の後に正規 API/dashboard 要求を送り、記録が1件だけになることを検査。 |
| `web/api/fake-gateway.ts` | 変更不要 | fetch mock で TCP listener を作らない。 |
| `web/e2e/parity/cutover.spec.ts` | 変更不要 | net server は port の予約のみ。 |
| `web/e2e/parity/console.spec.ts`, `web/e2e/support/gateway.ts` | 変更不要 | 実 express gateway の route 検査。接続回数や fake 応答台本を持たない。 |
| `web/components/ui/overlay-browser-test.ts` | 変更不要 | Vite の page server。要求を試験の件数として記録しない。 |

Node の gateway unit fixture も `createServer` で走査した。実行中にも
`browser-live.test.mjs` が host の認証無し要求を拾い、`undefined != Bearer test-daemon-token` で
落ちることを確認した（初回 `web-node.log`）。共通 `web/server/test-support/http-fixture.mjs` で
API 外要求と body 付き GET/HEAD を記録・assertion の前に拒否し、認証の assertion は残す。

| ファイル（`web/server/`） | 対応・理由 |
| --- | --- |
| `browser-live.test.mjs` | 修正。daemon と trusted-device daemon の API 要求だけを受理。before で `GET /` →404・記録0を固定。dashboard 自体は記録・有限台本を持たない。 |
| `browser-live-frames-prod.test.mjs` | 修正。daemonRequests への追加前に API を選別。body は既存の end で完了を待つ。 |
| `browser-settings.test.mjs` | 修正。requests への追加・JSON parse 前に API を選別。body は end で待つ。 |
| `saved-credentials.test.mjs` | 修正。署名の検査・requests への追加前に API を選別。 |
| `relay.test.mjs` | 修正。seen への追加前に API を選別。body は end で待つ。閉じた server は接続拒否の試験なので変更不要。 |
| `events.test.mjs`, `console.test.mjs`, `files.test.mjs` | 修正。seen への追加と stream 開始前に API を選別。GET/HEAD は本文なしで完結。 |
| `chat.test.mjs` | 修正。API 外要求を seen への追加前に拒否。正規 upload の途中 bytes・abort 観測は中断の検査そのものなので残す（完了 request 数ではない）。 |

## 検証

- `bash scripts/dev/test-affected.sh`: exit 0。6354 passed / 18 skipped、doc tests 成功。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- `cargo test -p celeris --lib daemon::classify_tick::e2e::rate_limit_never_becomes_a_credential_wait -- --exact`: exit 0、1 passed。
- `cargo test -p celeris --test fake_http`: exit 0、framing・sync/async の3件成功。
- web `vitest run`: exit 0、98 files / 682 tests 成功。Node `node --test web/server/*.test.mjs`: exit 0、98 tests 成功。
- web `tsc -b`: exit 0。`biome check .`: exit 0（既存 styles.css の !important 警告4件）。

初回 test-affected は CoS mock を通常の `/tasks` にも使う1件だけ失敗（6353 passed）。
その試験にも期待 path を明示できるよう修正し、全体を再実行して成功した。
初回 Node 検査で見つかった host 由来の認証無し要求による非同期 assertion 失敗も、
API 外要求を除外し `GET /` を注入する回帰試験を追加して解消した。

ログは task workspace の絶対 artifacts ディレクトリ
`/local/celeris/data/workspaces/01M5M07Y3X78AQ7V0AM1347EXW/artifacts/` に保存:
`test-affected.log`, `clippy.log`, `fmt-check.log`, `rate-limit-regression.log`,
`fake-http-test.log`, `web-vitest.log`, `web-node-final.log`, `web-typecheck.log`, `web-biome.log`。

補助確認の `progress-index.sh --check` は既存の 2026-10-21/22 文書の front matter 不足で
exit 1（`progress-index.log`）。報告された文書は今回の変更対象ではなく、この新規進捗に違反はない。

## delivery-repair: verify の対象外タスクを先に除外

2026-10-23 の `df3a0e017569` prepare.log は Rust/Web の全検査・build に成功した後、
verify の smoke が 60.38 秒で `reviewing` のまま時間切れになった。
同時刻の staging `celeris-new.log` は dispatch に 21.937 秒、39.912 秒を記録し、
N-1 も 27.395 秒、34.815 秒を記録していた。fake server 試験の再発ではない。

`dispatch_ready` の対象判定は `dispatch_one` まで遅延しており、verify が扱わない
snapshot の ready task にも依存の git 判定、子 task の照会、保守 executor を適用していた。
`dispatch_run` の `eligible_ready_tasks` で候補取得直後に `is_eligible` を検査し、
容量不足時も対象外 task の待ちを記録しない。
通常モード（filter なし）の振る舞いは同じ。60 秒の合格条件は変更していない。

回帰試験 `verify_skips_ineligible_ready_tasks_before_pre_dispatch_work` は対象外の
依存をキャッシュへ入れず、保守 task に event/状態変更を起こさず、容量不足でも待ちを
記録せず、対象の smoke が Done に進むことを一時 DB 上で確認する。
元 checkout・本番サービス・staging は変更せず、release/verify/deploy は実行しない。
実際の snapshot 上での 60 秒以内の完了は次回リリース検証で確認する必要がある。

修復後の検証:

- `bash scripts/dev/test-affected.sh`: exit 0、2604 passed / 6 skipped、doc tests 成功。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- `cargo test -p task-dispatch --lib verify_skips_ineligible_ready_tasks_before_pre_dispatch_work`:
  exit 0、1 passed（0.31 秒）。
- `git diff --check`: exit 0。

ログは同じ絶対 artifacts ディレクトリの `repair-test-affected.log`、`repair-clippy.log`、
`repair-fmt.log`、`repair-regression-final.log`、`repair-diagnosis.log`。
初回の source-size 増加は既存 dispatch モジュールへの helper 配置で解消した。
回帰試験初版の smoke は汎用 Done adapter に Reviewer 判定を要求して失敗したため、
verify と同じ `true` のコマンド判定に揃えた。最終検査では両方とも解消済み。
