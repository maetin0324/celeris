---
tasks: [01M4MEEKX485VD5SWKX0P30D4Y]
status: done
title: prune-policy: provider policy と仕様型を削除
updated: 2026-10-11
---
# prune-policy: provider policy と仕様型を削除

## 変更

- task-dispatch の `ProviderPolicy` / `StaticPolicy` と provider selection の state を除去し、失敗 cooldown は source/account の AccountBook 経路へ統一した。
- celeris の bootstrap/admin/config と関連 fixtures・tests を route registry と source account の設定に追従させた。
- task-worker `adapter.rs` の doc comment に残っていた旧型名を AccountBook の (source_id, account_id) cooldown の記述に直した（check の否定 grep に当たっていた）。
- celeris の account-limit 試験 3 件を新しい宣言（`[llm_sources.<id>]`）で書き直した。暗黙アカウントは account pool を持たない宣言済み source ごと（id = source id）なので、旧 `[[providers]]` fake 行 → source `none` の共有を前提にした検査と、`POST /providers` の concurrency 検査・`providers` 欄の検査（api-remove が消す API）は落とした。
  - `lib/tests.rs`: `implicit_account_limit_ignores_legacy_concurrency_and_survives_reload` → `implicit_account_limit_survives_reload_and_is_pruned_with_its_source`（reload・再起動で残り、source の宣言を消すと prune される）。
  - `daemon/api_account_limits_tests.rs`: `patch_account_limits_validate_persist_and_survive_reload_and_restart`・`implicit_source_account_limit_survives_reload` を `claude_oauth`（pool）+ `qwen`/`qwen2`（fake）の宣言に。

## 検証（attempt 1 の再試行）

- `! grep -rnwE 'ProviderSpec|LocalProviderSpec|StaticPolicy|ProviderPolicy|set_local_providers|provider_specs|local_cheap_providers' crates --include=*.rs` — exit 0（該当なし）。
- `bash scripts/dev/test-parallel.sh -E 'package(task-dispatch) or package(celeris) or package(celerisctl)'`（TMPDIR=/tmp）— exit 0、1489 passed / 0 failed / 2 ignored、doctest exit 0。
  - run の既定 TMPDIR（93 文字）では browser_doctor 2 件と celerisctl browser approve 1 件が `path must be shorter than SUN_LEN` で落ちる（環境要因。本葉と無関係。TMPDIR=/tmp で 3 件 pass）。
- `cargo clippy --workspace -- -D warnings` — exit 0。
- `git diff --check` — exit 0。

## 未解決

- `ImplicitAccountLive.providers` / API の `providers` 欄は常に空。欄の削除は api-remove。
- `provider_lives(config)`（SnapshotPublisher）は config-remove / api-remove の範囲。

## 本番への移行

なし。設定・API の互換削除は後続の config-remove / api-remove / hint-remove が担当する。
