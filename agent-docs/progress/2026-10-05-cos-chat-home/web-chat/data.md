---
title: CoS チャット web — data 層（chat API client・stream reducer・cursor 再開）
tasks: [01M48GK6BHSW2GA1JECC7214P4]
status: done
updated: 2026-10-06
---
# web-chat / data

ADR 2026-10-05-cos-chat-home D2（gui-api.md §3.127・「チャット stream」節）と D5 に従い、`web/features/chat/data/` に CoS チャットの data 層を作った。型は `web/api/generated/types.ts` の `Chat*` を使う。API schema・`gui/`・`crates/` は変えていない。

## 作ったもの

| file | 内容 |
|---|---|
| `client.ts` | `/chat/threads`（一覧・作成・取得・PATCH・resume-queue）・messages（一覧・送信・取消）・stop・run・run events・添付（XHR の multipart upload：進捗通知・`AbortSignal` で取消・失敗は `ApiError`）・references。カードの回答用に `/decisions/{id}/answer`・`/approvals/{id}/decide`・`/tasks/{id}/answer`・`/tasks/{id}/execution/plan-gate`・`/inbox/items/{id}/answer`・`/cos/operations/{o}/override`（`OverrideBody`） |
| `reducer.ts` | ChatEvent を適用する純関数。id は十進文字列を桁数込みで比べ、適用済み以下は捨てる。message は id で全置換、text_delta は UTF-8 byte offset 一致時だけ追記（不一致は `resync` を立て、cursor を進めず、取り直すまで event を適用しない）。tool は call_id、card は kind/id（message 内の card も）で置換。queue・thread・run は全置換。終端 run の後のその run の text_delta・tool・status は捨てる。streaming 本文は message と同じ id の 1 項目（message 未着なら draft、message が来たら消える）。送信中の発言は client_message_id で 1 つ。`selectTimeline`・`selectActiveRun` |
| `stream.ts` | fetch で SSE を読む（EventSource では 410 を区別できないため）。`Last-Event-ID` と `?after=` に同じ cursor。410 → `onExpired`、401 → 停止、他は上限付き指数 backoff で cursor から張り直す。heartbeat コメントは捨てる |
| `session.ts` | 1 thread の controller。snapshot（thread + messages）→ `snapshot_event_id` から stream。resync（offset 不一致）・410 は stream を止めて snapshot から取り直す。切断は run 状態に触れない。`send` は client_message_id を固定して `sendWithRetry` |
| `send.ts` | network・timeout・5xx だけ同じ要求で再送（4xx は再送しない） |
| `use-chat-session.ts` | `useSyncExternalStore` の hook。visible・online で張り直す |
| `web/api/queries/keys.ts` | `chatKeys`（export のみ）。`queryKeys` は D5 の staleTime 表と対で試験されるので入れない（表に無い key の既定 0 で足りる）。`stale-time.ts` は変えない |

## 証拠

| 条件 | コマンド | 結果 |
|---|---|---|
| 0. client・reducer・stream hook がある | `ls web/features/chat/data/` | client.ts・reducer.ts・stream.ts・session.ts・send.ts・use-chat-session.ts と試験 3 本 |
| 1. reducer の各規則の試験 | `corepack pnpm@12.6.0 -C web exec vitest run features/chat` | 3 files・27 tests passed（chat_data_drops_events_with_id_at_or_below_applied、chat_data_text_delta_offset_mismatch_requests_resync_without_applying、chat_data_cursor_expired_410_requests_resync、chat_data_session_410_refetches_thread_and_messages_then_resumes、chat_data_streaming_draft_and_final_message_are_not_shown_twice、chat_data_resend_with_same_client_message_id_does_not_add_bubbles、chat_data_send_retries_with_same_client_message_id_and_one_bubble、chat_data_disconnect_does_not_mark_run_stopped ほか） |
| 2. typecheck・lint・test | `corepack pnpm@12.6.0 -C web typecheck` / `test` / `exec biome check features/chat/data api/queries` | `install --offline --frozen-lockfile && typecheck && lint && test` 全体で exit 0（biome 352 files・errors 0・warnings 4 は base 既存の styles.css、vitest 65 files・418 tests、node server 試験 47 pass） |
| 範囲 | 計画の scope check（web/features/chat/data/・web/api/queries/keys・本 file だけ） | stale-time.ts は base に戻した。既存試験 4 file の import 並び 1 行ずつは、計画の lint check を通すために当てた（下の未解決） |
| 境界 | `corepack pnpm@12.6.0 -C web check:boundaries` | exit 0 |

試験は fake timer（`vi.advanceTimersByTimeAsync`）と試験側から流す `ReadableStream` で決定的に進める。実時間の sleep・負荷は無い。

## 未解決

- base（c40b3669）の時点で `pnpm lint` が既存 4 試験 file（`components/content/artifact-preview.test.tsx`・`components/ui/{confirm-dialog,drawer,gallery}.test.tsx`）の import 並び（biome organizeImports）で exit 1 だった。計画の check（全体 lint）を通すため、各 file の import 1 行を biome の並び（`OVERLAY_FIXTURE_TIMEOUT` を先頭）に直した（attempt 2）。中身の変更は無い。同じ修正を並行の葉が当てても同一の差分になる。
- override の本文は gui-api.md §3.129・生成型 `OverrideBody {action, reason}` に従った（ADR D2 の表の `{idempotency_key, expected_event_id, mode, reason}` とは食い違う。実装と生成型を正とした）。

## 提案

- ADR D2 の `POST /cos/operations/{o}/override` の入力欄を、実装（`OverrideBody {action, reason}`）に合わせて付記する（close 段の ADR 付記で）。
