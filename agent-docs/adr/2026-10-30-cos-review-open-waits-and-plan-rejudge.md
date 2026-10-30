# ADR 2026-10-30-cos-review-open-waits-and-plan-rejudge: 定期見直しの入力に開いている人待ちを入れ、人に回す件に背景を付け、決定の回答後に計画の承認を判定し直す

- 状態: 決定（実装は task 01M66BSZJECRFA8KBT8T3048FY で進行中）
- 関連: ADR 2026-10-27-cron-cos-review、2026-10-28-cos-review-back-thread、2026-10-30-cos-plan-conforms-approve、2026-10-27-relayed-instructions 付記 A2

## 背景
2026-10-30 12:00Z の定期見直しの報告（統括 CoS thread 01M4B11RX2K4CMYDHGVEX0Q11D seq 4148）は 3 点で事実と違った。
回答済みの決定を「増えた人の判断」と書き、人待ちでない BenchFS task を「認可待ち」と書き、done の K4 task を「人待ち」と書いた。
入力（裏 thread seq 124）には要約の変化と停滞しか無く、開いている人待ちの一覧が無かったため、CoS は前の session の記憶で埋めた。
また、受信箱の一次対応が人に回す件の文面に背景が無く、決定の回答（11:43:38Z）が一次対応（11:43:36Z）の後に来たため plan_approval が受信箱に残った。

## 決定
- D1: 見直しの入力（`crates/task-dispatch/src/dispatcher/cos_chat/review.rs` の `review_text`）に「いま開いている人待ち」と「前回の見直しから閉じた人待ち」の 2 つの一覧を DB から決定的に入れる。各件は task id・種類（受信箱の kind）・開いた時刻を持つ。既存の受信箱の集計（cos_inbox_items）を読み、新しい集計を作らない。
- D2: cos-operator skill の見直しの手順に、報告に書く人待ち・待ちの状態は入力か報告直前の API の読みで開いていると確かめたものだけにする規則を置く。session の記憶だけで状態を書かない。前回までに報告した件を「増えた」と書かない。
- D3: 人に回す件の書式（cos-inbox-triage skill と daemon が件ごとに整形する発言）を「背景（どの依頼・どの task から来たか、なぜ今要るか）・判断・理由・人が決めること・答えると何が起きるか・選択肢・推奨」にする。task 内部の語には短い言い換えを添える。旧い書式の文言は消す。
- D4: 人が決定に答えたとき、同じ task の escalated な plan_approval の受信箱項目を案件 CoS の一次対応に再び渡す。仕組みは付記 A2 の relay 再判断（`crates/task-core/src/chat/relay_rejudge.rs`）と同じ「項目の入力 message を進めて 1 回だけ渡す」方式を再利用し、新しい監視基盤を作らない。判定は §3 (f) plan_conforms の条件のまま。決定そのものは人に残す規則を変えない。
- D5: 自動で人へ回す範囲（費用・権限・本番・human check）は変えない。
