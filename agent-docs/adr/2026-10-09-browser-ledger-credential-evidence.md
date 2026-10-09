# ADR 2026-10-09: browser 適合台帳に credential 用の証拠（isolation_suite・egress_negative_suite）を実測で載せる

---
tasks: [01M4F73EKB2FFGAKRY2041RZ49]
---

- 日付: 2026-10-09
- 状態: 採用（実装前）
- 関連: [ADR-0102](0102-browser-phase4-isolation-injection-routing.md)（機密能力に要る件）、
  [ADR-0106](0106-browser-phase4-conformance-dispatch.md)（実測適合記録）、
  [ADR-0109](0109-browser-p4b-injection-ipc-cdp-sink.md)（解放条件 1: 別 host UID の実 runtime で違反 0）、
  [ADR-0112](0112-browser-p4b-conformance-evidence-unlock.md)（P4-B 証拠。P4-A の証拠化は範囲外とした）、
  [ADR-0126](0126-test-daemons-without-userns.md)（`CELERIS_USERNS_TESTS=1`）、
  [ADR-0138](0138-browser-prod-admission-confidential-release.md)（本番 admission）、
  [2026-10-08-browser-prod-enablement](2026-10-08-browser-prod-enablement.md)（D1.2 `sd_browser_ledger`・D1.5 作り直し・D2 gate）、
  2026-10-09-browser-ledger-generator-isolated-runtime（branch `ops/ledger-fix`、bf757bff。範囲外に本 ADR の件を残した）

## 背景

`CredentialInjection`（= `credential_use` の起動前必須能力）と `IdentityRestore` は `required_cases` で
`isolation_suite`・`egress_negative_suite`・`injection_attack_suite`・`auth_section_observation_stop` の 4 件を要る。
ADR-0112 で後ろ 2 件は実測証拠（`evidence`）が揃わないと通らなくなったが、前 2 件は「`passed` に件名があるか」だけで
判定され、しかもどの生成器もその件名を書かない。結果として本番台帳（release 31779bc1、`ops/ledger-fix` の生成器）の
`credential_backends` は空で、login を含む task は `ledger_lacks_credential` で止まる。

前 2 件を「件名を書けば通る」形のまま生成器に書かせると ADR-0112 が塞いだ穴（根拠なしの解放）を開け直すことになる。
本 ADR は P4-A の 2 件も ADR-0112 と同じ「実試験の結果を試験名ごとに記録し、全部 passed のときだけ通す」形にする。
セキュリティ要件は緩めない（判定は今より厳しくなる。本番 admission は変えない）。

## 決定

### D1. 各 case を構成する試験（試験名で固定）

証拠の試験名は `<package>:<target>::<libtest の試験名>`（`target` は `lib` か `tests/` の結合試験名）。一覧は
`task_core::browser_backend` の定数 `P4A_ISOLATION_TESTS`・`P4A_EGRESS_NEGATIVE_TESTS` に置き、生成器は同名の
定数を持つ（ADR-0112 の `P4B_*` と同じ扱い。名前を変えるときは両方を揃える。D2.6 の試験が食い違いを落とす）。

**`isolation_suite`**（P4-A の隔離。ADR-0109 解放条件 1 の「別 host UID」を含む）

