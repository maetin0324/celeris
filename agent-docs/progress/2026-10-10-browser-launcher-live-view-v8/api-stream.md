---
task: 01M4JAK3MY5G1K8T66Q4RTWSS5
unit: api-stream
status: done
---

# api-stream: task-api frame stream

## 変更

- `POST /api/v1/tasks/{id}/browser/live/{run}/{session}/frames` を追加。既存の署名 assertion・LiveGrant guard を再利用し、初回と 250 ms ごと／frame ごとに再確認する。認可失効時は stream を終了する。
- LiveSessions registry の session entry `live_key` が path の task/run と一致し、`LatestFrameSlot` がある場合にだけ stream を開始する。
- `application/octet-stream` で 4-byte big-endian length + opaque frame body を送り、`Cache-Control: no-store` を設定する。frame body は JSON、event、store、log に渡さない。
- grant response に `frames_available` と `live_reason` を追加。slot が無い場合は `launcher_protocol_no_live_frames` を返す。
- `browser_live_frame_` 試験で fake LiveSessionEntry の owner 成功、frame wire format、no-store、異なる task/run/session、owner 不一致、owner session でない場合、Origin 不一致、期限切れ assertion、無効 grant の拒否を確認する。
- 新しい POST route を CoS mutation registry の EXCLUDED に追加し、`agent-docs/adr/2026-10-09-cos-operations-all-mutations.md` の除外表・endpoint inventory 表の両方へ `/read` と同じ `browser_credential_attestation` 理由を記載した。API endpoint 表と grant response の記述も更新した。

## 検査

- `cargo test -p task-api browser_live_frame_`: pass（1 passed）
- `TMPDIR=/tmp cargo test -p task-api --test browser_live --test browser_e2e && cargo test -p task-api --lib`: pass（browser_live 1、browser_e2e 4、lib 88 passed / 2 ignored）
- `UPDATE_SCHEMA=1 cargo test -p task-api`: lib 88 passed / 2 ignored。生成後の API schema 照合を含め pass。grant response は既存の公開 JSON schema root に型登録されていないため、schema JSON と生成型の変更なし。
- `cargo fmt --all -- --check`、`git diff --check`: pass
- `cargo test -p task-api --test browser_live`: pass（1 passed）
- `cargo clippy -p task-api --all-targets -- -D warnings && cargo fmt --all -- --check`: pass。前回失敗した `sign_with_claims` の引数を `Claims` にまとめて解消。
- 規定の scope command: pass（許可パス内）

## 原因の確認

前回の prefix check は該当 prefix の試験が無く `0 passed` で失敗していた。今回 prefix 試験を追加して再実行し pass。
前回の lib check は frames route が mutation registry にはあったが ADR D2/D3 の除外表・endpoint inventory 表に無く失敗していた。両表・API docs の分類を揃え、CoS 網羅試験を再実行して pass。
初回の browser_e2e 再実行は既定の長い TMPDIR による fixture readiness timeout だった。`TMPDIR=/tmp` で再実行して pass。
