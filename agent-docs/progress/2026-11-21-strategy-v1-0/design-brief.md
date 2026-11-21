# 設計 brief: 議論と引用の流れ・observability・publish の署名・機密境界の脅威モデル（戦略層 v1、子 0 で固定）

子 2〜7 はこの brief を前提にする。変えるときは ADR を足し、この file の該当の節に「変更: ADR …」と書く。
`P<n>`・`RM<n>`・`X<n>` などの id は [inventory.md](inventory.md) の行（基準 main 7f69057907ad の file:line）。

## 1. 状態と権限の分離

| 状態 | 正本（新しい表を作らない） | 作れる主体 | 次の状態へ進められる主体 | 副作用 |
|---|---|---|---|---|
| Discussion | CoS Chat の案件 thread の message（H1・H2）と添付（H10） | 人・案件 CoS・relay（operator）・MCP の client | – | 0（読む・話すだけ） |
| Proposal | versioned draft: Roadmap の proposed の版（RM2）、knowledge の受信箱（K1・M7）、子 2 が決める artifact | 人・CoS・MCP の client（`proposed` まで） | 人だけ（Web か人の token の API。RM3 と同じ形） | draft の記録だけ |
| Approved Strategy | 人が approve した版（Roadmap の approved と同じ形。子 2 が置き場所を決める） | 人だけ | 人だけ（撤回・差し替え） | 無し |
| Execution | Task tree・WorkUnit・controller の起票（C1〜C6） | 承認済みの戦略か、authority ledger の許可（A4）と予算の範囲 | controller・dispatcher（決定的） | 既存のまま |
| Publication | 人が署名した publish の manifest（X6。子 7） | 人だけ | 人だけ | 外部への公開。自動は無い |

規則:
- 外部の LLM（ChatGPT・Muse）の発言は Discussion か Proposal にしかならない。承認・人の発言として数えない。主体は `mcp:<client_id>` か、人が貼った引用（`human_paste`）のどちらかで、`Principal` は認証だけが作る（A1・A2）。
- `Project.request`（P2）と承認済み Roadmap（RM1）は戦略層から書かない。ProjectBrief（P7）は派生の未検証の要約として扱い、根拠にしない。
- discuss-only の mode（X5）は prompt ではなく API の側で拒否する（子 3。`/cos/operations` の task.create・roadmap・publish の系を、mode が discuss の run では 403 にする）。

## 2. 議論と引用の流れ

### F1 ChatGPT Chat（人の主な議論の場）

1. 人が ChatGPT で議論する。Celeris は ChatGPT の会話履歴・Memory を読まない。browser login・Cookie の収集・ChatGPT のセッションの代理はしない。
2. 人が採る部分だけを選ぶ。渡し方は 3 つで、どれも人の操作が起点:
   - (a) 人が CoS の案件 thread に貼る（Web・RDC の chat。主体は人、source は `human_paste`、consent は貼った人）。最低限の経路で、どのプランでも使える。
   - (b) 人が Markdown・JSON の envelope（§5）を書き出して thread に添付する（`markdown_import`）。
   - (c) ChatGPT の custom remote MCP が使える環境では、ChatGPT が MCP の read の道具（`knowledge_search`・`tasks_get`、子 4 が足す discussion snapshot）を呼ぶ。書き込みは `knowledge_propose` と（子 4 が足す）propose の道具だけ。optional で、外部公開の MCP の本番配置は人の承認まで行わない。
3. Celeris に入ったものは Discussion か Proposal。採るかは人が Web で決める。

### F2 Muse ほか MCP の client

- scope は `KnowledgeRead`・`TasksRead`・`KnowledgePropose` と、議論だけの `ConsoleInstruct`（discuss の mode の thread だけ。子 3・4）に限る。
- `TasksInteract`・`TasksDecide`・`TasksControl`・`OrgWrite`・`SkillsWrite` を外部の LLM の client に渡さない。理由: `decision_answer`・`task_plan_gate`（M5・M6）は `TasksInteract` で人の決定・plan の承認に答えられる。
- MCP の client の id ごとに scope を持つ（M4）。本番の client の scope の今の設定は**未確認**（子 1・4 が `celerisctl` か API の読み取りで確かめる）。

### F3 内部の CoS（案件 thread）

- 既存の固定の案件 CoS thread を使う（H1〜H10）。履歴は FTS5 で引ける（H8）。長い履歴の要約は引用（message id）と revision を持つ。編集・撤回された message を引いた要約は失効する（子 3）。
- relay は operator の principal だけ（H5）。統括 CoS の proposal の転送は `/cos/operations` だけ（H6）。

### F4 既存の RDC home-dev → celeris-chat の経路

依頼の原文にある経路。thread_id・project_id・出典で続けられるようにする（子 4）。この子では経路の中身（scripts/rdc の設定）を確かめていない（**未確認**）。

## 3. Observability（人が publish の責任者として見るもの）