| 試験名 | 何を否定するか | 環境 |
|---|---|---|
| `task-worker:browser_runtime_isolated::probe_inside_runtime_cannot_reach_host_sockets_or_network` | runtime から broker/control socket・/run/user・host tmp・loopback・private IP・IPv6・DNS 直叩きに届かない、root/etc が書けない | userns・bwrap |
| `task-worker:browser_runtime_isolated::real_browser_in_runtime_facts_and_restore_refused_on_same_uid` | 実 browser の namespace・UID・mount・TCP listen の事実、同一 UID では restore を拒否 | userns・bwrap・browser |
| `task-worker:browser_runtime_isolated::controller_kill_leaves_no_runtime_processes` | controller が死んでも runtime process が残らない | userns・bwrap |
| `task-worker:browser_runtime_isolated::restart_reaps_recorded_runtime_and_ignores_stale_records` | 再起動で記録済み runtime を回収し古い記録を信じない | userns・bwrap |
| `task-worker:browser_runtime_supervisor::runtime_processes_do_not_survive_controller_kill_restart_or_stop` | supervisor 経由でも runtime process が残らない | userns・bwrap |
| `task-worker:browser_launcher_ptrace::launcher_chrome_denies_daemon_uid_ptrace` | launcher が起こした chrome を daemon UID から ptrace できない（別 host UID の実証） | launcher・subuid（`CELERIS_LAUNCHER_TESTS=require`） |
| `task-core:lib::browser_isolation::tests::verified_runtime_is_isolated` | 隔離判定の正例 | なし |
| `task-core:lib::browser_isolation::tests::daemon_owned_userns_is_rejected` | daemon が持ち主の userns を拒否 | なし |
| `task-core:lib::browser_isolation::tests::unknown_userns_owner_is_rejected` | 持ち主不明の userns を拒否 | なし |
| `task-core:lib::browser_isolation::tests::same_uid_and_root_are_rejected` | 同一 UID・root を拒否 | なし |
| `task-core:lib::browser_isolation::tests::every_namespace_is_required` | namespace の欠けを拒否 | なし |
| `task-core:lib::browser_isolation::tests::root_must_be_readonly_and_writes_stay_in_session` | root rw・session 外書き込みを拒否 | なし |
| `task-core:lib::browser_isolation::tests::broker_and_host_ipc_are_not_visible` | broker・host IPC が見えることを拒否 | なし |
| `task-core:lib::browser_isolation::tests::cdp_must_not_be_on_tcp_or_outside_controller_dir` | CDP の TCP 公開・controller dir 外を拒否 | なし |
| `task-core:lib::browser_isolation::tests::privileges_and_pgid_are_checked` | 特権・pgid の検査 | なし |
| `task-core:lib::browser_isolation::tests::bwrap_argv_unshares_everything_and_remounts_ro` | bwrap の argv が全 namespace を切り ro で mount し直す | なし |
| `task-core:lib::browser_isolation::tests::owner_check_alone_without_proof_is_rejected` | launcher 証明なしの持ち主検査だけでは通さない | なし |
| `task-core:lib::browser_isolation::tests::proof_does_not_override_isolation_violations` | 証明があっても隔離違反を上書きしない | なし |
| `task-core:lib::browser_isolation::tests::launched_facts_reject_namespace_shared_with_daemon` | daemon と共有する namespace を拒否 | なし |

**`egress_negative_suite`**（P4-A の egress 負例）

