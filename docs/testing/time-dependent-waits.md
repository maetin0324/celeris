# e2e・結合試験の時間依存待ち一覧（ADR-0125）

---
tasks: [01M3ZCXG7C34WZFJ46Q64XS8SZ]
---

2026-10-02 時点の `tests/e2e/tests` と `crates/*/tests` を走査した。行番号はこのタスクの変更後。検索対象は `sleep`、`wait_until`、`recv_timeout`、`deadline`、`timeout`、固定回数ループ。行番号は候補位置であり、全てが欠陥とは限らない。`sleep` の一部は実時間の仕様そのもの（SSE の 2 秒制約など）である。

| ファイル | 候補行 | 扱い |
| --- | --- | --- |
| `crates/celeris/tests/browser_startup_reap.rs` | 75, 78, 81 | 今回修正: 出来事待ち・保険を再検討 |
| `crates/celeris/tests/instance_handoff.rs` | 27, 204, 205, 210, 213, 240, 246, 265, 283, 289, 298, 305, 615, 635 | 今回修正: 出来事待ち・保険を再検討 |
| `crates/celeris/tests/notify.rs` | 788 | 既存: 出来事待ち、仕様の時間検査、または fixture 制御 |
| `crates/celeris/tests/releases_api.rs` | 134, 254, 260, 263 | 今回修正: 出来事待ち・保険を再検討 |
| `crates/celeris/tests/unpromoted_release.rs` | 211, 212, 217, 220, 320 | 既存: 出来事待ち、仕様の時間検査、または fixture 制御 |
| `crates/celeris-credentiald/tests/broker.rs` | 253, 282, 350, 356, 359, 602, 608, 611 | 今回修正: 出来事待ち・保険を再検討 |
| `crates/celeris-credentiald/tests/injection_ipc.rs` | 147, 153, 156, 211, 218, 221, 658 | 今回修正: 出来事待ち・保険を再検討 |
| `crates/celerisctl/tests/worker_run_signal.rs` | 14, 20, 121, 139, 155, 163 | 今回修正: 出来事待ち・保険を再検討 |
| `crates/llm-proxy/tests/proxy_integration.rs` | 1525, 1703, 1731, 1980, 2045, 2049（他の `from_secs(5)` 28 箇所は proxy の設定値） | 既存: 再走査待ちの下限・3 秒要件の上限を主張する仕様の時間検査。設定値は待ちではない |
| `crates/scratch-cache/tests/common/mod.rs` | 72 | 既存: 出来事待ち、仕様の時間検査、または fixture 制御 |
| `crates/scratch-cache/tests/sccache_webdav_e2e.rs` | 191, 193, 194 | 外部 WebDAV server を要する `#[ignore]` の手動試験。通常有効な対象試験から除外（下記参照） |
| `crates/scratch-cache/tests/webdav.rs` | 128, 130, 131, 186, 188, 189 | 今回修正: 出来事待ち・保険を再検討 |
| `crates/task-api/tests/browser_e2e.rs` | 102, 108, 111 | 今回修正: 出来事待ち・保険を再検討 |
| `crates/task-api/tests/browser_h3_injection.rs` | 109, 115, 118, 767, 770, 773 | 今回修正: 出来事待ち・保険を再検討 |
| `crates/task-api/tests/browser_restore_live_session.rs` | 194, 275 | 既存: 出来事待ち、仕様の時間検査、または fixture 制御 |
| `crates/task-api/tests/browser_waits.rs` | 81, 87, 90 | 今回修正: 出来事待ち・保険を再検討 |
| `crates/task-api/tests/console.rs` | 708, 728, 751, 776, 791, 809, 834 | 今回修正（第 2 走査）: SSE 到着待ちを `EVENT_WAIT` 60 秒へ |
| `crates/task-api/tests/daemon_providers_config.rs` | 46, 52, 61, 68（76 は不在の確認で残す） | 今回修正（第 2 走査）: SSE 到着待ちを `EVENT_WAIT` 60 秒へ |
| `crates/task-api/tests/common/mod.rs` | 679, 685, 696, 698, 734, 739, 742 | 既存: 出来事待ち、仕様の時間検査、または fixture 制御 |
| `crates/task-api/tests/list_and_detail.rs` | 132 | 既存: 出来事待ち、仕様の時間検査、または fixture 制御 |
| `crates/task-api/tests/standby.rs` | 71 | 既存: 出来事待ち、仕様の時間検査、または fixture 制御 |
| `crates/task-api/tests/stream.rs` | 304, 358, 410, 416, 482, 485, 491, 521 | 今回修正: 出来事待ち・保険を再検討 |
| `crates/task-dispatch/tests/unified_kill.rs` | 6, 27, 31, 165, 173, 177, 182, 187, 190, 261, 268, 271 | 今回修正: 出来事待ち・保険を再検討 |
| `crates/task-worker/tests/browser_cdp_sink.rs` | 132, 135, 138, 292, 295, 315 | 今回修正: 出来事待ち・保険を再検討 |
| `crates/task-worker/tests/browser_control_gate_wire.rs` | 103, 116, 182, 183, 185, 186, 239, 245 | 今回修正: 出来事待ち・保険を再検討 |
| `crates/task-worker/tests/browser_egress_process.rs` | 38, 43, 46, 48, 138, 139, 144, 150 | 既存: 出来事待ち、仕様の時間検査、または fixture 制御 |
| `crates/task-worker/tests/browser_egress_relay.rs` | 178, 180, 181, 213, 265, 267, 270, 296, 297, 309, 369, 376, 379, 405, 411, 412 | 今回修正: 出来事待ち・保険を再検討 |
| `crates/task-worker/tests/browser_h3_wire.rs` | 139, 142, 145, 185, 191, 194, 355, 358, 378, 452 | 今回修正: 出来事待ち・保険を再検討 |
| `crates/task-worker/tests/browser_injection_attacks.rs` | 188, 191, 194, 234, 240, 243, 472, 474, 492, 1043, 1045, 1062, 1082, 1086, 1087 | 今回修正: 出来事待ち・保険を再検討 |
| `crates/task-worker/tests/browser_injection_wire.rs` | 139, 142, 145, 178, 221, 227, 230, 378, 409, 412, 432, 496 | 今回修正: 出来事待ち・保険を再検討 |
| `crates/task-worker/tests/browser_launcher_ptrace.rs` | 143, 225, 231, 246, 333, 335, 340, 460, 463, 466 | 今回修正: 出来事待ち・保険を再検討 |
| `crates/task-worker/tests/browser_restore_deliver.rs` | 405, 409, 413, 523 | 今回修正: 出来事待ち・保険を再検討 |
| `crates/task-worker/tests/browser_runtime_isolated.rs` | 71, 72, 78, 114, 115, 116, 258, 367, 370, 374, 382, 392, 393, 461, 475, 478, 495, 498, 502, 505, 507, 508, 513, 517, 519, 551, 557, 560, 626, 658, 659, 665 | 今回修正: 出来事待ち・保険を再検討 |
| `crates/task-worker/tests/browser_runtime_supervisor.rs` | 145, 147, 148, 200, 202, 205, 232, 234, 247, 362, 374, 377, 394, 438, 440, 443, 459, 466, 469, 478, 488, 492, 574, 576, 577, 592, 658 | 今回修正: 出来事待ち・保険を再検討 |
| `crates/task-worker/tests/browser_shared_cdp.rs` | 39, 46, 47, 57, 81, 85, 87, 212, 384, 527, 587 | 今回修正: 出来事待ち・保険を再検討 |
| `crates/task-worker/tests/ssh_localhost.rs` | 121, 130, 146, 217, 245, 258 | 既存: 実 ssh の exec 上限（30 秒）と到達不能の失敗確認。localhost ssh が無い環境では skip |
| `crates/task-worker/tests/reap_finished_children.rs` | 14, 23, 26 | 今回修正: 出来事待ち・保険を再検討 |
| `tests/e2e/tests/account_pool_scenarios.rs` | 46, 52, 277, 284, 299, 428, 438, 448, 520, 526, 550 | 今回修正: 出来事待ち・保険を再検討 |
| `tests/e2e/tests/api_scenarios.rs` | 39, 45, 247, 254, 393, 422, 558, 584, 702, 725, 747, 753, 792, 833, 843, 897, 1103, 1150, 1266, 1496 | 今回修正: 出来事待ち・保険を再検討 |
| `tests/e2e/tests/cluster_scenarios.rs` | 173 | 既存: 出来事待ち、仕様の時間検査、または fixture 制御 |
| `tests/e2e/tests/codex_account_pool_scenarios.rs` | 46, 52, 139, 285, 292, 307, 435, 450, 462, 540 | 今回修正: 出来事待ち・保険を再検討 |
| `tests/e2e/tests/delegation_scenarios.rs` | 39, 45, 287, 301, 515, 733 | 今回修正: 出来事待ち・保険を再検討 |
| `tests/e2e/tests/multi_account_scenarios.rs` | 136, 224 | 今回修正: 出来事待ち・保険を再検討 |
| `tests/e2e/tests/phase7_scenarios.rs` | 166 | 既存: 出来事待ち、仕様の時間検査、または fixture 制御 |
| `tests/e2e/tests/plan_scenarios.rs` | 134, 225 | 既存: 出来事待ち、仕様の時間検査、または fixture 制御 |
| `tests/e2e/tests/provider_admin_scenarios.rs` | 38, 44, 207, 214, 359, 467, 493, 510, 535, 598, 620 | 今回修正: 出来事待ち・保険を再検討 |
| `tests/e2e/tests/scenarios.rs` | 200, 258 | 既存: 出来事待ち、仕様の時間検査、または fixture 制御 |
| `tests/e2e/tests/worker_db_read_only.rs` | 238 | 既存: 出来事待ち、仕様の時間検査、または fixture 制御 |

