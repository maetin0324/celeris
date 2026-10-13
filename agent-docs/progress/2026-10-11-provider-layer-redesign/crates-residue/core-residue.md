---
tasks: [01M4SRWPFY0E50F6QF0GQV877X]
status: in_progress
title: crates residue — task-core
updated: 2026-10-13
---

# crates residue: task-core

## 未解決

- `HarnessFallback::Tier` の値は `crates/task-core/src/harness.rs` の `HarnessFallback::tier()`、さらに `HarnessSpec::fallback_tier()` で `Tier` に変換される。解決・使用箇所は `crates/celeris/src/config/dispatch.rs` で `KnowledgeRuntimeConfig.fallback_tier` に渡し、`crates/task-dispatch/src/dispatcher/dispatch_run.rs` が langmem 接続先 probe の失敗時に `task.harness_id = None` と `task.worker_hint.tier = tier` を設定して通常 routing へ戻す経路（接続性判定は同 crate の `knowledge_fallback_reason`）。従って fallback の `cheap` は旧 `[[providers]]` の tier 対応ではなく、通常の tier routing 指定で、今回検出した範囲では旧 provider alias への依存はない。
- 明示 source/model に固定するなら task-core の `HarnessFallback` の形だけでは実現できず、config 転送・dispatch fallback・通常の routing を含む task-dispatch / celeris の変更が必要になる。今回の task-core WorkUnit 外のため未変更。必要なら別 WorkUnit で fallback を tier 指定から明示 route 指定に設計変更し、fallback 時に選択される source/model を routing audit と試験で固定する。

## この WorkUnit で対応

- `harness.rs` と `knowledge_run.rs` の proxy に関する説明を `<source_id>/<model_id>` と route_defaults / model routing の用語に更新。
- `routing_log_tests.rs` の現行要求例を `qwen/qwen3.8-27b` に更新。migration SQL の歴史的注釈は維持。