| 試験名 | 何を否定するか | 環境 |
|---|---|---|
| `task-worker:browser_egress_relay::fixture_reachable_only_through_per_connection_egress_proxy` | 実 runtime の netns から fixture へは接続ごとの egress proxy 経由でしか届かない | userns・bwrap・browser・unshare/ip/openssl |
| `task-worker:browser_egress_process::independent_proxy_refuses_worker_selected_private_or_proxy_destinations` | 独立起動の proxy が worker の選んだ private・proxy 宛てを拒否 | なし（egress bin） |
| `task-worker:browser_egress_process::malformed_or_oversized_policy_never_appears_in_process_output` | 壊れた・大きすぎる policy を出力に漏らさない | なし（egress bin） |
| `task-worker:browser_egress_process::missing_inherited_socket_is_refused` | 継承 socket が無ければ起動を拒否 | なし（egress bin） |
| `task-worker:lib::browser_egress::tests::real_unix_transport_rejects_proxy_dns_and_http_bypasses_before_resolution` | proxy・DNS・HTTP 迂回を名前解決前に拒否 | なし |
| `task-worker:lib::browser_egress::tests::actual_dns_transport_refuses_private_ipv6_and_rebinding` | private IPv6・rebinding を拒否 | なし |
| `task-worker:lib::browser_egress::tests::denied_origin_never_reaches_even_the_configured_dns_socket` | 拒否 origin は DNS socket にも届かない | なし |
| `task-worker:lib::browser_egress::tests::oversized_header_is_bounded_and_denied` | 大きすぎる header を拒否 | なし |
| `task-worker:lib::browser_egress::tests::malformed_dns_cannot_inject_an_address` | 壊れた DNS 応答で宛先を差し込めない | なし |
| `task-worker:lib::browser_egress::tests::dns_cname_requires_terminal_owner_and_refuses_cycles` | CNAME の循環・終端不一致を拒否 | なし |
| `task-worker:lib::browser_egress::tests::malformed_dns_length_or_transaction_is_rejected_over_tcp` | TCP DNS の長さ・transaction 不正を拒否 | なし |
| `task-worker:lib::browser_egress::tests::browser_allowed_domains_egress_denies_outside_task_and_grant` | task と grant の交差の外を拒否 | なし |
| `task-worker:lib::browser_egress::tests::egress_get_switching_origin_on_the_same_connection_never_connects` | 同一接続での origin 切替を接続しない | なし |
| `task-worker:lib::browser_egress::tests::egress_get_denials_are_recorded_like_connect` | plain HTTP の拒否も CONNECT と同じく記録 | なし |
| `task-worker:lib::browser_egress::tests::egress_test_loopback_connect_allowed_only_when_listed` | 試験用 loopback 例外は列挙したものだけ | なし |
| `task-core:lib::browser_isolation::tests::egress_rejects_private_ranges_and_rebinding` | private 範囲・rebinding を拒否 | なし |
| `task-core:lib::browser_isolation::tests::egress_rejects_ipv6_private_and_disabled` | IPv6 private・無効時の IPv6 を拒否 | なし |
| `task-core:lib::browser_isolation::tests::egress_rejects_ip_literals_and_unlisted_hosts` | IP literal・未列挙 host を拒否 | なし |
| `task-core:lib::browser_isolation::tests::egress_rejects_dns_bypass_and_proxy_chain` | DNS 迂回・proxy 連鎖を拒否 | なし |
| `task-core:lib::browser_isolation::tests::egress_test_loopback_default_off_keeps_ip_literal` | 試験用 loopback 例外は既定 off | なし |
| `task-core:lib::browser_isolation::tests::egress_test_loopback_other_private_and_variants_stay_denied` | 例外の変種・他の private は拒否のまま | なし |

選ばなかったもの: `task-api` の `browser_restore_live_session`（restore 経路。identity restore の HTTP 結線で、
P4-A の隔離・egress そのものの否定ではない）、`inner_relay_in_test_netns`・`inner_supervisor_in_test_netns`・
`helper_*`（親試験が再帰起動する補助で、単独では早期 return する）、正例だけの試験（`egress_allows_public_allowed_host` など。
ただし `verified_runtime_is_isolated` は否定試験群の基準として入れる）。`ops/ledger-fix` が足す「本番 build に loopback 例外が
無い」試験は名前が統合後に決まるので、この一覧には入れず、統合後に足す場合は本 ADR の付記と両定数を同時に変える。

### D2. 生成器 `scripts/browser-conformance.py` の新 mode

1. CLI: `--credential-evidence <LEDGER> --credential-backend <id>... --output-dir <DIR>`。
   `--credential-backend` は繰り返し可・1 件以上・既知の backend id のみ（`--p4b-backend` と同じ検査）。
   `--celeris-release`・`--celeris-sha`・`--p4b-evidence` との併用は usage error（exit 2）。入力 ledger は
   `schema == 1` かつ `source == "celeris-browser-conformance"` でなければ exit 1（ADR-0112 と同じ）。`generated_for` は保つ。