## 判定と修正

- `api_scenarios` の `daemon_view_shows_in_flight_runs_and_cooldowns_and_throttle_is_recorded`: 固定 6 秒の worker を release ファイル待ちへ変更。`in_flight` と cooldown が同時に観測されるまで release せず、保険超過時は worker が `done` を返さない。ProviderThrottled の event、attempts=0、同時実行中の provider 使用数の主張を保持する。
- 同じファイルの `writes_from_celerisctl_and_api_while_celeris_ticks_fast_never_hit_database_is_locked`: 180 件全ての `Done` を観測する。60 秒の進捗停止と 600 秒の総保険を併用し、daemon の生存と `database is locked` 不在を引き続き検査する。
- 4 本の `wait_api`: TCP 接続成功で listen 済みを先に確認し、続いて `/health` の 200 を確認する。子の早期終了は即失敗、各段の 120 秒は保険。
- `phase3_control_converges_rejects_competition_and_cancel_stops`: lease 更新後の固定 sleep をやめ、`GET /control` が `paused` かつ holder 消滅を返すまで待つ。失効前の競合拒否、version conflict、cancel 後の stopped を保持する。
- `instance_handoff` の旧 `draining`・新 `active`・両 instance の終了、`worker_run_signal` の PID ファイル・子終了、`reap_finished_children` の zombie 観測は、状態を主判定として 60 秒以上の保険へ変更した。reaper の固定 200 回ループも撤去した。
- `provider_admin_scenarios` の slow worker は固定 8 秒から release ファイル待ちへ変更した。新 account への dispatch を event で確認してから解放し、reload 前後の provider 状態も 120 秒の保険で観測する。
- credentiald、browser wire、task-api browser の socket 出現待ち 10 箇所を固定 100/200 回から出現条件と 60 秒保険へ変更した。`releases_api` のログ出現、`unified_kill` の孫 PID 出現・終了も同方式へ変更した。`unified_kill` の固定 40 tick も孫の消滅を主判定へ変更した。
- 他の `wait_until` / `deadline` は状態、event、プロセス終了、ファイル出現を繰り返し観測する既存の出来事待ち。残る短い期限のうち `browser_egress_process` の 8 秒は終了時間そのものを主張する試験であり、SSE の 2 秒も API 契約の検査である。固定 sleep は実時間仕様の確認または fixture のタイミング制御として残るものがある。

