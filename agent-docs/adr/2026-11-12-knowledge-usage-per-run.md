# KB の読み取りを run ごとに入口で数え、curation は更新・統合を先に選ぶ

日付: 2026-11-12。状態: 採用。関連: ADR-0047（KB）、ADR-0068（KB の GC）、ADR-0131 付記（日次整理の自動適用）、2026-10-27-daily-knowledge-extraction、2026-11-11-worker-knowledge-entry、ADR-0098（followups の env と run 後の吸収）。

## 根拠と範囲

task 01M75722TSX26Q8PXTG0RCZ72Q の依頼にある operator の観測（2026-10-28〜11-11）は、run の stdout.jsonl の tool 呼び出しを台本で数えたものである。CoS は 1583 run のうち KB の検索・取得をした run が 15 本、worker は 1995 run のうち 61 本（3.1%）。chat_events の tool の要約の 84% が `[redacted]` で、Claude Code 以外の harness の log は大まかにしか照合できない。harness の log の解析では読み取り率を継続して測れない。

curation は 10-28 以降の適用 101 回で新規 450・統合 2・削除 502・修正 42。統合の比率（統合 ÷（新規＋統合））は 0.4%。

## 決定

### D1 数える場所は KB の操作の入口（`celerisctl knowledge`）

`celerisctl knowledge search`・`get`・`record`（CoS run の API 経由の `record` を含む）は、env `CELERIS_KNOWLEDGE_USAGE_FILE` があれば、その file に 1 行の JSON（`{"op":"search"|"get"|"record","ok":true|false}`）を追記する。env が無いとき（人の手元、試験の外）は何も書かない。追記に失敗しても KB の操作は失敗にしない（計測の失敗で作業を止めない。失敗は stderr に 1 行出す）。

数えるのは op と成否だけ。検索語・path・本文は書かない（秘密の写しを増やさない）。台帳の形と読み取りの規則は `task_core::knowledge::usage` の 1 か所に置き、CLI と daemon が共に使う。

harness の log・chat_events の tool 要約から数える経路は作らない。CoS の `skill_reads`（harness の tool 呼び出しから数える既存の欄）はこの ADR の範囲外で、変えない。

### D2 file の置き場と run の帰属は daemon が決める

daemon は run を起こす前に、run の artifacts の下の `knowledge-usage.jsonl` を消し、その path を env で渡す（ADR-0098 D1・D6 の followups と同じ手順）。run の後に daemon が file を読み、合計を run の記録に付ける。どの run の数かは daemon が持つ run id で決め、file の中身に run id は書かせない（自己申告の帰属を使わない。ADR-0098 D2）。

数は観測値で、権限・完了・判断主体の根拠には使わない。agent が file を書き換えれば数は変わるが、計測の用途（読み取り率の傾向）には足りる。

### D3 保存先

- worker run（worker・reviewer・planner）: `RunMetrics.knowledge`（`KnowledgeUse { search, get, record }`）。`WorkerFinished` の `metrics` から `runs.metrics_json` に入る。env を渡した run は 0 回でも `Some`。`None` は「計測していない run」（この ADR より前の run と、env を渡さない経路）。読み取り率の分母は `Some` の run だけにする。
- CoS run: `chat_runs.knowledge_use_json`（migration で足す列）。値の形は worker と同じ `KnowledgeUse`。意味は同じで、置き場だけが run の表に合わせて分かれる。

events への追加は `RunMetrics` の任意欄だけで、既存の event は書き換えない（events は追記専用）。

### D4 report

`celerisctl knowledge usage [--since <RFC3339>] [--json]`（DB は読み取りだけ）が次を出す。

- agent の種類（worker・reviewer・planner・cos）と案件ごと: 計測済み run 数、検索か取得を 1 回以上した run 数と率、検索・取得・記録の回数。
- curation（`KnowledgeCurationApplied`）ごと: 日付・新規・統合・削除・修正と、統合の比率（統合 ÷（新規＋統合）。分母 0 なら比率なし）。期間の合計も出す。

### D5 curation は新規より既存の更新・統合を先に選ぶ

日次整理の計画の決定的な検査（`task_ops::knowledge_curation::validate`）は、新規ページの提案が既存の正本と同じ話題なら拒否する。同じ話題の判定は決定的にする: 同じ最上位の置き場（`projects/<slug>/`・`environment/<分類>/` など、ADR-0047 の layout）にあり、題名を語に分けた集合の Jaccard 係数が 0.6 以上のページがある。拒否の理由には統合先の候補の path を書き、worker が `merge` に直せるようにする。curation の skill の手順も「まず既存ページの更新・統合、無ければ新規」に直す。

閾値 0.6 は観測の前の初期値で、D4 の report の統合比率を見て変える。変えるときはこの ADR に付記する。

## 撤去

- worker の prompt の索引の列挙は 2026-11-11-worker-knowledge-entry で撤去済み。
- 「候補は人が確認してから正本に入る」という古い説明は同 ADR の変更で直した。

## 検証

- D1: CLI の試験で、env を渡した search・get・record が台帳に op ごとに 1 行ずつ書かれ、env が無ければ書かれないこと。
- D2・D3: dispatcher の試験で、run の後に台帳の合計が `WorkerFinished.metrics.knowledge` に入ること、CoS run の行に `knowledge_use_json` が入ること。
- D4: store の試験の DB で、agent の種類・案件ごとの率と curation の比率を固定する。
- D5: 同じ話題の既存ページがある新規の提案を検査が拒否し、統合の提案は通る試験。
