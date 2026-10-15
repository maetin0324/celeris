---
tasks: [01M4YES0YP2F68ZG10RYANHYHB]
status: done
completed: 2026-10-15
---
# root 完了まとめ: 通し試験（WU e2e）

`crates/celeris/tests/root_done_summary.rs`（4 本）。本番の組み立て経路 `build_dispatcher` ＋ 偽 CoS（`fake` adapter を harness `claude_code` に割り当て、
`[cos] route_candidates`）＋ 127.0.0.1 の偽 webhook server。外部ネットワークに出ず、CPU 負荷試験は無い。

- `root_done_runs_cos_once_and_notifies_once`: CoS run 1・スレッド書込み 1・Discord 1、本文に CoS のまとめ・スレッドと task のリンク、旧 `task_ready` は 0。
- `child_done_emits_nothing`: 子の done は行・run・スレッド・通知・POST のどれも 0。
- `restart_does_not_duplicate`: `build_dispatcher` を作り直して同じ DB を再走査（cursor 巻き戻しも）しても行・run・通知・POST は 1 のまま。
- `cos_failure_uses_fallback_notice`: CoS run が失敗 → 決定的な短文（状態 `fallback`）が 1 通。

## 証拠

- `cargo test -p celeris --test root_done_summary` → 4 passed（3 回連続）。`cargo clippy -p celeris --tests -- -D warnings` → exit 0。

## 未解決事項

- 注入時計: dispatcher の `test_now` は task-dispatch 内部で外から差せない。試験は時間経過に依存しない（run の終了を DB 状態で待つ）。