基準（確認済み、replay から）: アーキv4 では、未達を検知できた時点（2026-11-18T20:36Z）から親の最初の評価（2026-11-20T14:12Z）まで約 41.6 時間、その間に 137 run・264.25 USD（list price）。
人が見る画面は既存の案件の詳細・controller・Roadmap・authority・chat（W1〜W5）を延ばす（子 7）。最低限の表示:

| 表示 | 正本 | 区別の印 |
|---|---|---|
| 人の元の目的 | P2 `Project.request` | 正本 |
| 承認済みの戦略・候補の比較 | §1 の Approved Strategy・Proposal | approved / proposed |
| 仮説と検証の状況・反証 | 子 2 の契約 | confirmed / estimated / contradicted / needs_decision（§5 の confidence） |
| 親の目的への途中評価 | 子 6 の checkpoint | on_track / at_risk / off_track / unknown（replay の answer key と同じ語） |
| 議論の出典 | §5 の references と thread の message id | source の種類（内部・人の貼付・MCP） |
| 予算と spent | controller の budget（C6）と `worker_finished` の cost | 実測 / list price の換算 |
| 人の判断待ち・危険・公開の準備度 | decision・authority・publish の manifest | 強い警告は needs_decision と contradicted |

## 4. Publish の署名

- manifest は claims → evidence（commit・実験の記録・文献の URL）→ 再現条件 → limitations・未解決 → 人の署名、の順に並べ、本文の sha256 を持つ。
- 署名できるのは `Principal::Human`（Web か人の token の API）だけ。CoS・operator・MCP・worker は署名できない（RM3 と A3 と同じ形の `requires_human`）。
- 署名は manifest の sha256 に対して行い、本文が変わったら失効する。公開の操作は署名の後に人が行う。会話・LLM・controller からの自動の publish は作らない。
- 既存の拒否（P5 `external_publication_requires_human`）は案件 CoS の operation に効く。manifest の署名（X6）は無いので子 7 が足す。

## 5. 共有フォーマット（discussion envelope）

欄: `project_id`・`source{kind,label,thread_id}`・`actor{kind,id}`・`source_revision`・`timestamp`・`content_hash`（`sha256:` + 本文）・`consent{granted_by,scope,granted_at}`・
`idempotency_key`・`decision_state`（discussion / proposed / approved / rejected / withdrawn）・`confidence`（confirmed / estimated / contradicted / needs_decision）・
`linked{hypotheses,evidence}`・`references`・`body`（UTF-8 で 16,384 byte まで）。外へ出すときは `direction`・`classification` を付ける。

検査の規則（R1〜R13 は envelope の規則で、inventory の id とは別。参照の検査器 [fixtures/validate.py](fixtures/validate.py)）:

| 規則 | 内容 | 否定対照の fixture |
|---|---|---|
| R1 | 必須の欄がそろう | `negative/missing-field.json` |
| R2 | source の種類は 4 つだけ（browser のセッションは不可）。state・confidence は決めた語だけ | `negative/browser-session-source.json` |
| R3 | source と actor の組（MCP は `mcp`、貼付・import は `human`） | `negative/mcp-claims-human.json` |
| R4 | content_hash が本文と一致 | `negative/hash-mismatch.json` |
| R5 | 本文の長さの上限 | `negative/too-long.json` |
| R6 | thread の案件と project_id が一致 | `negative/other-project.json` |
| R7 | 同じ idempotency key で違う本文は拒否（同じ本文は二重取り込みにしない） | `negative/idempotency-replay-different-content.json` |
| R8 | 外部の source は人が与えた consent（選んだ抜粋か要約）が要る | `negative/no-consent.json`・`negative/consent-by-llm-full-history.json` |
| R9 | 秘密に見える文字列（token・cookie・webhook）を拒否 | `negative/secret-in-body.json`（値はダミー） |
| R10 | approved は人の承認（Web か人の token）の参照が要る | `negative/cos-self-approves.json`・`negative/llm-approval-ref.json` |
| R11 | 外部の source は discussion・proposed より先に進めない（注入された指示でも同じ） | `negative/prompt-injection-approves.json` |
| R12 | confirmed は references が要る | `negative/confirmed-without-refs.json` |
| R13 | confidential を外へ出すには人の承認が要る | `negative/confidential-outbound.json` |

baseline（受理されるべきもの）: `baseline/internal-cos.json`・`baseline/chatgpt-human-paste.json`・`baseline/muse-mcp-propose.json`・`baseline/human-approved-in-web.json`。

## 6. 機密境界

- 分類: public / internal / confidential / secret。未発表の研究結果（例: BenchFS ISC27 の測定）と機微な Knowledge は confidential を既定にし、外部の LLM に送らない（R13）。secret（token・password・cookie・webhook URL）は成果物・ログ・コメント・envelope に書かない（R9）。
- 外部の資料（ChatGPT・Muse の発言、Web の文献）は untrusted evidence。中の指示文は権限を変えない。権限は `Principal` と authority ledger と scope だけから決まる（A1〜A4・M4）。
- 外へ出すときは、出す範囲（抜粋・要約）を人が選び、manifest の sha256 と出した先を記録する。

