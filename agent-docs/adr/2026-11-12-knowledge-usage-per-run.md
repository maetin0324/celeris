# KB の読み取りを run ごとに入口で数え、curation は更新・統合を先に選ぶ

日付: 2026-11-12。状態: 採用。関連: ADR-0047（KB）、ADR-0068（KB の GC）、ADR-0131 付記（日次整理の自動適用）、2026-10-27-daily-knowledge-extraction、2026-11-11-worker-knowledge-entry、ADR-0098（followups の env と run 後の吸収）。

## 根拠と範囲

task 01M75722TSX26Q8PXTG0RCZ72Q の依頼にある operator の観測（2026-10-28〜11-11）は、run の stdout.jsonl の tool 呼び出しを台本で数えたものである。CoS は 1583 run のうち KB の検索・取得をした run が 15 本、worker は 1995 run のうち 61 本（3.1%）。chat_events の tool の要約の 84% が `[redacted]` で、Claude Code 以外の harness の log は大まかにしか照合できない。harness の log の解析では読み取り率を継続して測れない。

curation は 10-28 以降の適用 101 回で新規 450・統合 2・削除 502・修正 42。統合の比率（統合 ÷（新規＋統合））は 0.4%。

## 決定

### D1 数える場所は KB の操作の入口（`celerisctl knowledge`）と API の受け側

1 回の KB の操作を、その操作が通る 1 か所だけで数える。

- worker run: `celerisctl knowledge search`・`get`・`record` が、env `CELERIS_KNOWLEDGE_USAGE_FILE` があれば、その file に 1 行の JSON（`{"op":"search"|"get"|"record","ok":true|false}`）を追記する。
- CoS run（run credential で API を呼ぶ）: API の受け側が credential から run を決めて数える（`task_api::cos::knowledge_use::note`）。受け側は次の 4 つ。
  - `GET /api/v1/knowledge/tree`（CoS の常設の文脈が案内する検索。`q` の無い一覧も KB を探す操作として数える）→ `search`
  - `GET /api/v1/knowledge/page` → `get`
  - `POST /api/v1/cos/operations` の `knowledge.record`（`POST /api/v1/knowledge/inbox`）→ `record`。`celerisctl knowledge record` の CoS の経路もここを通るので、CLI は数えない。
  - `POST /api/v1/cos/knowledge-use`（body `{op, ok}`。`op` は `search`・`get` だけで、`record` は 422）: CoS run の `celerisctl knowledge search`・`get` は KB の根を直接読み API を通らないので、入口がここに 1 回を送る。
- 人・web・worker の token の API 呼び出しは数えない（CoS run の credential が無い）。

どれも無いとき（人の手元、試験の外）は何も数えない。数えるのに失敗しても KB の操作は失敗にしない（計測の失敗で作業を止めない。失敗は stderr か daemon の log に 1 行出す）。

数えるのは op と成否だけ。検索語・path・本文は書かない（秘密の写しを増やさない）。台帳の形と読み取りの規則は `task_core::knowledge::usage` の 1 か所に置き、CLI と daemon が共に使う。

harness の log・chat_events の tool 要約から数える経路は作らない。CoS の `skill_reads`（harness の tool 呼び出しから数える既存の欄）はこの ADR の範囲外で、変えない。

### D2 台帳の置き場と run の帰属は daemon が決める

worker run: daemon は run を起こす前に、run の artifacts（WU の run は WU の artifacts）の下の `knowledge-usage.jsonl` を空にして置き、その path を followups の env（ADR-0098 D6。どちらも run の中の `celerisctl` の書き先）と一緒に渡す。前の run の台帳は `clear_stale_followups` が置き直す。Remote の run は台帳が手元に無いので置かない（`None`）。run の後に daemon が file を読み、合計を run の記録に付ける。どの run の数かは daemon が持つ run id で決め、file の中身に run id は書かせない（自己申告の帰属を使わない。ADR-0098 D2）。台帳の用意と読み取りは `task_dispatch::knowledge_usage`（dispatcher の module の外）に置く。dispatcher の module は repo health の巨大責務の閾値を超えているので、足すのは配線の行だけにする。

CoS run: CoS の起動（dispatcher の `cos_chat`）は変えない。run credential が run を決めるので、台帳の file も env も要らない。

数は観測値で、権限・完了・判断主体の根拠には使わない。agent が file を書き換えれば数は変わるが、計測の用途（読み取り率の傾向）には足りる。

### D3 保存先

