---
title: "snapshot-old: DaemonSnapshot の旧 unroutable・awaiting_children・awaiting_human 欄を消し task_waits へ寄せる"
tasks: [01M4NAKDCCS5XYAAJ518MVZ134]
status: done
updated: 2026-10-11
---

# snapshot-old（final review 差し戻し: ADR D4・D6 の旧 3 欄の削除）

## 変えたもの
- `crates/task-ops/src/daemon.rs`: `DaemonSnapshot` から `awaiting_human`・`awaiting_children`・`unroutable` を削除。
- `crates/task-dispatch/src/dispatcher/snapshot.rs`: 旧 3 欄の組み立てを削除（内部集合は `snapshot_task_waits` が `task_waits` に写す。ADR D4 どおり内部集合は残す）。
- `crates/task-ops/src/inbox.rs`: `attention[].unroutable` を `snap.task_waits` の `TaskWait::Unroutable` から組む。
- `web/features/ops/daemon-screen.tsx`: 「人の待ち」「担当なし」を `task_waits` の kind（`review_awaiting_human`・`unroutable`）で数える。
- 試験・fixture: `crates/task-ops/src/inbox/tests.rs`（unroutable の fixture を `task_waits` の `Unroutable` に）、`crates/task-ops/src/wait/tests.rs`、
  `crates/task-api/tests/common/mod.rs`、`crates/task-api/src/handlers/tests.rs`、`crates/task-dispatch/src/dispatcher/tests/review.rs`（子待ちの親を
  `DelegatedChildren` で確かめる）、`.../tests/cluster_tunnel.rs`、`web/e2e/admin/ops-runtime.spec.ts`、`web/components/fetch-state/fetch-frame.test.tsx`。
- schema 再生成: `docs/api/v1/api-v1.schema.json`・`web/api/generated/{schema.json,types.ts}`（旧 3 欄が消えた）。`docs/api/v1/api.md` §3.20・受信箱の節・改訂行。
- ADR 付記「葉 snapshot-old」。

## 証拠
- `grep -rn 'unroutable\|awaiting_children\|awaiting_human' crates/task-ops/src/daemon.rs` → 該当なし。schema・生成型に `DaemonSnapshot` の旧 3 欄なし。
- `UPDATE_SCHEMA=1 cargo test -p task-api --lib` → 90 passed。`node web/scripts/gen-types.mjs --check` → exit 0。
- `TMPDIR=/tmp cargo nextest run -p task-ops -p task-dispatch -p task-api` → 2154 件中 2153 passed、1 failed（`work_units::parallel_units_survive_dispatcher_restart`。単独 3 回再実行で 3/3 合格。旧欄を使わない負荷時の flaky）。
- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` → passed 5183, failed 0, ignored 14, nextest_exit 0, doctest_exit 0（TMPDIR=/tmp は既定 TMPDIR の unix socket path 長超過を避けるため）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo clippy --workspace --tests -- -D warnings` → 指摘なし。`cargo fmt --all --check` → exit 0。
- `corepack pnpm@12.6.0 -C web typecheck` → exit 0。`... -C web test` → Vitest 95 files / 690 tests passed。`... -C web lint` → error 0（既存 warning 4）。
- `corepack pnpm@12.6.0 -C web exec playwright test e2e/admin/ops-runtime.spec.ts` → 11 passed。

## 未解決
- StageReview（段の review: human）待ちを waits に出し dispatcher の `human_gate_hold` と共有する件は並行葉 stage-gate の範囲。

## 提案
- `DaemonSnapshot.task_waits` の `#[serde(default)]` は snapshot を保存しないので不要（web 側で `?? []` が要る原因）。次に触るとき必須にしてよい。