- `browser_cdp_sink` と `browser_h3_wire` の real browser injection は、page target 作成前に `Browser.getVersion` の `product` と `protocolVersion` を確認する CDP ready 待ちを追加した。試験専用の応答上限を 30 秒、ready の保険を 60 秒とし、失敗時は最後の CDP 応答、browser の生存、bwrap・sandbox init の `/proc` 状態、stderr を出す。receipt と origin guard の主張は維持した。

## 第 2 走査（2026-10-03、review 差し戻し後）

1 回目の走査は `sleep`・`wait_until`・`deadline` 等の語で拾ったため、SSE helper（`next_frame` / `next_named` の `within`）や `recv_timeout`・`set_read_timeout` に直接渡す短い固定期限を取りこぼした。2 回目は `Duration::from_secs(N)` / `from_millis(N)` の字面のうち 200 ms 以上 60 秒未満で `sleep` 以外のもの（95 行）を全て見て、待ちか、設定値か、仕様の時間検査か、不在の確認かを判定した。

- `crates/task-api/tests/common/mod.rs` に `EVENT_WAIT`（60 秒）を置いた。来るはずのフレーム・状態の待ちはこれを使う。
- `console.rs`: hello の 500 ms、`console.block` の 3 秒（7 箇所）を `EVENT_WAIT` へ。block の中身（progress・task・reply の streaming/done）の主張は不変。
- `daemon_providers_config.rs`: hello と `daemon` event の 2 秒（4 箇所）を `EVENT_WAIT` へ。変わらない snapshot を再送しない確認（300 ms で `None`）は不在の確認なので短いまま残す（遅れても誤って落ちない向き）。
- `stream.rs`: `TWO_SECONDS` は api.md §8.8 の「2 秒以内の配信」契約の試験（`hello_then_created_event_arrives_within_two_seconds`）だけに残し、hello の 500 ms もその契約の 2 秒に揃えた。再開・reset・503・切断・heartbeat・loopback TCP の到着待ち 25 箇所と server 停止の 5 秒 2 箇所は `EVENT_WAIT` へ。重複が無いことの 200 ms と切断後の 300 ms は不在の確認なので残す。
- `standby.rs`: reload 要求の到着 5 秒を `EVENT_WAIT` へ。
- `browser_runtime_supervisor.rs`: reap・stop 後の process 消滅待ち 5 秒・1 秒を 60 秒の保険へ（消滅を観測した時点で返る。残存 0 の主張は不変）。
- `browser_runtime_isolated.rs`: probe ファイルの出現待ち 20 秒を 60 秒へ。
- `browser_shared_cdp.rs`: loopback TCP の 1 byte 読み 5 秒を 60 秒へ。auth event が relay を越えない確認（200 ms で読めない）は不在の確認なので残す。
- `browser_egress_process.rs`: 拒否応答の読み 3 秒を 60 秒、`finish` の終了待ち 8 秒を 60 秒の保険へ。policy EOF の期限で終わる試験は `start.elapsed() < 8s` を別に主張し続ける（helper の 5 秒期限の契約）。python 台本の NoNewPrivs 観測 3 秒を 60 秒、alarm 6 秒を 90 秒へ。
- `scratch-cache/tests/common/mod.rs`: server の port 受け取り 10 秒と応答読み 10 秒を 60 秒へ。
- 残すもの: `browser_egress_relay.rs` の `fetch_marker` 3・5 秒は「届かない」ことの確認、`api_scenarios.rs` 725 の 2 秒は SSE 契約、`llm-proxy` の 900 ms 以上・4 秒未満は再走査の仕様、`unified_kill.rs` や `common/mod.rs` の `idle_timeout`・`busy_timeout` 等は設定値、`notify.rs` 1001 は値の比較。

