# ADR-0100: Browser Phase 3 の live proxy の ACL と記録（P3-B の契約）

---
tasks: [01M3PBAVFAYPDWMQMDBXPTE2V8]
---

- 日付: 2026-09-29
- 状態: **Accepted・実装済み（2026-09-29）**。ACL・再接続計画・scrub（task-core）、live event の保存（store）、task/run 単位 grant と event の読み書き（task-api）、worker の live emitter、GUI relay（task-api 経由の認可と永続 event の WS 配信）。e2e `phase3_live_grant_is_task_scoped_scrubbed_and_reconnects_from_last_seen`
- 関連: [ADR-0078](0078-browser-execution-capability.md) D8 P3-B、[ADR-0080](0080-browser-phase2-policy-broker-approval.md) H3・D3・D6、[ADR-0099](0099-browser-phase3-control-lease.md)、人の決定 H4（task 別 ACL の proxy を先行）

## 範囲

本 ADR が決めるのは P3-B の「誰がどの task/run の live を見てよいか」と「何を永続 event に残すか」だけである。
P3-A（Browser Identity）の契約は含めない。別の ADR で決める。

## D2. live proxy の規則

実装は `crates/task-core/src/browser_live.rs`（I/O と時計を持たない。`now` は呼び出し側が渡す）。

1. 入口は task/run 単位（H4）。要求の `task_id`/`run_id` が browser session の束縛と一致しなければ `other_task`（403）。
2. 見る人は ADR-0080 D6 の本人の session だけ。auth 無効・共有 instance は `live_view_disabled`（403）、cookie 無しは `unauthenticated`（401）、他 session は `not_owner_session`（403）、Origin 不一致は `origin_mismatch`（403）。
3. 許可は期限付きの grant（既定 60 秒、ADR-0080 H2 と同じ短さ）。grant は task/run/session に束縛し、別の task の session には使えない。期限切れは `grant_expired`（410）、run の終了は `run_ended`（410）。
4. credential を注入した session（認証区間）では新規接続も既存接続も `observation_stopped`（403）で切る（ADR-0080 H3）。session の終わりまで再開しない。
5. 中継のたびに `check_connection` を呼ぶ。接続時の 1 回の判定に頼らない。
6. 再接続は client の `last_seen` の event 番号から。保持範囲の外・未来の番号・番号無しは `Reset`（現在の状態から送り直す）。
7. 永続 event に残すのは status（固定の語）・tabs（数と origin）・url（scheme/host/path、query・fragment・userinfo は捨てる）・console（長さ上限）。frame と tab の title は残さない。
8. cookie・token らしい文字列（marker 語、JWT の接頭辞、24 文字以上の token 様の連続）を含む console と URL の path 部分は固定の `[redacted]` に置き換える。部分的な除去はしない。
9. エラーは固定コードだけを返し、ページの内容を echo しない。

## 未実装（後続）

- 実装済み: WS の中継（GUI relay）、task-api の経路、event の store への保存、`last_seen` からの再接続、他 task の拒否、cookie/token の scrub。
- 残り: 認証 task の表示は引き続き D2-4 の拒否（観測停止）が安全側の動作。実 browser を相手にした長時間の切断・再接続試験。
