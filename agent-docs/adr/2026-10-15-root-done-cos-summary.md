# ADR 2026-10-15: root task の完了を CoS がまとめて Discord に知らせる

---
tasks: [01M4YETK5J7Q554ATEB4DSHJP6]
---

- 日付: 2026-10-15
- 状態: 採用・実装済み（done の root のみ。failed／cancelled は D8 の提案）
- 2026-10-22 付記: [2026-10-22-cos-fixed-threads](2026-10-22-cos-fixed-threads.md) によりD1 の root ごとの専用スレッドは置き換え済み。要約は root の案件の CoS スレッド（無ければ統括 CoS）に入り、通知の先頭に CoS の名を付ける。
- 関連: ADR-0133（notice store）、2026-10-05-cos-chat-home、2026-10-06-cos-chat-run-dispatch、2026-10-06-cos-inbox-triage-fallback、
  2026-10-07-cos-inbox-thread-conversation、2026-10-11-breaking-changes-by-default、ADR-0079 D6/D11（木の子は通知しない）
- 事実の出所と証拠コマンド: `agent-docs/progress/2026-10-15-root-done-cos-summary.md`（『## 事実』）

## 背景

事実（コードは base `612287e5521f`、本番 DB は 2026-10-15 に `sqlite3 -readonly` で読んだ）:

- `crates/celeris/src/notify.rs` の `scan_task_ready` は root の done に `NotificationKind::TaskReady`（key = `task id:done 遷移の seq`）を作る。
  木の子・support task は除き、`delivery` があれば「検証後の delivery 通知にまとめる」として skip する。
  delivery 側は `SecretaryReply`「デプロイ準備完了」候補を作る。`scan_task_failed` は failed の root に `TaskFailed` を作る（cancelled は対象外）。
- **現行 daemon は `notify::scan`／`schedule` を呼ばない**（呼び出しは試験だけ）。tick_loop が送るのは `CosEscalation`／`CosFallback` だけで、
  他の未送信行は `discard_pending` で退けられる。したがって `scan_task_ready`・`scan_task_failed` は死んだ経路である。
- 本番 DB: `task_ready` の最終は 2026-10-02、`notification_scan_state.scanned_at` は 2026-10-03 で止まっている。最近 done の root 3 件
  （01M4M8D4AQ・01M4WSXTXW・01M4X19V0R）のうち delivery が Ready の 2 件は、Discord への notification が **0 件**
  （TaskReady も delivery 通知も出ない）。task_done／delivery／release の notice が積まれ、CoS が observe して受信箱スレッドの digest に
  1 行残るだけだった。delivery の無い 1 件は `unintegrated` が escalate されて Discord に 1 通出た。
  つまり **root が終わったことは人に能動的に届いていない**。
- 重複排除は `notifications` の `UNIQUE(kind, key)`（`INSERT OR IGNORE`）で、DB に残るので再起動しても二重にならない。
- `chat_threads.kind` は `human`／`inbox`／`legacy`。`inbox` は 1 本。task id の列は無い。
  CoS run の起動は dispatcher の `tick_cos_chat_launch`／`start_thread(ClaimSource::…)`。run は一時 Task と `CosChatContext` で動き、
  dispatcher と store は LLM を呼ばない。
- `CosFallback` の本文は notifications 行の JSON packet から決定的に組み、`web_base_url` が無ければエラーにする（`notify/triage.rs`）。

## 決定

破壊的変更の方針に従い、旧い経路は同じ変更で消す。互換分岐・新旧切替 flag は置かない。

## D1 スレッド

比較:

| 案 | 長所 | 短所 |
|---|---|---|
| root task ごとの専用スレッド | 通知→スレッド→task が 1 対 1。まとめの後の質問・追加依頼をその root の文脈で続けられる。受信箱が膨らまない | `chat_threads.kind` の CHECK と task 列の migration が要る |
| 受信箱スレッド | 既存の経路だけで済む | 一次対応の digest・人の発言と混ざり、root の文脈が流れる。1 本に全 root が積まれる |
| home | 追加なし | 人の会話と混ざる。task への辿りが弱い |