## 第 3 走査（2026-10-03、30 秒の待ちと埋め込み台本）

前回の記録にあった「全て判定」は不正確だった。review が指摘した `browser_runtime_isolated` の CDP pipe 応答と `browser_runtime_supervisor` の SIGKILL 後の消滅は、30 秒のまま残っていた。今回、同型の待ちも含め次のように修正した。本番の timeout は変更していない。

| 対象 | 観測する出来事と保険 |
| --- | --- |
| `task-worker/tests/browser_runtime_isolated.rs` | CDP pipe の応答受信を待つ。30 → 60 秒。応答 ID と HeadlessChrome の検査を維持 |
| `task-worker/tests/browser_runtime_supervisor.rs` | controller SIGKILL 後の同一 PID/starttime の消滅、起動時 reap の完了。両方 30 → 60 秒。残存 0 と tunnel close を維持 |
| `task-worker/tests/browser_cdp_sink.rs`, `browser_h3_wire.rs`, `browser_injection_wire.rs` | feature で限定した CDP 応答上限を 30 → 60 秒。getVersion ready 判定、origin/receipt 検査、遅延応答の回帰試験を維持 |
| `task-worker/tests/browser_control_gate_wire.rs` | click 保持用 channel の解放を 60 秒待つ。timeout を捨てず失敗にし、解放されない click を成功させない |
| `task-worker/tests/browser_shared_cdp.rs` | agent の WebSocket 応答読みを 30 → 60 秒。埋め込み Python の CDP probe は 55 → 90 秒、host の probe ファイル待ちは 120 秒。各接続の 2 秒は失敗条件ではなく、総保険内で再試行する単位 |
| `task-worker/tests/browser_egress_relay.rs` | 埋め込み shell の proxy 応答読みを 8 → 60 秒。8 要求の総保険は 600 秒。`go` ファイルは固定 600 回から `until` と 300 秒の保険へ。200/403 の主張は維持 |
| `task-worker/tests/browser_injection_attacks.rs`, `browser_restore_deliver.rs`, `browser_shared_cdp.rs`、`task-api/tests/browser_restore_deliver.rs` | 本番既定の CDP 5 秒を継承していた fixture の controller に、既存の試験用 feature で 60 秒の応答保険を設定。task-api は dev-dependency のみ feature を追加 |
| `task-api/tests/browser_restore_live_session.rs` | 実 browser の CDP ready 応答を 30 → 60 秒。応答 ID と restore の拒否条件は不変 |
| `celeris/tests/releases_api.rs` | fixture server の終了を 60 秒で待ち、timeout と JoinError を無視せず検査 |

