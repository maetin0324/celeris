# Phase browser-3: Browser Identity・live proxy・takeover（途中）

---
tasks: [01M3PBAVFAYPDWMQMDBXPTE2V8]
---

- 状態: **途中**。3 行とも task-core の純粋関数（規則と判定）と ADR だけ。task-api・worker・store・GUI への配線は無い。
- 更新: 2026-09-29

| ID | 入ったもの | 無いもの（後続） |
|---|---|---|
| P3-A Browser Identity | ADR-0083、`crates/task-core/src/browser_identity.rs`（project+origin の束縛、期限 既定 7 日 / 上限 30 日、失効で世代を上げる、削除、他 identity・他 origin の混入拒否、隔離の無い session への利用拒否）、単体試験 10 件 | 封緘の実体（AEAD）・保存・鍵の消去、API/GUI、0.38.1 の restore の負例。利用は P4-A（隔離）の後 |
| P3-B live proxy | ADR-0082、`browser_live.rs`（task/run ACL、本人の session だけ、grant 60 秒、認証区間の観測停止、再接続計画、永続 event の scrub）、単体試験 11 件 | WS の中継と接続/再接続の試験、task-api の経路、event の保存、GUI |
| P3-C takeover | ADR-0081、`browser_control.rs`（pause 収束、controller lease の排他、takeover/resume/stop、切断・競合・二重 action）、単体試験 10 件 | 制御 API、worker・task cancel への配線、状態の永続化、実際の WS 切断の試験、GUI |

## 証拠

- `cargo test -p task-core --lib browser_` → exit 0、42 passed
- `cargo test -p task-core --lib browser_identity` → exit 0、10 passed
- `cargo clippy -p task-core --all-targets -- -D warnings` → exit 0
- `cargo test --workspace`（2026-09-29、Run #5、HEAD 454a922 + docs）→ exit 0、2890 passed / 0 failed / 7 ignored（`test result: ok` 95 行）
- `cargo clippy --workspace -- -D warnings`（同）→ exit 0、warning 0

## 行ごとの判定（2026-09-29）

- P3-A: 「project/origin 単位の暗号化・期限/削除/失効、他 identity 混入拒否」のうち、期限・削除・失効・混入拒否は規則と単体試験まで。**暗号化の実体は未実装**。利用（worker の session への復元）は P4-A の隔離が要るので Phase 4 の後（ADR-0083 D3）。
- P3-B: 「task/run ACL、他 task 拒否、cookie/token 非記録」は規則と単体試験まで。**WS 接続/再接続と永続 event の保存は未実装**。
- P3-C: 「pause 収束・lease 排他・takeover/resume/stop、切断・競合・二重 action」は状態機械の単体試験まで。**API・worker・cancel への配線と実際の切断の試験は未実装**。
- 3 行とも「単独の成果・検査」をまだ満たしていない。配線は子 task に分けて提案した（P3-B → P3-C → P3-A 封緘 → GUI の順）。

## 未解決

- 0.38.1 の restore の挙動は未検証（ADR-0083 D4 の仮定）。
- browser_identity / browser_live / browser_control を呼ぶ側がまだ無い。
