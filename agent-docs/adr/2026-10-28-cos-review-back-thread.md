# ADR 2026-10-28: 定期見直しを統括 CoS の裏 thread で回し、変化があるときだけ統括 CoS チャットに報告する

- 日付: 2026-10-28
- 状態: 採用・実装済み（2026-10-28。付記に実装との突き合わせ）
- tasks: [01M61HW6RGHHRVAGRMQASDF9YY]
- 改める ADR: [2026-10-27-cron-cos-review](2026-10-27-cron-cos-review.md)（D2 の積み先を置き換える。D1・D3〜D5 は保つ。旧 ADR に付記した）
- 関連: [2026-10-22-cos-fixed-threads](2026-10-22-cos-fixed-threads.md)（統括・案件の固定 thread、session は thread ごと）、
  [2026-10-28-single-human-window-general-cos](2026-10-28-single-human-window-general-cos.md)（人の窓口は統括 thread 1 本）、
  [2026-10-09-cos-operations-all-mutations](2026-10-09-cos-operations-all-mutations.md)（CoS operation の表と監査）、
  [2026-10-27-project-brief](2026-10-27-project-brief.md)（案件の要約）、[ADR-0131](0131-cron-jobs.md)（cron job）

## 背景

人の依頼（2026-10-28、統括 CoS チャット seq 3936 の要旨）: 定期見直しは、やることが無い回も入力（「定期見直し」）と
CoS の返事（「変化 0」）が統括 CoS チャットの画面に毎回 2 行出て邪魔。裏の thread で監視し、何かあったときだけ統括
CoS チャットに報告してほしい。

旧 ADR の D2 では、`cos_review` の cron job が発火ごとに統括 CoS の thread（人が見るチャット）へ `mode = queue` の入力を
1 件積み、CoS run がそこで返事を書いていた。2026-10-28-single-human-window-general-cos で統括 thread は人の唯一の窓口に
なったので、定期の入力と返事が人の会話に混ざる害が大きくなった。

## 決定

### D1 thread 種別 `review`（統括 CoS の裏 thread）

- `chat_threads.kind` に `review` を足す（`ChatThreadKind::Review`、DB の値 `'review'`）。統括 CoS の定期見直し専用で、
  daemon 全体で 1 件だけ（部分 UNIQUE index `WHERE kind='review'`。`general` と同じ形）。`project_id` は NULL。
- 初回に作る: cos_review の sink（D2）が初めて積むときに、無ければ作る（`cos_review_thread_open`。`cos_thread_open` と同じ
  冪等な open）。題は『統括 CoS（定期見直し）』。migration は行を作らない（CHECK 制約の更新だけ）。
- 人の画面に出さない: thread 一覧（`GET /api/v1/chat/threads`）・ホーム・検索・未読数（未読の合計と thread ごとの数）・
  通知の振り分け先に `review` を含めない。読めるのは D6 の読み取り専用の経路と `GET /api/v1/chat/threads/{t}`・
  `…/messages` の直接指定だけ。
- 人は投稿できない: `review` thread への `POST …/messages`・`…/relays`・`…/forwards`・`…/attachments`・
  `PATCH /chat/threads/{t}`・`…/resume-queue` は、人・operator・MCP・CoS のどの主体からも 403 `chat_thread_read_only`。
  入力を積むのは D2 の sink（store の関数を直接呼ぶ）だけ、assistant の message を書くのは review thread の CoS run だけ。
  `…/stop` と `…/read` は閉じない（止める操作は人に残す。既読は害が無い）。
- 主体の写像: review thread の run の主体は `cos:<review thread id>`。decision log・監査の写像（`decision_log.rs` の
  thread 種別から主体の種類への写像）では `review` を `CosGeneral` に写す（同じ統括 CoS の役で、案件 CoS ではない）。

### D2 cos_review の積み先を review thread にする（旧 D2 の置き換え）

- `CosReviewSink`（`crates/task-dispatch/src/dispatcher/cos_chat/review.rs`）は統括 thread ではなく review thread に
  `mode = queue` の入力を 1 件積む。本文の組み方（変わった案件の要約・停滞・各 20 件まで）は旧 D2 のまま。
- 統括 thread には何も積まない。CoS の run は通常の待ち行列の claim が review thread で起こす。dispatcher も task-ops も
  LLM を呼ばない（旧 D2 と同じ）。

### D3 重複しない（旧 D3 を保つ）

