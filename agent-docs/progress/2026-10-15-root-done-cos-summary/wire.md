---
tasks: [01M4YES0YP2F68ZG10RYANHYHB]
status: done
completed: 2026-10-15
---
# root 完了まとめ: 配線（WU wire）

ADR: [2026-10-15-root-done-cos-summary](../../adr/2026-10-15-root-done-cos-summary.md) D1〜D7。

## やったこと

- `crates/task-dispatch/src/dispatcher/cos_chat/root_done.rs`（新規。LLM 呼び出しなし）: tick で events の
  `Transitioned{to: Done}` を cursor（`feed_cursor` `root_done_summary`、初回は現在の末尾）で読み、`is_root_done` を満たす
  task だけ `root_done_summaries` に 1 行。専用の human スレッドに事実ブロック＋指示を user message で置き、既存の
  `claim_queued` が CoS run を起こす。run が completed で最終文があれば、その文＋スレッド・task のリンクで
  `notifications`（`root_done_summary`、`UNIQUE(kind,key)`）を作る。CoS 無効・`[api] listen` 無し・run 失敗/停止/空・
  未着手のまま `unavailable_after_secs` 超過は `task_ops::root_done::fallback_body` の短文で同じ行を作る。
  取り込み・release 準備が動いている間は最大 1 時間、統合 commit/release を待つ。
- migration 0080 `root_done_summaries`（状態は new→queued→notified|fallback の一方向）。旧 `notifications.kind='task_ready'` は
  `root_done_summary` へ種別を移し、未送信のまま残る行は `ok=0` で畳む。SCHEMA_VERSION 80。
- 旧経路の削除: `scan_task_ready`、`scan` の delivery「デプロイ準備完了」(Ready) 候補、`NotificationKind::TaskReady`、
  旧試験（task_ready を期待する 2 本を「scan は done を知らせない」1 本へ）。API schema・web 生成型を再生成。
- notifier: `select_routes_batch` に `RootDoneSummary`、`triage::render` は本文そのまま、tick_loop は withdraw 再確認と
  `discard_pending` から外した。`CosChatLaunchConfig.web_base_url`（`[notify] web_base_url`）を追加。
- 試験（`dispatcher::tests::cos_chat_triage::root_done::*` 4 本、`celeris` notify 1 本）: root done で 1 スレッド・1 run・
  通知 1 通（cursor 巻き戻しでも増えない）、子の done は何も起きない、導入前の done は遡らない、CoS 無効で fallback 1 通。

## 証拠

- `cargo test -p task-dispatch --lib root_done` → 4 passed。`cargo test -p celeris --lib notify` → 12 passed。
- `bash scripts/dev/test-parallel.sh` → exit 0、nextest passed 5144 / failed 0（`userns: null`）。
- `cargo clippy --workspace -- -D warnings` → exit 0。

## 未解決事項

- 偽 webhook server を使う e2e（Discord への実 POST、再起動をまたぐ重複なし）は次の WU e2e。
- architecture-map の root_done・root_done_summaries の行は closeout 葉。
- 進捗ファイルは delivery の head の tree から `git show` で引く。delivery が無い root は「不明（進捗ファイルなし）」と出る。

## 提案

- 呼ばれていない `notify::scan`／`schedule`／`schedule_routes`（`task_failed`・`bad_news` 等）の整理と、failed／cancelled
  root の通知（D8）は別の仕事にする。

## 本番への移行

- config 欄の追加・削除は **不要**（`[notify] web_base_url`・webhook・`[cos]` を流用）。
- 本番の `[notify] web_base_url` が設定済みであること（無いと CoS のまとめは通知に出せず、fallback も作れずログに警告が出続ける）。
- migration 0080 は daemon 起動時に自動適用（`root_done_summaries` の追加と旧 `task_ready` 行の種別移行。行は失わない）。
- release 昇格後の最初の tick で cursor を現在の末尾に置く（昇格前に done になった root は通知しない）。