- worker run（`WorkerFinished.role` が `worker` の run）: `RunMetrics.knowledge`（`KnowledgeUse { search, get, record }`）。`WorkerFinished` の `metrics` から `runs.metrics_json` に入る。env を渡した run は 0 回でも `Some`。`None` は「計測していない run」（この ADR より前の run と、env を渡さない経路）。読み取り率の分母は `Some` の run だけにする。
- reviewer・planner・設計 review の run は計測しない（`RunMetrics.knowledge` は `None`）。これらの run は task の artifacts を worker と共有し、followups の env（台帳の env を運ぶ経路）を渡さない。prompt にも KB の入口（2026-11-11-worker-knowledge-entry）を入れていない。report の行には出るが、計測した run が 0 本なので率は出ない。計測するときは run ごとの台帳の置き場（`runs/<run_id>/` など）を決めてこの ADR に付記する。
- CoS run: `chat_runs.knowledge_use_json`（migration 0125 で足す列）。値の形は worker と同じ `KnowledgeUse`。意味は同じで、置き場だけが run の表に合わせて分かれる。新しい行は列の既定値で 0 回から始まり（計測した run）、migration より前の行は NULL（計測していない run）。書き込みと読み取りは `task_core::KnowledgeUseStore`（`knowledge` の module。`chat` の module の SqliteStore の method 数は閾値を超えているので足さない）。版 124 は別の task の branch が使っているので予約にする（`RESERVED_VERSIONS`。123 は main に入った）。

events への追加は `RunMetrics` の任意欄だけで、既存の event は書き換えない（events は追記専用）。

### D4 report

`celerisctl knowledge usage [--json]`（DB は読み取りだけ。集計は `task_ops::knowledge_usage`）が次を出す。期間の絞り込みは置かない（計測していない run は分母に入らないので、計測を始めた後の run だけが数に入る）。

- agent の種類（worker・reviewer・planner は `WorkerFinished.role`、cos は `chat_runs`）と案件（CoS は thread の案件）ごと: 計測済み run 数、検索か取得を 1 回以上した run 数と率、検索・取得・記録の回数。
- curation（`KnowledgeCurationApplied`）ごと: 日付・新規・統合・削除・修正と、統合の比率（統合 ÷（新規＋統合）。分母 0 なら比率なし）。期間の合計も出す。

### D5 curation は新規より既存の更新・統合を先に選ぶ

日次整理の計画の決定的な検査（`task_ops::knowledge_curation::validate`）は、新規ページの提案が既存の正本と同じ話題なら拒否する。同じ話題の判定は決定的にする: 同じ最上位の置き場（`projects/<slug>/`・`environment/<分類>/` など、ADR-0047 の layout）にあり、題名を語に分けた集合の Jaccard 係数が 0.6 以上のページがある。拒否の理由には統合先の候補の path を書き、worker が `merge` に直せるようにする。拒否の文は worker がそのまま `fix`・`merge` に直せる形にする（`celerisctl curation validate` で worker が提出前に確かめられる）。curation の skill（本番 KB の `skills/` にあり repo には無い）の手順に「まず既存ページの更新・統合、無ければ新規」を足す作業は、本番 KB の変更なので task の進捗の「本番への移行」に人の手順として書く。

閾値 0.6 は観測の前の初期値で、D4 の report の統合比率を見て変える。変えるときはこの ADR に付記する。

## 撤去

- worker の prompt の索引の列挙は 2026-11-11-worker-knowledge-entry で撤去済み。
- 「候補は人が確認してから正本に入る」という古い説明は、前置き・guide・API 文書・architecture-map（同 ADR の変更）と、`celerisctl knowledge record` の出力・`--path` の help、`docs/ops/cos-chat.md`、知識抽出の prompt（`task_core::knowledge`）で直した。

## 検証

- D1: CLI の試験で、env を渡した search・get・record が台帳に op ごとに 1 行ずつ書かれ、env が無ければ書かれないこと。
- D2・D3: dispatcher の試験で、run の後に台帳の合計が `WorkerFinished.metrics.knowledge` に入ること（前の run の台帳を混ぜない）。task-api の試験（`tests/cos_knowledge_use.rs`）で、CoS run の credential の `GET /knowledge/tree`・`GET /knowledge/page`（404 は失敗として数える）・operation `knowledge.record`・`POST /api/v1/cos/knowledge-use` が credential の run の `knowledge_use_json` に数えられ、人の token の同じ呼び出し・body の run id・knowledge-use の `record` を受けず、NULL の run を数えないこと。
- D4: store の試験の DB で、agent の種類・案件ごとの率と curation の比率を固定する。
- D5: 同じ話題の既存ページがある新規の提案を検査が拒否し、統合の提案は通る試験。
