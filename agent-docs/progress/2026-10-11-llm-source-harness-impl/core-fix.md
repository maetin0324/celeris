---
tasks: [01M4MAYXS5C7438RAB6TRZK60W]
status: done
title: core 段の統合失敗の修正（Config 初期化と config 試験の tracing 捕捉）
updated: 2026-10-11
---

# core-fix

統合 `integrate-core` の 2 点の失敗を直した。新しい機能は足していない。

## 1. workspace の test target が build できない（E0063）

`cargo test --workspace --no-run` が `crates/celerisctl/src/commands/worker_tests.rs:87` の `Config { .. }` で止まった。config-decl が足した 6 欄（`llm_sources`・`harness_selection`・`harness_declarations`・`llm_routes`・`route_defaults`・`cos_route_candidates`）が無いためだった。

- `crates/celerisctl/src/commands/worker_tests.rs`: 6 欄を既定値で足した（`Default` の無い既存 literal に合わせて明示的に書いた）。
- workspace 内で `Config` の struct literal を他に探した。`Config` は `Default` を実装していないので、他の literal は全て全欄を書いており、6 欄の不足は出なかった。`cargo build --workspace --tests --keep-going` が exit 0（`Finished`）で、全 test target（celerisctl・celeris-mcp・tests/e2e・crates/celeris を含む）が build できる。

## 2. config:: 試験が並列で不安定

`config::tests::legacy_provider_concurrency_is_optional_and_warns_including_zero_and_include` が並列実行で落ちた。単独では通った。

原因（実測で確認）:

- 警告の callsite（`crates/celeris/src/config/providers.rs:521`）の interest は、初めて hit した時点の thread の default subscriber で決まる。
- この試験の `with_default` の subscriber が登録された唯一の dispatcher のとき（tracing-core の `has_just_one`）、並列の別の試験 thread が先に同じ callsite を hit すると、その thread には subscriber が無いので interest は「無効」で固定される。
- 差し替え後の `tracing::callsite::rebuild_interest_cache()` を試したが直らなかった。callsite がまだ登録されていない（hit されていない）と、rebuild の対象に入らないためである。
- 実測: 直前に `warn!("PROBE_EXPLICIT")` を同じ scope で出すと捕捉されるが、providers の警告は捕捉されない（scope の max level は WARN のままで、原因は level ではない）。

直し方:

- `crates/celeris/src/config/tests.rs`: 試験の先頭で、常駐の sink 用 global subscriber（`with_writer(std::io::sink)`、WARN まで）を `std::sync::Once` で 1 度だけ登録する。登録された dispatcher が常に 1 つ以上あるので、callsite の interest はどの thread が先に hit しても常に決まる。試験の捕捉（`with_default`）はそのまま。
- sleep や繰り返しは入れていない。
- 同じ型の他の捕捉試験: `crates/llm-proxy/tests/proxy_integration.rs` の `set_default` 試験は変更しなかった。理由: 3 回連続で通っている（`cargo test -p llm-proxy --test proxy_integration`: 3 回とも 27 passed）。直す根拠が観測されていないものは触らない。
- 試験自体は消していない（provider の試験の削除は runtime-v2）。

## 検証

- `cargo test -p celeris --lib config::` を 8 回連続で流した: 8 回とも `169 passed; 0 failed`（修正前は同じコマンドを 5 回流して 3 回失敗。失敗は上記の試験 1 件）。
- `cargo test -p llm-proxy --test proxy_integration` を 3 回流した: 3 回とも `27 passed; 0 failed`。
- `cargo clippy --workspace -- -D warnings`: exit 0（`Finished`、warning 0、error 0）。
- `bash scripts/dev/test-parallel.sh`: 下の「test-parallel の結果」を参照。

## test-parallel の結果

- 1 回目（既定の TMPDIR `/local/celeris/data/workspaces/.../runs/01M4MCYCDXEFY65Y27VZNXYR7C/tmp`、長さ 93）: `5136 tests run: 5028 passed, 108 failed, 13 skipped`（exit 100）。失敗のほとんどは `path must be shorter than SUN_LEN`（142 件）だった。nextest が TMPDIR の接頭辞を Unix socket の path 長に算入するためで、本 task の変更とは無関係の環境要因である（2026-10-09 の memory と同じ）。
- 2 回目（`TMPDIR=/local/celeris/data/t`、短い scratch）: `5136 tests run: 5128 passed, 8 failed, 13 skipped`（exit 100）。
- 残り 8 件（すべて browser・credential broker の試験。sandbox の制約で落ちる種類として記録する。直していない）:
  - `celeris-credentiald::broker daemon_rejects_worker_secret_retrieval_even_with_valid_lease`
  - `celeris-credentiald::broker invalid_control_process_id_is_rejected`
  - `task-api::browser_e2e approved_credential_requires_conformance_and_does_not_consume_approval`
  - `task-api::browser_e2e auth_section_store_denies_takeover_until_closed`
  - `task-api::browser_e2e denied_approval_fails_the_task_without_a_lease`
  - `task-api::browser_e2e registered_credential_does_not_open_approval_for_uncertified_backend`
  - `task-api::browser_waits real_broker_registration_keeps_sentinel_out_of_db_events_artifacts_and_worker_output`
  - `task-worker browser::launcher_run::tests::action_socket_dir_must_be_private_to_this_user`
  - 失敗理由: `timed out waiting for fixture readiness`（8 件）と `credential broker is not configured`、`Io` エラー。broker・launcher の fixture が実 daemon を立てられないことによる。config・task-core・task-dispatch・celeris 本体の試験は全部通った。
  - 未確認: これらが base（`3959f8fd`）でも落ちるかは、別の作業ツリーを作って確かめていない。

## 未解決事項

- 上の残り 8 件の browser・credentiald 試験が sandbox 外で通るかは、sandbox 外の host で確かめる必要がある（人か、userns が使える環境）。
- llm-proxy の `set_default` 試験は今は通るが、同じ型の不安定さを持つ可能性がある。観測されたら同じ方法（常駐 sink の global subscriber）で直す。

## 提案

- 試験の `Config` 初期化が重複するので、将来は `Config` に `Default` を入れるか、テスト用の builder に寄せると、新しい欄を足すたびに 6 箇所直す必要がなくなる。今回は既定値の明示で揃えた（`Config` の既存方針に合わせた）。
