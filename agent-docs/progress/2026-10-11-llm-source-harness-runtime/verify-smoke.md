---
tasks: [01M4MEEKX485VD5SWKX0P30D4Y]
status: complete
title: verify-smoke: verify の煙試験の注入を llm source・harness・route に替える
updated: 2026-10-11
---
# verify-smoke: verify の煙試験の注入を llm source・harness・route に替える

## 変更

- `crates/celeris/src/config/harness.rs` の `Config::apply_verify_smoke` は provider も reviewer の adapter・tier も足さない。代わりに ADR 2026-10-11 の形で次を入れる（同じ id の本番宣言は上書きする）:
  - `[llm_sources.smoke]` kind・api_format = `fake`、accounts = `["smoke"]`、models = `["fake"]`
  - `[harnesses.smoke]` kind = `fake`（起動コマンドは `FakeAdapter::default_command()` に固定した `[adapters.fake]`）
  - `[route_defaults.smoke]` = `smoke` / `fake` だけ
  - `[harness_selection]` を `genre = "smoke"` → `smoke` の規則 1 つだけにする。verify は smoke のタスクしか dispatch しないので、worker run も reviewer run も偽の harness しか選べない（LLM を呼ばない性質を設定の形で守る）
  - 上書きした `smoke` source・harness を指す本番の `llm_routes`・他 harness の `route_defaults` 候補・`cos_route_candidates` は外す（上書き後の形と合わず検証で落ちるため）
  - `[[roles]]`・`[[genres]]` の smoke はこれまでどおり
- `crates/celeris/tests/instance_handoff.rs`: 試験 config に偽の source・harness・route_defaults・全タスクを拾う選択規則（`FAKE_ROUTE`）を足した。normal モードの試験は「smoke を選ぶ harness が無い本番形」（genre = coding だけの規則）に直した。上書き試験は provider の代わりに smoke の source・harness の上書き、`llm_route_registry()` の成功、worker／reviewer の両 role で `select_harness` が `smoke` を返すことを確かめる。
- `crates/celeris/src/daemon/run.rs` の comment を新しい形に合わせた。

## 実行コマンドと結果

- `cargo nextest run -p celeris --test instance_handoff` → 8 passed（変更前は対象の 3 件が失敗: verify_mode_never_dispatches_and_never_touches_daemon_instances・normal_mode_does_not_inject_the_smoke_builtins・a_newer_release_takes_over_while_the_old_one_finishes_its_run）
- `cargo nextest run -p celeris --no-fail-fast` → 520 件中 518 passed・2 failed。失敗した 2 件（`browser_doctor::tests::*`）は `path must be shorter than SUN_LEN`。この run の長い TMPDIR が原因で、今回の変更とは関係ない
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0
- `cargo fmt -p celeris -- --check` → exit 0
- `bash scripts/dev/test-parallel.sh` はこの WU では走らせていない（同じ段の e2e 修復葉が並走中。統合で走る）

## 未解決

- 試験 config の `[[providers]]` 1 行は、読み込み検証（`at least one [[providers]] entry is required`・`[reviewer]` の provider 検査）がまだ要求するので残している。config-remove の WU がその検証と一緒に消すこと。
- `apply_verify_smoke` はもう `[reviewer]` の adapter・tier を設定しない（reviewer run も harness 選択を通るため）。`ReviewerConfig` の adapter・tier 欄は config-remove／hint-remove で消す。

## 本番への移行

- なし（verify モードの組み込みだけ。本番 config の書き換えは不要）。