**推奨・決定: root task ごとの専用スレッド。** スレッドは通常の `human` 種（`chat_threads.kind` の CHECK は変えない。
表の作り直しと API・画面の型の変更を避ける）で、`client_thread_id = root-done:<task_id>:<seq>` により root の done 遷移ごとに
冪等に作る（題名は「完了: <task の題名>」）。task との対応は新しい表 `root_done_summaries`
（migration 0080。`task_id`・`done_seq`・`state`・`thread_id`・`input_message_id`）が持つ。人は書き込める
（通常のスレッドと同じ）。

## D2 重複排除 key と再起動耐性

新しい `NotificationKind::RootDoneSummary`（`root_done_summary`）を足す。key = `<task_id>:<done 遷移の seq>`
（`transition_key` と同じ）。reopen 後の再 done は別 key でもう一度まとまる。
dispatcher の tick が events の `Transitioned{to: Done}` を cursor（`feed_cursor` `root_done_summary`）で読み、
`task_ops::root_done::is_root_done`（parent なし・木の子でない・support でない）を満たす task だけを
`root_done_summaries` に 1 行入れる（`INSERT OR IGNORE`）。cursor が無い初回は現在の末尾から始め、過去の done を遡って
鳴らさない。行は `new → queued → notified | fallback` と一方向にだけ進み（条件付き UPDATE）、DB に残るので再起動後は
未確定の行から続きを再開する。通知の行は最終文の確定後に作り（`notification_upsert_pending` = `INSERT OR IGNORE`、
`UNIQUE(kind, key)`）、2 通目は作られない。

## D3 CoS run の起動経路

既存の CoS chat の queue に乗せる。専用スレッドに事実（D4）と「何をして何が出たかを数行にまとめて、このスレッドに書く」
指示を 1 件の user message として置く（`client_message_id` も同じ key で冪等）。通常の `claim_queued` →
`start_thread(ClaimSource::Queue)` が CoS run を起こすので、`max_cos_runs` の枠・account 選択・session・sink は
ADR-0089 のまま。run の最終文はそのスレッドの assistant 発言になる。**dispatcher・store は LLM を呼ばない**（事実の
組み立てと状態遷移だけが決定的な仕事）。run の状態は `chat_runs` が正本で、task の events・lease は作らない。
取り込み・release 準備が動いている間（delivery が reviewing / merge_queued / merging / preparing）は、統合 commit と
release が出るのを最大 1 時間待ってからスレッドを作る。

## D4 決定的な事実収集

dispatcher が `task_ops::root_done::collect`（純粋関数）で DB と進捗ファイルだけから「事実ブロック」を作り、system message として保存する。CoS の自己申告は使わない。
項目と出所（詳細な表は進捗ファイル『事実』(f)）:

- 題名・目的（`tasks`）。
- 段階・子 task の要約（`execution_plans`・`work_units`・子 task の題名と status）。
- 統合 commit（events `integration_merged`・`phase_integrated`、`deliveries` の `head`・`target_sha`）。
- release（`deliveries.json.release`。delivery が無ければ「取り込みなし」と書く）。
- 成果物 path（events `artifact_produced`）。
- 受け入れ結果（events `review_verdict`・check の終了 event）。
- 本番への移行節の有無・未解決事項・提案節の有無（delivery の head の tree から `agent-docs/progress/*.md` を `tasks:` で引いて `git show`。読めなければ「不明」）。
- 未統合・配送の止まり（`delivery_skipped`、派生 inbox の `unintegrated`）。
worker の `worker_finished.outcome`（自己申告）は参考として添えるだけで、単独の根拠にしない。事実ブロックは CoS 入力にもなる。

## D5 Discord 通知とリンク