`from_secs` だけでなく埋め込み Python の `monotonic` / socket timeout と shell の `timeout` / `seq` も確認した。短い期限を残すのは、不在を確認する direct egress / fetch / SSE、実時間を要求する仕様検査、再試行間隔、または製品に渡す設定値。`ssh_localhost` の exec 上限は実 SSH コマンドの実行契約であり、状態出現待ちの helper ではない。

## SIGSTOP stutter 検証

test process のみを `SIGSTOP` 300 ms / `SIGCONT` で 2〜3 回止め、再開後に完了を待った。CPU 負荷は生成していない。`api_scenarios` の次の 4 試験は各 3 回、計 12 回とも exit 0。機械ログは run の成果物ディレクトリにある `e2e-stutter.log`。daemon と worker を止める試験ではなく、test process の観測遅延に対する検証である。

- `daemon_view_shows_in_flight_runs_and_cooldowns_and_throttle_is_recorded`: 3/3
- `writes_from_celerisctl_and_api_while_celeris_ticks_fast_never_hit_database_is_locked`: 3/3
- `phase3_control_converges_rejects_competition_and_cancel_stops`: 3/3
- `api_is_off_by_default_and_health_reports_versions_when_enabled`（`wait_api` を通る）: 3/3

API 系 fixture の `db.worker_read_only = false` は、この run の user namespace 制約で daemon 起動を可能にするための設定。DB guard の専用結合試験は別にあり、API 試験の状態機械・ロック・イベントの判定は残している。

追加の CDP 2 試験は `cargo test -p task-worker --features attack-test-hooks --test browser_cdp_sink --no-run` と同 `browser_h3_wire` が exit 0。通常の sandbox では `unshare: Operation not permitted` で browser 起動前に失敗した。権限付き実行では両方の実 browser 試験が exit 0。さらに各 test process を `SIGSTOP` 300 ms / `SIGCONT` で止める stutter を各 3 回実施し、`browser_cdp_sink` 3/3、`browser_h3_wire` 3/3 が exit 0。CPU 負荷は生成していない。

第 2 走査で直した試験も同じ方式で確かめた（test process を 100 ms ごとに `SIGSTOP` 300 ms / `SIGCONT` を 3 回。CPU 負荷なし。機械ログは成果物ディレクトリの `task-api-stutter.log`）。

- `task-api --test console`（12 件）: 3/3（初回 run3 は bash の `wait` が STOP 状態の 147 を返した採り方の誤りで、試験は未完了。`wait` を exit まで繰り返す形で 3 回やり直し 3/3）
- `task-api --test daemon_providers_config`（8 件）: 3/3
- `task-api --test stream`（8 件）: 3/3
- `task-api --test standby`（3 件）: 3/3
- `task-worker --test browser_egress_process`（6 件）: 3/3
- `scratch-cache --test webdav`（3 件）: 3/3