2. 実行: 最初に `cargo build --manifest-path <repo>/Cargo.toml -p task-worker --bin celeris-browser-sandboxd --bin celeris-browser-egress`
   を 1 回（失敗なら全試験 `not_run`、code `build_failed`）。続いて D1 の試験を **1 試験 1 起動**で回す:
   `cargo test --manifest-path … -p <package> (--lib | --test <target>) -- --exact <libtest 名> --nocapture --test-threads=1`。
   子の環境は呼び出し元を継ぎ、次を上書きする: `CELERIS_USERNS_TESTS=1`、`CELERIS_LAUNCHER_TESTS=require`、
   `CELERIS_ISOLATION_TESTS` は**除去**（`skip` を継いで黙って飛ばさない）。`TMPDIR` は呼び出し元の値の長さが 40 文字を超えるとき
   生成器が `/tmp` に短い dir（`mkdtemp(prefix="cel-ce-")`）を作って渡し、終わりに `chmod -R u+w` してから消す
   （Unix socket の path 上限で試験が落ちるのを避ける。大きな file は置かない）。1 試験の上限は 600 秒。
3. 試験ごとの outcome（`passed|failed|not_run`）:
   - `passed` は次の**全部**を満たすときだけ: exit 0、stdout に `running 1 test`、行 `test <libtest 名> ... ok`、
     summary `test result: ok. 1 passed; 0 failed; 0 ignored`、かつ stdout・stderr のどこにも skip 印が無い。
   - skip 印（早期 return した試験。libtest は `ok` と数えるので、これで見分ける）: 正規表現
     `SKIPPED|^SKIP:|\(not passed\)`（`userns_gate`・`test_support::skip_unless_userns_tests`・
     `browser_launcher_ptrace::missing` の既存の出力）。印があれば exit 0 でも `not_run`。
   - `failed`: exit が 0 以外で行 `test <名> ... FAILED` がある。
   - `not_run`: それ以外すべて（試験が見つからない `running 0 tests`、`ignored`、skip 印、timeout、build 失敗、行が読めない）。
4. 記録: 指定 backend の `evidence` から既存の `isolation_suite`・`egress_negative_suite` の項目を除いて今回の結果を足し
   （P4-B の項目は残す）、`passed` から 2 件を外したうえで、**全試験が `passed` のときだけ** 2 件を `passed` に足す（fail closed）。
   指定外の backend は触らない（D4 により証拠の無い件名は数えられない）。`write_ledger` で `<DIR>/conformance.json` に原子的に書く。
   試験ごとの出力は `<DIR>/credential-logs/<package>.<target>.<試験名>.log`、結果の一覧は `<DIR>/credential-evidence.json`
   （`{complete, code, reason, evidence}`）。標準出力に同じ JSON を 1 行（`record` を足す）。
5. exit: 全件 `passed` なら 0、そうでなければ 1（code は `tests_failed`（failed が 1 件以上）/ `tests_not_run`（failed 0 で not_run あり）/
   `build_failed`）、usage error は 2。
6. 試験用の差し替え: env `CELERIS_CONFORMANCE_CARGO`（cargo の実行 file。既定 `cargo`）。単体試験はこれに libtest 形の出力を返す
   偽 cargo を渡し、skip 印・`running 0 tests`・FAILED・timeout・build 失敗・全件 passed の各場合を決定的に確かめる。実 cargo・
   userns は単体試験で使わない。生成器の定数と `task_core::browser_backend` の定数の一致も単体試験で確かめる（Rust の source を
   文字列として読む）。試験の置き場は `scripts/tests/test_browser_conformance_credential.py`。

### D3. `sd_browser_ledger` の段（release.sh と `browser-ledger.sh` が同じ関数で回る）

1. 位置: 現在の「3. P4-B 証拠」の後、「4. daemon と同じ判定で検査」の前（3b）。`browser-ledger.sh` は `sd_browser_ledger` を呼ぶので
   新たな結線は要らない。
