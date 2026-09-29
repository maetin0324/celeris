# Phase browser-4: isolated runtime・trusted injection・backend routing（P4-A〜C）

---
tasks: [01M3Q49ZTST3XQ9DGF6AGNR0XG]
---

- 状態: **P4-A・P4-B・P4-C とも契約と検査（純関数・型）を実装し単独の試験が通る**。実 runtime / 実 sink / specialist backend の配線は未。本番未昇格。
- 更新: 2026-09-29
- ADR: [ADR-0084](../adr/0084-browser-phase4-isolation-injection-routing.md)

## 行ごとの判定

| 行 | 判定 | 証拠 |
|---|---|---|
| P4-A isolated runtime | 満たす（契約・検査）。実 runtime 起動は未 | `cargo test -p task-core browser_isolation` → 14 passed（別 UID/root、6 namespace、read-only、書き込み範囲、broker/host IPC 非露出、CDP の TCP 拒否、egress 負例: private/metadata/CGNAT/予約・v6 ULA/link-local/mapped/NAT64/6to4・IP literal 変種・DNS bypass・proxy 連鎖・rebinding、orphan 回収） |
| P3-A identity 復元（隔離下のみ） | 満たす | `cargo test -p task-api restore_is` → 2 passed（trusted local は `isolation_required`、attestation 付きは開封、他 project/origin/期限切れは拒否） |
| P4-B stronger injection | 満たす（契約・検査）。実 CDP sink は未 | `cargo test -p celeris-credentiald injection` → 6 passed（worker/agent の取得拒否、認証区間外拒否、TOCTOU 6 種、redirect・cross-origin iframe、DOM 再表示の型拒否と観測の破棄） |
| H3 観測停止の維持 | 維持 | `cargo test --workspace auth_section` exit 0（Phase 3 の配線は無変更） |
| P4-C backend routing | 満たす（fixture・routing・fallback・同一 task 評価）。specialist の選定は H7 待ち | `cargo test -p task-core browser_backend` → 7 passed（機密宣言は P4-A/B fixture 必須、能力を落とさない fallback、明示の拒否、混在 fixture の拒否） |

## 全体の検査

- `cargo test --workspace` exit 0（2957 passed / 0 failed）
- `cargo clippy --workspace -- -D warnings` exit 0

## 未解決

- H7（browser-specialist の backend 選定）は人の決定。既定は既存 loop（ACP / 明示 Claude）のまま。
- 実 runtime（UID 払い出し・bwrap 実行・事実採取・filtering proxy・killpg）と broker IPC の peer UID → role の割り当ての worker 配線。
- egress に celeris 内部 origin を足すかの方針（既定: 足さない）。
- `cargo clippy --all-targets` で既存の `crates/task-api/tests/browser_e2e.rs` に type_complexity（本 Phase 以前から、受け入れ条件の対象外）。