## 第 3 走査後の全変更対象 stutter 記録

2026-10-03、main `aed80844` を取り込んだ tree で、変更した integration test ファイルを `cargo test --workspace --no-run --message-format=json` の出力に対応付けて列挙した。各 binary を `--nocapture --test-threads=1` で実行し、test process 本人だけに **SIGSTOP 300 ms / SIGCONT 後 100 ms** を終了まで繰り返した。process group は止めない（controller-kill 試験が自分で停止した子を、外部の SIGCONT で起こさないため）。`CARGO_BIN_EXE_*` は同じ target の build 済み binary を渡した。追加修正した 6 ファイルと、clippy 指摘に対応した `releases_api` は再ビルドして各 3 回を取り直した。

通常有効な **36 ファイルを各 3 回、計 108 実行で exit 0**。libtest の最上位集計は 1 回あたり 170 passed（inner helper の入口を含む）。下表の件数は子 binary の集計を二重に加算しない。主要 4 件も `api_scenarios` 全 11 件の実行に含む。`browser_runtime_isolated` の ignored 3 件は独立した試験ではなく、通常の親試験が `--ignored --exact` で呼び出す helper である。

次の 2 点は通常有効な試験の stutter 結果と区別する。

- `browser_launcher_ptrace`: この run の sandbox は userns を作れないため、admission 表の real session を skip するのが想定どおり。以前の host protocol v1 launcher に対する `CELERIS_LAUNCHER_TESTS=require` は exit 101 だった。その後、人が host launcher を main `3527c8e3`（`PROTOCOL_VERSION=3`）へ更新し、host で必須モードを実行した。`/var/tmp/launcher-evidence-main.log` に 6 passed、EXIT 0、期待どおりの `ADMISSION[real-session]` 5 行が記録されている。sandbox での skip と host での必須モード成功を別々の証拠として扱う。
- `scratch-cache/sccache_webdav_e2e`: 外部 WebDAV server を要する手動専用の `#[ignore]` なので、通常有効な対象試験から除外する。通常実行の 0 passed / 1 ignored は想定どおりで、stutter 成功には数えない。sccache を host 設定へ移す task `01M3YD2Z58` で cache server ごと撤去される見込みであり、この task で外部 server を起動して検証しない。

機械ログ・再現台本は `/var/lib/celeris/workspaces/01M3ZCXG7C34WZFJ46Q64XS8SZ/artifacts/` の `stutter-all.py`、`test-binaries.jsonl`、`stutter3-all.log`、`stutter3-final.log`、`stutter3-final-summary.json`、各 `stutter3-<source>-<1..3>.log`。必須モードの失敗ログは `launcher-required-environment-failure.log` に保存した。最終 6 ファイルの再実行は `STUTTER_FILTER=browser_egress_relay,browser_injection_attacks,browser_restore_deliver,browser_shared_cdp,browser_launcher_ptrace STUTTER_LAUNCHER_MODE=optional python3 <上記ディレクトリ>/stutter-all.py`。`common/mod.rs` の変更は利用側の試験ファイルで検証している。

