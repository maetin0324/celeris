---
task: 01M4JAK3MY5G1K8T66Q4RTWSS5
unit: gateway
status: done
---

# gateway: Live View frame WebSocket relay

## 実装

- `/browser/live/{task}/{run}/frames` の専用 WebSocket upgrade を追加。既存 owner session、Origin、run guard、grant check を接続時と接続中に再確認する。
- daemon の署名付き `POST /api/v1/tasks/{task}/browser/live/{run}/{session}/frames` を購読し、4-byte length-prefixed frame を WebSocket binary message に変換する。daemon の `no-store` 応答と最大 2 MiB frame を要求・確認する。
- 未送信 frame は容量 1 の slot で置換する。viewer からの WebSocket data は close し、入力・control を受け付けない。
- `/browser/runs` は grant response の `frames_available` / `live_reason` を使い、frame 経路があれば dashboard upstream 無しでも link、旧 launcher は `launcher_protocol_no_live_frames` を返す。

## 検査

- `node --test web/server/browser-live.test.mjs`: 27 passed。frame binary relay、viewer input close、owner-only、upstream 不在の link と旧 launcher reason を確認。
- `pnpm -C web exec biome check server/browser-live.js server/browser-live.test.mjs`: pass。
- `node --check web/server/browser-live.js`、`git diff --check`: pass。
- `sh "$CELERIS_WU_SCOPE_PATHS"`: pass。変更は gateway 実装・試験・本進捗だけ。