- 入力の `client_message_id` は `cron-cos-review:<job id>:<予定時刻の unix 秒>` のまま。sink は post の前に review thread で
  同じ key を探し、あれば積まずに `created`。

### D4 溜めない（旧 D4 を保つ。見る thread を変える）

- review thread に待ち行列の入力が残っていれば積まずに `skipped_overlap`。人は review thread に書けないので、残るのは
  前回の見直しの入力だけになる。統括 thread の人の発言は見直しを止めない。
- CoS chat が無い・無効・使えないときは積まずに `error`（detail に理由）。

### D5 手動実行しない（旧 D5 を保つ）

- `POST /cron-jobs/{id}/run` と `run_now` は `cos_review` の job を 422 で拒否する（`check_manual_run`）。

### D3' review thread の CoS run の文脈・skill・session

- 文脈は統括 CoS と同じにする: 全案件の要約（`project_summaries(store, None)`）と、全 thread の未解決の受信箱項目
  （`cos_triage_open_items(None, …)`）。`launch.rs` の thread 種別による分岐で `Review` を `General` と同じ枝に入れる。
- skill は統括 CoS と同じ（`cos-operator`・`cos-inbox-triage`）。受信箱の一次対応（triage）の起動先は統括 thread のまま
  変えない。review thread の run は受信箱の件を読むが、判断の記録は D4' の報告で人に返す。
- session は人の統括 thread と分ける。session は thread ごとに resume する（2026-10-22-cos-fixed-threads D4）ので、thread が
  別なら session も別になる。照合 key（2026-10-28-cos-session-policy-key D1）は変えない。
- 同時実行: review thread は統括 thread と別の thread なので、両者の run は並び得る。CoS の slot（ADR-0089）の数え方は
  他の thread と同じ。

### D4' 報告の operation `chat.review_report`

- route: `POST /api/v1/cos/review-report`、operation 名 `chat.review_report`。CoS operation の表（`/cos/operations` の
  registry、`crates/task-api/src/cos/ops/surface.rs`）に ALLOWED で登録し、CoS operation としてだけ受ける。
- body: `{review_message_id, text}`。`review_message_id` は review thread の入力（cron が積んだ `role=user` の message）の id。
  `text` は 1〜4000 字。
- 受ける条件（外れたら 422 `cos_review_report_invalid`、rejected で監査に残る）:
  - 呼び出した run が review thread の run である（統括・案件・legacy の run、人・operator・MCP の token は 403）。
  - `review_message_id` が review thread の message で、`client_message_id` が `cron-cos-review:` で始まる。
- 書くもの: 統括 thread に `role=assistant` の message を 1 件。`client_message_id` は `cos-review-report:<入力 id>`。
  同じ入力への再送は既存の message を返し、2 件目を書かない（入力 1 件につき報告は最大 1 件）。
- provenance: message の `provenance`（無ければ欄を足す）に `{kind: "cos_review_report", review_thread_id, review_message_id,
  run_id}` を持つ。統括 thread の画面はこの欄から review thread の入力へたどれる（D6 の経路で開く）。
- 監査: message の書き込みと `cos_operations` の行（actor cos、thread = review thread、target `chat_thread` = 統括 thread）を
  同じ transaction で書く（2026-10-09 の原子性の規則）。
- 統括 CoS の run は起こさない: 報告は assistant の message で queue に入らない。統括 thread の未読数には数える
  （人に知らせるための報告なので）。通知（notice）は出さない。

### D5' 既定の指示（`COS_REVIEW_INSTRUCTION` の置き換え）

既定の指示を次の趣旨に改める（job の `template.objective` が空のとき）。

- 人に伝えるべきことがあるときだけ `chat.review_report` で統括 CoS チャットに 1 件報告する。伝えるべきこととは、新しい
  停滞、人の判断が要る件、案件の要約の重要な変化（状態・待ちの変化）。
- 伝えることが無ければ報告しない。review thread に 1 行（例: 「変化なし」）書いて終える。
- 報告の本文は人が数十秒で読める分量にし、案件名・task id・人がする事を書く。

skill（`cos-operator` の operation 表、`cos-inbox-triage` の定期見直しの節）も同じ規則に合わせる。

### D6 履歴は cron の画面から開く

- web の定期実行の画面（`web/features/cron/`）の `cos_review` job の行に、review thread を読み取り専用で開く導線を置く。
  開いた画面は composer・停止以外の操作を出さない（D1 で API も拒否する）。
