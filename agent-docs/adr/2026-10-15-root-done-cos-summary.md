# ADR 2026-10-15: root task の完了を CoS がまとめて Discord に知らせる

---
tasks: [01M4YETK5J7Q554ATEB4DSHJP6]
---

- 日付: 2026-10-15
- 状態: 提案
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

**推奨・決定: root task ごとの専用スレッド。** migration で `chat_threads.kind` に `root_summary` を足し、`task_id`（nullable、
`kind='root_summary'` で部分 UNIQUE）を足す。スレッドは root の done 遷移でまとめを書く時に作る（`task_id` で ensure、冪等）。
専用スレッドは人が書き込める（受信箱スレッドと同じ）。

## D2 重複排除 key と再起動耐性

新しい `NotificationKind::RootDoneSummary`（`root_done_summary`）を足す。key = `<task_id>:<done 遷移の seq>`
（`transition_key` と同じ）。reopen 後の再 done は別 key でもう一度まとまる。
`notification_upsert_pending`（`INSERT OR IGNORE`）が重複排除を担い、行を作る契機は dispatcher tick が events の
`Transitioned{to: Done}`（parent なし・木の子でない・support でない）を cursor（`feed_cursor`、同じ IMMEDIATE transaction）で読むこと。
行・スレッド・run が DB に残るので、再起動後は未完の行（pending）から続きを再開する。スレッドの system message は
`chat_system_message_add_once`（key `root-done:<task_id>:<seq>`）で冪等にする。

## D3 CoS run の起動経路

既存の CoS run dispatch に乗せる。dispatcher が `RootDoneSummary` の行を見て D1 のスレッドに system message（D4 の事実）を置き、
スレッドに生きた run が無ければ `start_thread(ClaimSource::RootSummary)` で CoS run を起こす（`max_cos_runs` の枠・account 選択は
ADR-0089 のまま）。CoS には事実を渡して「何をして何が出たか」を数文にまとめさせ、最終文はそのスレッドの assistant 発言になる。
**dispatcher・store は LLM を呼ばない**（事実の組み立てと状態遷移だけが決定的な仕事）。run の状態は `chat_runs` が正本で、
task の events・lease は作らない。

## D4 決定的な事実収集

dispatcher（store）が DB と進捗ファイルだけから「事実ブロック」を作り、system message として保存する。CoS の自己申告は使わない。
項目と出所（詳細な表は進捗ファイル『事実』(f)）:

- 題名・目的（`tasks`）。
- 段階・子 task の要約（`execution_plans`・`work_units`・子 task の題名と status）。
- 統合 commit（events `integration_merged`・`phase_integrated`、`deliveries` の `head`・`target_sha`）。
- release（`deliveries.json.release`。delivery が無ければ「取り込みなし」と書く）。
- 成果物 path（events `artifact_produced`）。
- 受け入れ結果（events `review_verdict`・check の終了 event）。
- 本番への移行節の有無・未解決事項・提案節の有無（統合 commit の tree から `agent-docs/progress/*.md` を `tasks:` で引いて `git show`）。
- 未統合・配送の止まり（`delivery_skipped`、派生 inbox の `unintegrated`）。
worker の `worker_finished.outcome`（自己申告）は参考として添えるだけで、単独の根拠にしない。事実ブロックは CoS 入力にもなる。

## D5 Discord 通知とリンク

CoS run の最終文が**スレッドに書き込めた後**に、同じ行の本文を確定して pending にし、notifier が送る（スレッド書込みが成功しなければ送らない）。
本文は題名・まとめの先頭数文・リンク 2 本: スレッド `<web_base_url>/?thread=<thread_id>` と task `<web_base_url>/tasks/<id>`。
`web_base_url` が無ければ `NO_WEB_URL` と同じくエラーにする（リンク無しで成功扱いにしない）。スレッドの先頭 message（事実ブロック）に
task へのリンクを入れ、通知→スレッド→task を辿れるようにする。`select_routes_batch`（Discord に出す kind の選別）に `RootDoneSummary` を足す。

## D6 CoS 失敗時の fallback

`cos.enabled=false`・run が failed／stopped／interrupted、run 行なし、`unavailable_after_secs` 超過のとき、`CosFallback` と同じ考え方で
D4 の事実から**決定的な短文**（題名・統合 commit（無ければ「取り込みなし」）・成果物の有無・本番への移行節の有無・リンク）を作り、
同じ行の本文にして送る。arbitration は `cos_notification_routes` の UNIQUE と同じ仕組みで、まとめと fallback が両方出ないようにする。
スレッドにも fallback の system message（key `root-done-fallback:<task_id>:<seq>`）を残す。LLM は呼ばない。

## D7 TaskReady の root 完了経路を置き換えて消す

root 完了の通知はこの `RootDoneSummary` だけにする。同じ変更で消す:

- `scan_task_ready` と「delivery があれば skip」の分岐、delivery の `SecretaryReply`「デプロイ準備完了」候補、`NotificationKind::TaskReady`
  （読み取り用の旧行は migration で別 kind へ移すか、既存の `milestone_ready` と同様の方針で扱う。DB の行は失わない）。
- 呼ばれていない `notify::scan`／`schedule`／`schedule_routes` のうち、残った通知経路が使わない部分、それを固定する旧試験。
delivery 通知との関係: 本番では既に Discord に出ていないので「二重」は起きていない。新経路は delivery の有無によらず root の done で 1 回鳴り、
release があれば事実ブロックの release 項目に入る。notice の `task_done`・`delivery`・`release` は受信箱の観測用として変えない。

## D8 failed／cancelled root

範囲: 本 ADR は done だけを実装の範囲とし、failed／cancelled の扱いは決めきらない。`scan_task_failed`／`TaskFailed` も死んだ経路で、
置き換えて消すか新経路に含めるかが論点。推奨は進捗ファイルの『## 提案』節に書く。

## D9 config 欄

必要な config 欄は無い。送信先は既存の `[notify] web_base_url` と webhook、起動は `[cos]`（`enabled`・`max_cos_runs`・
`[cos.triage] unavailable_after_secs`）を流用する。fallback の待ち時間も `unavailable_after_secs` を使う。新しい欄・消す欄は無い。

## 帰結

- 既に鳴っていなかった root 完了が、CoS のまとめ付きで 1 root 1 通、スレッドと task へのリンク付きで届く。
- migration が 1 本要る（`chat_threads` の kind と task_id、`TaskReady` 旧行の扱い）。