| 試験ファイル（`tests/` 以下） | 1 回の件数 | stutter 結果 |
| --- | --- | --- |
| `crates/celeris-credentiald/tests/broker.rs` | 12 | 3/3 exit 0 |
| `crates/celeris-credentiald/tests/injection_ipc.rs` | 15 | 3/3 exit 0 |
| `crates/celeris/tests/browser_startup_reap.rs` | 1 | 3/3 exit 0 |
| `crates/celeris/tests/instance_handoff.rs` | 8 | 3/3 exit 0 |
| `crates/celeris/tests/releases_api.rs` | 8 | 3/3 exit 0 |
| `crates/celerisctl/tests/worker_run_signal.rs` | 1 | 3/3 exit 0 |
| `crates/scratch-cache/tests/webdav.rs` | 3 | 3/3 exit 0 |
| `crates/task-api/tests/browser_e2e.rs` | 4 | 3/3 exit 0 |
| `crates/task-api/tests/browser_h3_injection.rs` | 3 | 3/3 exit 0 |
| `crates/task-api/tests/browser_restore_deliver.rs` | 1 | 3/3 exit 0 |
| `crates/task-api/tests/browser_restore_live_session.rs` | 1 | 3/3 exit 0 |
| `crates/task-api/tests/browser_waits.rs` | 6 | 3/3 exit 0 |
| `crates/task-api/tests/console.rs` | 12 | 3/3 exit 0 |
| `crates/task-api/tests/daemon_providers_config.rs` | 8 | 3/3 exit 0 |
| `crates/task-api/tests/standby.rs` | 3 | 3/3 exit 0 |
| `crates/task-api/tests/stream.rs` | 8 | 3/3 exit 0 |
| `crates/task-dispatch/tests/unified_kill.rs` | 4 | 3/3 exit 0 |
| `crates/task-worker/tests/browser_cdp_sink.rs` | 2 | 3/3 exit 0 |
| `crates/task-worker/tests/browser_control_gate_wire.rs` | 5 | 3/3 exit 0 |
| `crates/task-worker/tests/browser_egress_process.rs` | 6 | 3/3 exit 0 |
| `crates/task-worker/tests/browser_egress_relay.rs` | 2 | 3/3 exit 0 |
| `crates/task-worker/tests/browser_h3_wire.rs` | 2 | 3/3 exit 0 |
| `crates/task-worker/tests/browser_injection_attacks.rs` | 2 | 3/3 exit 0 |
| `crates/task-worker/tests/browser_injection_wire.rs` | 3 | 3/3 exit 0 |
| `crates/task-worker/tests/browser_launcher_ptrace.rs` | 6 | sandbox: 3/3 exit 0（real session は userns 不可で skip）。host: `require` で 6 passed、admission 表 5 行、EXIT 0 |
| `crates/task-worker/tests/browser_restore_deliver.rs` | 4 | 3/3 exit 0 |
| `crates/task-worker/tests/browser_runtime_isolated.rs` | 6 | 3/3 exit 0 |
| `crates/task-worker/tests/browser_runtime_supervisor.rs` | 4 | 3/3 exit 0 |
| `crates/task-worker/tests/browser_shared_cdp.rs` | 2 | 3/3 exit 0 |
| `crates/task-worker/tests/reap_finished_children.rs` | 1 | 3/3 exit 0 |
| `tests/e2e/tests/account_pool_scenarios.rs` | 3 | 3/3 exit 0 |
| `tests/e2e/tests/api_scenarios.rs` | 11 | 3/3 exit 0 |
| `tests/e2e/tests/codex_account_pool_scenarios.rs` | 2 | 3/3 exit 0 |
| `tests/e2e/tests/delegation_scenarios.rs` | 5 | 3/3 exit 0 |
| `tests/e2e/tests/multi_account_scenarios.rs` | 3 | 3/3 exit 0 |
| `tests/e2e/tests/provider_admin_scenarios.rs` | 3 | 3/3 exit 0 |

## 重複の扱い

`celeris-wu/01M3Z08A0T81ZQ60XVR62XJMPD/e2e-stable` の f307d63b を `cherry-pick -x` で取り込んだ。`deflake-lock` ブランチはこの worktree で参照できず、同じ全件 Done 待ちには追加で進捗停止と総保険の方式を適用した。後から同ブランチが main に入る場合は同じ箇所の重複を確認する。2026-10-03 に main（33aca5a3）を取り込んだ時点でも `deflake-lock` の `wait_for_progress` は main に無く、重複の可能性は残る（同じ試験の末尾待ちを、こちらは 60 秒停止・600 秒総保険の方式で直している）。

## 最終 tree の確認（2026-10-03）

最新 main `3527c8e3` を rebase せず `git merge main` で取り込んだ。`git merge-base --is-ancestor main HEAD` は exit 0。merge 後に次のゲートを一続きで実行し、exit 0。ログは run の成果物ディレクトリの `final-gate-merged.log` に保存した。

```sh
cargo build --workspace --bins && cargo test -p e2e --test api_scenarios && cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings
```

`api_scenarios` は 11 passed / 0 failed。禁止された CPU 焼き負荷パターンを task 差分から探す受け入れコマンドも exit 0。上記の SIGSTOP stutter 各 3 回は merge 前の試験記録であり、この最終 tree では指定ゲートを再実行した。
