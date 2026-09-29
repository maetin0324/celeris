# Phase browser-4: isolated runtime・trusted injection・backend routing（P4-A〜C）

---
tasks: [01M3Q49ZTST3XQ9DGF6AGNR0XG]
---

- 状態: **P4-A/B/C の受け入れは未完**。契約と純関数テストはある。P4-C の既存 loop の非機密判定を worker の起動経路に接続した。実 runtime / sink / specialist backend は未。本番未昇格。
- 更新: 2026-09-29
- ADR: [ADR-0084](../adr/0084-browser-phase4-isolation-injection-routing.md)

## 行ごとの判定

| 行 | 判定 | 証拠 |
|---|---|---|
| P4-A isolated runtime | 未達。純関数検査だけ | `cargo test -p task-core browser_isolation` → 14 passed。worker から `bwrap_argv`・`verify_isolation`・`check_egress`・`orphan_groups` は未呼出。 |
| P3-A identity 復元（隔離下のみ） | 未達。API の契約テストだけ | `cargo test -p task-api restore_is` → 2 passed。`restore_isolated` は稼働中 runtime に未結合。 |
| P4-B stronger injection | 未達。攻撃の純関数テストだけ | `cargo test -p celeris-credentiald injection` → 6 passed。実 CDP sink / IPC peer UID role は未接続。旧 plugin bridge 経路は Phase 2/3 結合テストのため残る。 |
| H3 観測停止の維持 | 維持 | `cargo test --workspace auth_section` exit 0（Phase 3 の配線は無変更） |
| P4-C backend routing | 一部接続。既存 loop の非機密起動前 route。機密判定・実 fixture 実行・specialist は未 | `cargo test -p task-core browser_backend` → 7 passed、`cargo test -p task-worker production_backend_route --lib` → 1 passed。fixture 結果は現在静的登録であり runtime の適合を証明しない。 |

## 全体の検査

- `cargo test -p task-api --test browser_e2e` exit 0（4 passed、Phase 2/3 の認証区間を維持）
- `cargo test --workspace` exit 0
- `cargo clippy --workspace -- -D warnings` exit 0

## 未解決

- H7（browser-specialist の backend 選定）は人の決定。既定は既存 loop（ACP / 明示 Claude）のまま。
- 実 runtime（UID 払い出し・bwrap 実行・事実採取・filtering proxy・killpg）と broker IPC の peer UID → role の割り当ての worker 配線。
- P4-C の登録 fixture は既存テストに対応する静的値。実 backend で fixture を実行して結果を登録する経路が必要。
- egress に celeris 内部 origin を足すかの方針（既定: 足さない）。
- `cargo clippy --all-targets` で既存の `crates/task-api/tests/browser_e2e.rs` に type_complexity（本 Phase 以前から、受け入れ条件の対象外）。