CoS run の最終文が**スレッドに書き込めた後**に、同じ行の本文を確定して pending にし、notifier が送る（スレッド書込みが成功しなければ送らない）。
本文は題名・まとめの先頭数文・リンク 2 本: スレッド `<web_base_url>/?thread=<thread_id>` と task `<web_base_url>/tasks/<id>`。
`web_base_url` が無ければ `NO_WEB_URL` と同じくエラーにする（リンク無しで成功扱いにしない）。スレッドの先頭 message（事実ブロック）に
task へのリンクを入れ、通知→スレッド→task を辿れるようにする。`select_routes_batch`（Discord に出す kind の選別）に `RootDoneSummary` を足す。
notifier は本文をそのまま送る（`notify::triage::render`）。CoS の待ち（`cos_inbox_items`）の再確認 `withdraw_if_resolved` は
この kind には無い（再確認する元の待ちが無い）。旧 `legacy` の行の畳み込み（`discard_pending`）の対象からも外す。

## D6 CoS 失敗時の fallback

`cos.enabled=false`・run が failed／stopped／interrupted、run 行なし、`unavailable_after_secs` 超過のとき、`CosFallback` と同じ考え方で
D4 の事実から**決定的な短文**（題名・統合 commit（無ければ「取り込みなし」）・成果物の有無・本番への移行節の有無・リンク）を作り、
同じ行の本文にして送る。まとめと fallback が両方出ないことは、`root_done_summaries` の状態が一方向（`queued` から
`notified` か `fallback` のどちらか 1 つへ）であることと、通知の `UNIQUE(kind, key)` が保証する。CoS が無効・`[api] listen` が
無い（`unavailable_reason`）ときはスレッドを作らず、スレッドへのリンクの無い短文にする。未着手のまま待った場合は、
スレッドの入力 message を取り下げてから fallback にする（後から run が起きない）。
スレッドにも fallback の system message（key `root-done-fallback:<task_id>:<seq>`）を残す。LLM は呼ばない。

## D7 TaskReady の root 完了経路を置き換えて消す

root 完了の通知はこの `RootDoneSummary` だけにする。同じ変更で消す:

- `scan_task_ready` と「delivery があれば skip」の分岐、`scan` が作っていた delivery の `SecretaryReply`「デプロイ準備完了」
  候補（Ready の分岐。CoS の確認が要る Blocked `[needs-human]` の候補は別の通知なので残す）、`NotificationKind::TaskReady`、
  それを固定していた旧試験。`notifications` の旧 `task_ready` 行は migration 0080 で `root_done_summary` へ種別を移す
  （行は失わない。未送信のまま残っていた行は `ok=0` で畳む）。
- 範囲外として残すもの: 呼ばれていない `notify::scan`／`schedule`／`schedule_routes` のうち他の kind（`task_failed`・
  `bad_news` など）が使う部分。これらの整理は D8 と合わせた別の仕事（進捗ファイルの『提案』）。
delivery 通知との関係: 本番では既に Discord に出ていないので「二重」は起きていない。新経路は delivery の有無によらず root の done で 1 回鳴り、
release があれば事実ブロックの release 項目に入る。notice の `task_done`・`delivery`・`release` は受信箱の観測用として変えない。

## D8 failed／cancelled root

この変更の範囲は done だけ。`scan_task_failed`／`TaskFailed` も daemon から呼ばれない死んだ経路として残してある（`notify::scan` の他 kind と合わせた整理を別の仕事にする）。提案（未決定・別 task で実装）:

- failed は同じ仕組み（専用スレッド・事実ブロック・CoS・fallback）に含める。事実ブロックに失敗の分類と理由（`classify_task_failure`）と main 取り込み済みの有無を足し、`RootDoneSummary` を outcome 欄を持つ `RootSummary` に改める。`TaskFailed` と `scan_task_failed` は同じ変更で消す（通知の空白期間を作らない）。
- cancelled は人が自分で止めたものなので通知しない（スレッドも作らない）。

## D9 config 欄

必要な config 欄は無い。送信先は既存の `[notify] web_base_url` と webhook、起動は `[cos]`（`enabled`・`max_cos_runs`・
`[cos.triage] unavailable_after_secs`）を流用する。fallback の待ち時間も `unavailable_after_secs` を使う。新しい欄・消す欄は無い。

## 帰結

- 既に鳴っていなかった root 完了が、CoS のまとめ付きで 1 root 1 通、スレッドと task へのリンク付きで届く。
- migration が 1 本（0080: `root_done_summaries` と旧 `task_ready` 行の移行）。`chat_threads` は変えない。
