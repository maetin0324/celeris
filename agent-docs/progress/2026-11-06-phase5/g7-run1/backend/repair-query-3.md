---
title: cos_digest の trace 試験が壊れた usage_json の run 読み失敗に頼る前提を直す（repair-query-3）
tasks: [01M6RJTQQ629D0NP953708JMS4]
status: done
updated: 2026-11-06
completed: 2026-11-06
---
# repair-query-3

完了日: 2026-11-06

## 修正前
- `cargo nextest run -p task-dispatch cos_digest_pass_trace_fixture_is_current` は exit 非 0（1 件失敗）。
- assertion: `cos_digest_trace.rs:1248` の `reads.iter().any(|i| i["run_state"] == "stopped" && i["reason"].is_null())`。
- 原因: 場面 `stopped/run-unreadable-reason-null` が `usage_json='"not usage"'` で読み失敗を作っていた。repair-query-2 以降、壊れた usage_json の run は usage=None・cost_state=unknown で読めるため、reason が null の digest が作られなくなった。

## 修正
- 製品コード（task-core `run_from_raw`）は変えていない。
- 試験側の場面だけ変更。壊す列を `resolved_config_json` に変え、`{"session_mode":"bogus"}` を入れた。未知の session_mode は今も `chat_run_get` を失敗させる。
- 「run を読めない digest は reason が null」の契約は同じ assertion で残っている。fixture の再生成は不要だった。

## 修正後（作業ツリーの HEAD で実行）
- `cargo nextest run -p task-dispatch cos_digest project_evidence_integrity`: 74 件中 74 件合格（`cos_digest_pass_trace_fixture_is_current` を含む）。
- `cargo clippy -p task-dispatch --all-targets -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- 変更範囲は `crates/task-dispatch/` と本進捗だけ。全 suite・web e2e は実行していない。

## commit
- repair: cos_digest の trace 試験を壊れた usage_json に頼らず再現する

## 未解決・提案
- なし。統合検査（`test-affected.sh`）の再実行は integrate-query で行う。
