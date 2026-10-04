---
tasks: [01M40P9SXGPH795XC3REF1J70A]
---
# credentiald broker 試験の子 process 環境を一時 path に限る（credentiald-sandbox）

WorkUnit `credentiald-sandbox`（工程 fix、task「task-api の browser 試験を userns 不可の sandbox で
skip させ、task-api 試験を sandbox で通す」）の実行記録。

## 根因

工程 fix の統合後の検査（`cargo test --workspace`）が範囲外の
`crates/celeris-credentiald/tests/broker.rs::daemon_rejects_worker_secret_retrieval_even_with_valid_lease`
で落ちた（`broker.rs:367` 付近 `initialize_key` の `init.success` が `false`）。

子の `celeris-credentiald serve` は親（cargo test の run sandbox）の環境を継いで起動していた。
worker sandbox は `CELERIS_CREDENTIALD_DATA_DIR`（本番 data dir）など `CELERIS_*`/`XDG_*` を
プロセス環境に持たせており、試験は `HOME`/`XDG_RUNTIME_DIR` だけを一時 path に上書きしていたが
`env_clear()` をしていなかったため、子 daemon が本番 data dir 側の vault/journal を見に行き
`initialize_key` が失敗していた（journal NotFound）。

同じ問題が python の resolve client（`PATH` 以外の環境を使わない想定だったが実際は親の全環境を
継いでいた）と `bridge` 子 process にもあった。

## 修正

別 task（test-db-userns）の branch にある修正 `b466b3c1`（main 未取り込み）の
`crates/celeris-credentiald/tests/broker.rs` 差分だけを `git show b466b3c1 -- crates/celeris-credentiald/tests/broker.rs | git apply -`
で取り込んだ（同コミットが含む `agent-docs/progress/2026-10-02-test-db-userns/broker-sandbox.md` は
持ち込んでいない。適用前にこのリポジトリの `broker.rs` が `b466b3c1` の親と一致することを
`git apply --check` で確認済み）。

- `serve` 子 process: `.env_clear()` を追加し、`HOME`・`XDG_RUNTIME_DIR` だけを渡す（既存どおり）。
- python resolve client 子 process: `.env_clear()` を追加し、`PATH`（`python3` の解決に必要）だけを渡す。
- `bridge` 子 process: `.env_clear()` を追加し、`HOME`・`XDG_RUNTIME_DIR` だけを渡す（`HOME` を新規に追加）。
- `assert!(init.success)` → `assert!(init.success, "initialize_key failed: code={:?}", init.code)`。
  assert 自体は弱めていない（真偽判定は同じ）。失敗時に `code` を出すだけ。

## 検証

### `cargo test -p celeris-credentiald`

```
$ time cargo test -p celeris-credentiald
```

- exit: 0
- real: 0m34.723s（user 0m10.697s, sys 0m3.821s）
- 試験数: 44 passed, 0 failed, 0 ignored（内訳: `unittests src/lib.rs` 17、`unittests src/main.rs` 1、
  `tests/broker.rs` 12（`daemon_rejects_worker_secret_retrieval_even_with_valid_lease` を含む）、
  `tests/injection_ipc.rs` 15、`tests/policy_selector.rs` 5、`tests/prod_admission.rs` 9、doc-tests 0）
- CPU を焼く負荷（busy loop 等）は追加していない。

## 未解決事項・提案

- この run の sandbox では `cargo test --workspace` 全体は実行していない（範囲は `-p celeris-credentiald`
  に絞った、objective の指示どおり）。工程 fix の統合（integrate-fix）で `cargo test --workspace` を
  再度流して本当に解消したかを確かめる必要がある。
- 同じ根因（run sandbox が worker 用の `CELERIS_*`/`XDG_*` 環境を継ぐ）が他の crate の試験にもないか、
  今回は確認していない。もし同様の報告が出れば `env_clear()` + 必要最小限の env という同じパターンで
  直せる。