- review thread の id は API で引ける: `GET /api/v1/cos/review-thread`（無ければ 404。作らない）。
- 統括 thread の報告の message は provenance（D4'）から同じ読み取り専用の画面へ移れる。

### D7 過去の見直しの message は残す

- 統括 thread に既に溜まった定期見直しの入力と返事は消さない・移さない。migration は `chat_messages` に触らない
  （`chat_threads` の CHECK の更新と index の追加だけ）。

### D8 試験の置き場所

| 対象 | 置き場所 | 確かめること |
| --- | --- | --- |
| 種別・open・報告の書き込み | `crates/task-core/src/chat/store_tests.rs`、migration は `crates/task-core/src/chat/tests.rs` | review thread は 1 件だけ・冪等に open、一覧・未読数に出ない、報告は入力 1 件に 1 件・再送で同じ message、監査と同じ transaction、旧 thread の message が migration で変わらない |
| 報告 operation・読み取り専用 | `crates/task-api/tests/cos_review_report.rs`（新設） | review thread の run だけ受ける（他は 403/422）、統括 thread に assistant 1 件・統括 run を起こさない、review thread への人の投稿は 403、`GET /cos/review-thread` |
| cron の積み先 | `crates/task-dispatch/src/dispatcher/tests/cron_cos_review.rs`（既存を改める） | 発火で統括 thread に入力も返事も積まれず review thread に 1 件、D3〜D5 の既存の試験（同じ周期の再 tick・再起動で増えない・`skipped_overlap`・手動実行の拒否）が通る、統括 thread の人の発言では `skipped_overlap` にならない |
| review run の文脈・session | `crates/task-dispatch/src/dispatcher/cos_chat/tests.rs` | 全案件の要約と全 thread の受信箱項目が渡る、skill が統括と同じ、session が統括 thread と別 |
| 報告する／しないの手順 | 進捗ファイルの手順（fake harness の試験で報告 0 件・1 件の両方） | 変化 0 の入力では統括 thread に何も出ない、報告すべき入力では 1 件だけ出る |
| web | `web/features/cron/queries.test.ts` と cron 画面の unit 試験 | `cos_review` の行から review thread を読み取り専用で開ける、composer が出ない |

既存の `crates/task-ops/src/cron_jobs/tests.rs`・`crates/task-api/tests/cron_jobs.rs` は変えずに通す。

## 結果

- 変化の無い回は統括 CoS チャットに何も出ない。報告すべき事がある回だけ assistant の message が 1 件出る。
- 見直しの入力・CoS の判断の過程は review thread に残り、cron の画面から読める。
- 統括 thread の人の発言が見直しを止めなくなる（D4 の判定が review thread だけを見る）。
- 名前（`review` 種別・`chat.review_report`・`POST /api/v1/cos/review-report`・`GET /api/v1/cos/review-thread`・
  `chat_thread_read_only`・`cos_review_report_invalid`）は後の葉がこの ADR に従う。

## 付記（2026-10-28、実装との突き合わせ）

- D4' の拒否の code: 呼び出した run が review thread の run でないとき（統括・案件の run）は 403 ではなく 422
  `cos_review_report_invalid`（rejected で監査に残る）にした。人・operator・MCP の token が `POST /api/v1/cos/review-report` を
  直接呼ぶと 422 `cos_review_report_invalid`、CoS run が operation を経ずに直接呼ぶと 422 `cos_audit_context_required`。
  試験は `crates/task-api/tests/cos_operations_review_report.rs`（D8 の `cos_review_report.rs` の役）。
- D1 の forwards: 宛先が review thread の `POST …/forwards` は task-core の `chat_relay_forward_tx` が 422 `validation`
  （宛先は案件 thread だけ）で拒否し、`chat_thread_read_only` ではない。書き込みは起きない。
- D4 の補足（止まった queue）: 人が review run を止めて queue が止まると、review thread の `resume-queue` は 403 なので
  誰も戻せない。sink は次の周期に、待っている入力を取り消し、新しい入力を積んで queue を再開する（`cos_review_input_post` が
  `queue_paused` を解く）。新しい入力の期間は run が取った最後の入力の予定時刻から始める。
- D6 の 3 点目（統括 thread の報告の message から review thread へ移る導線）は web に置いていない。provenance
  （`review_thread_id`・`review_message_id`）は message に入っており、cron の画面（`/cron/<job id>#review-thread`）から読める。
