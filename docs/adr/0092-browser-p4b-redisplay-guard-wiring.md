# ADR-0092: P4-B RedisplayGuard を controller の agent 観測経路へ配線する

- 状態: 採用（2026-09-30）
- 関連: ADR-0089 D4-7、ADR-0091 D1（shared CDP relay）

## 決定
1. broker は注入ごとに `RedisplayGuard` を作り、`injection.sock` の成功応答 `InjectionReply.redisplay_guard`（`{salt, digest, len}`、hex）で controller に渡す。**receipt の外**に置く（receipt は ADR-0089 どおり値・長さ・hash を持たない）。秘密そのものは渡さない。ADR-0089 D4-7 の「salt と hash だけ」に対し、**byte 長も渡す**点が差分（窓幅に必須。controller は信頼側で、長さは agent・worker に出ない）。
2. `CdpController` は session（= controller の生存期間）ごとに guard を保持する。区間を閉じた後も保持し続ける（再表示は区間後に起こる）。guard の無い／壊れた成功応答は注入失敗（`sink_failed`）として fail closed。
3. `CdpController::agent_command` の応答と `take_agent_events` の event を返す前に全 guard で検査し、一致すれば観測を丸ごと捨てて `redisplay_detected` を返す（event は落とす）。shared CDP relay（ADR-0091）は同じ関数を通るので同じ検査を受け、agent には `{"error":{"message":"redisplay_detected"}}` が返る。trusted な `controller_command` は検査しない（agent に渡らない）。
4. 検査する表現: raw、percent（`%XX`）、UTF-16LE（2 通りの整列）、base64（標準・URL-safe、8 文字以上の連なりを 4 通りの開始位置で）、JSON escape（`\uXXXX`・`\"` 等）。これらは入れ子 2 段まで復号する（例: base64 の中の UTF-16LE）。JSON 応答は直列化した text と全 key・文字列葉の両方を見る。
5. 扱わない表現（未解決）: 画面の画素（screenshot の PNG 内の描画文字。OCR はしない）、大文字小文字変換・逆順・部分文字列・独自暗号化・圧縮（gzip/deflate）された値、16 MiB を超える観測の復号。4 byte 未満の値は従来どおり常に一致扱い（全観測を捨てる）。

## 試験
- `tests/browser_injection_attacks.rs` A8: 実 broker・実 chrome 上で区間後に 8 経路（innerText・outerHTML・btoa・encodeURIComponent・二重 JSON・UTF-16 風・`DOM.getDocument`・`Accessibility.getFullAXTree`）を agent 経路で読み、全て `redisplay_detected`。screenshot は sentinel 無し。
- 負の対照: 同じ 8 経路を guard の無い `controller_command` で読むと sentinel が見える（A8 の攻撃が実在し guard が止めている）。別値の guard では検出しない（A8n）。
- `celeris-credentiald` unit: 各表現の検出・wire 往復・壊れた wire の拒否。