2. 実行: P4-B が完了（`p4b=true`）したときだけ走らせる（P4-B が欠けると credential は certify されないため。走らせない場合 code
   `p4b_incomplete`）。生成器は `--credential-evidence "$partial/conformance.json" --credential-backend claude-code
   --credential-backend browser-specialist --output-dir "$partial/credential"`（P4-B と同じ backend）。全体の deadline を共有する。
3. 採用: exit 0 で `$partial/credential/conformance.json` があるときだけ `$partial/conformance.json` に `mv` する。exit 1・timeout・その他では
   P4-B 後の台帳をそのまま使う（その台帳の 2 件は証拠が無いので D4 により数えられず、`credential_backends` は空。公開台帳は残る）。
   credential 段の timeout（124/137）は台帳全体を落とさない（P4-B 段までの timeout の扱いは変えない）。
4. 記録: `ledger-status.json` に `credential_evidence: {ok, code, reason}` を足す。`code` は `ok` / `p4b_incomplete` / `tests_failed` /
   `tests_not_run` / `build_failed` / `timeout` / `generator_failed`（exit 2 など）。`reason` は 1 行の説明（not_run・failed の先頭の試験名、
   無ければ空文字）。台帳を置けなかった場合（`_sd_bl_fail`）は `{ok:false, code:"not_reached", reason:""}`。
   既存の `ok`・`code`・`credential_backends` の意味は変えない（`credential_backends` は従来どおり check の結果から書く）。
   ログは `sd_log "browser-ledger: credential evidence exit $rc (code=…)"`、試験ごとのログは `browser/credential/credential-logs/` に残る。
5. 全体の上限 `SD_BROWSER_LEDGER_TIMEOUT` の既定を 1800 から 3600 秒にする（試験 40 件の起動と userns 試験の分）。
6. 試験: `scripts/selfdeploy/tests/browser_ledger_credential_*.sh`（偽 runner で exit 0 / 1 / timeout / p4b 失敗の各場合の
   `ledger-status.json` と台帳の採否、`browser-ledger.sh` からも同じ段が回ること）。

### D4. certify の条件（task-worker の `ledger_status` が使う `task_core::browser_backend`）

- `required_evidence(IsolationSuite)` は `P4A_ISOLATION_TESTS`、`required_evidence(EgressNegativeSuite)` は `P4A_EGRESS_NEGATIVE_TESTS` の
  全試験名を返す。`ConformanceResult::case_passed`（ADR-0112 D1）はそのまま使うので、2 件は**名前だけでは通らず**、要る試験が全部
  `passed` で並び、同じ件に `failed`/`not_run` が 1 件も無いときだけ通る。
- これは `IdentityRestore` にも同じく効く（判定が厳しくなるだけ）。旧形式（証拠なし）の台帳では `credential_backends` は空のまま。
- 既存の試験 fixture で 2 件を件名だけで `passed` に置いているもの（`task-core` の `full_evidence`、`task-worker` の `browser_tests.rs`・
  `browser_ledger_tests.rs`、`task-api` の `browser_h3_injection.rs`、`celeris` の `browser_doctor_tests.rs`）は証拠を足して直す。
  試験接頭辞 `browser_credential_evidence_`。
- 本番 admission（ADR-0110・0138: broker は launcher 証明つき `Attested` のみ）は変えない。台帳は routing の起動前判定を満たすだけで、
  実注入の可否は従来どおり admission が決める。

## 帰結

- credential_use の task は、release の台帳生成で P4-A・P4-B の実試験が全部通った host でだけ routing を通る。userns・launcher が
  無い host では `credential_evidence.code = tests_not_run` が残り、理由が `ledger-status.json` と `celerisctl browser doctor` で見える。
- 試験名を変えるときは `task_core::browser_backend` の定数・生成器の定数・本 ADR の表を揃える。
- 本番への台帳配置は従来どおり人が行う（release.sh か `browser-ledger.sh`。ADR-0095 付記 D-d）。