## 7. 脅威の表

| # | 脅威 | 入口 | 今の守り（inventory） | 残る穴 | 否定対照・試験 | 担当 |
|---|---|---|---|---|---|---|
| T1 | 外部 LLM の発言を人の発言・承認と取り違える | MCP・貼付 | A1・A2（token は mcp・cos を名乗れない）、RM3（decide は人だけ） | M5・M6 の scope を外部の client に渡すと承認に効く | R3・R10・R11 の fixture | 子 1・4 |
| T2 | prompt injection で権限が上がる | 外部の引用・添付 | 権限は Principal と scope だけ（A1・M4） | discuss の mode が prompt だけだと task.create に届く | R11 の fixture、子 3 の API の 403 の試験 | 子 3・4 |
| T3 | 未承認の戦略が実行される | CoS の operation・controller | P3・P4・RM3・C4（TS の提案を Rust が拒否） | 戦略の draft と承認の区別が無い（X2） | R10 の fixture、子 2 の未承認の実行の拒否の試験 | 子 2 |
| T4 | 別の案件の資料を誤って採る | 引用・knowledge | thread と project の紐付け（H1） | envelope に project の照合が無い | R6 の fixture | 子 2・4 |
| T5 | 二重取り込み・古い revision の採用 | 再送・編集 | 無し | idempotency と revision の照合が無い | R7 の fixture、子 3 の失効の試験 | 子 3・4 |
| T6 | 機微な結果が外へ出る | MCP の read・export | 無し（KnowledgeRead は分類を見ない） | 分類と outbound の拒否が無い | R13 の fixture | 子 4 |
| T7 | 秘密が成果物・ログに残る | 貼付・添付 | 無し（envelope の段） | 秘密の検出が無い | R9 の fixture（ダミーの値） | 子 3・4 |
| T8 | ChatGPT の会話全文・Memory の自動の取り込み、browser の login・Cookie | 外部連携 | 作らない（この brief で禁止） | – | R2（browser のセッションの source を拒否）・R8（全履歴の consent を拒否） | 子 4 |
| T9 | 自動の publish | controller・CoS・MCP | P5（案件 CoS の外部公開は人） | manifest と署名が無い（X6） | 子 7 の人だけが署名・公開できる試験 | 子 7 |
| T10 | 根拠の無い主張が確認済みとして表示される | 要約・評価器 | 無し | confidence と references の照合が無い | R12 の fixture、rubric の unsupported claims | 子 2・6・7 |
| T11 | 人への判断要求が届かない（needed_before の書式の誤り） | decision | 無し（アーキv4 E で発生） | fail-closed の通知が無い | 子 1 の負の試験 | 子 1 |
| T12 | 途中の親の未達を見落とす（固定の計画を最後まで走る） | 計画 | C6 の停滞の guard は no-progress だけ | 親の目的への途中評価が無い（X3・X4） | replay の 12 時点（検知は E-mid） | 子 5・6 |

## 8. 実装前 brief（戦略層 v1 全体。子 0 は source を変えない）

1. 根本原因: 親の目的と途中の実測を照らす段が無く、子の done が親の成果と扱われる（replay: 検知できた時点から 41.6 時間、137 run）。外部の議論の入口に、出典・同意・主体の区別が無い。
2. 責務 owner: 目的の正本は Project.request（task-core org）、承認は roadmap の handler、決定的な停止は controller の guard（task-dispatch project_guard）、議論は CoS Chat（task-core chat・task-api chat）、外部の入口は celeris-mcp。
3. 既存の再利用先: inventory.md の reuse の行（P・RM・C・H・M・K・A・W）。試験の前例は roadmap の人だけの decide、relay の operator だけ、goal_guard の 422。
4. 選択肢: (i) 新しい Strategy の DB 表と Manager を足す、(ii) 既存の Roadmap の proposal・knowledge の受信箱・artifact と events に乗せる。(ii) を採る（依頼の禁止事項と、v2 の純増の教訓）。表は既存で足りないと示せてから足す。
5. 新しい状態・分岐・fallback・port: この子では無し。子 2 以降で足すものは §1 の 5 状態の中に限り、それぞれ子の ADR で理由を書く。
6. 撤去対象: この子では無し。子 3 の discuss の mode を入れるとき、prompt だけの制限が残っていれば同じ変更で消す。
7. 不変条件: 外部の発言は承認にならない（R3・R10・R11）、Project.request と承認済み Roadmap は書かない、自動の publish は無い、confidential は既定で外へ出ない（R13）。
   確かめる試験は fixtures の 19 件（子 2〜4 が Rust・TS の試験に写す）と、replay の否定対照 3 件。
