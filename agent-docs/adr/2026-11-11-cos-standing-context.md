---
tasks: [01M75722QN630BBASEQ759KG4T]
---
# ADR 2026-11-11: CoS の常設の文脈（役割・利用者・任された範囲・覚え書き・KB の入口）と、人の委任の grant 化

- 状態: 採用。D1〜D3・D6 は実装済み、D4・D5 は同じ task の続きの run で実装する（[進捗](../progress/2026-11-11-cos-standing-context.md)）。
- 関係: [ADR 2026-11-04 project controller](2026-11-04-project-controller.md) A7（authority ledger）、
  [ADR 2026-10-27 取り次いだ指示](2026-10-27-relayed-instructions.md)、
  [ADR 2026-10-28 窓口は統括 CoS 1 本](2026-10-28-single-human-window-general-cos.md)、
  [ADR 2026-10-08 CoS chat の prompt cache](2026-10-08-cos-chat-prompt-cache.md) D6（Core は byte 単位で同じ）、
  ADR-0033 D5（standing_rules）、ADR-0047（KB）、[ADR 2026-10-11 破壊的変更](2026-10-11-breaking-changes-by-default.md)。

## 背景（本番で operator が読み取りで確かめた数字。2026-10-28〜11-11）

- CoS の run の `RunContext` は `..RunContext::default()` で組まれ、CoS run 1583 本のうち standing rule・memory・KB の節を
  持つ prompt は 0 本だった。org の cos の profile（`kb:user`・`kb:projects`・`memory`）はどこからも読まれていない。
  Core は「standing rule の指示に従う」と書くが、何も入らない。
- 統括 CoS の取り次ぎ（`chat.relay_forward`）は task の指定と期限が必須で、relay 33 件はすべて task 1 件、統括 CoS の
  26 件は 4〜48 時間で切れた。少なくとも 6 件は常設の指示（「job 待ちは基本認可」など）で、切れた後に BenchFS の CoS は
  job 待ちの上限を 5 回人に回した。
- authority ledger は 0 行、standing_rules は 2 行で worker の prompt にだけ入る。CoS の記憶は thread ごとの checkpoint の
  要約だけ。KB を検索した CoS run は 15 本、人に回した 197 本は 1 度も検索していない。

## 決定

### D1 層と正本

| 層 | 正本 | prompt に入るもの | 上限（字） | 更新の経路 |
| --- | --- | --- | --- | --- |
| soul（境界・口調） | `task_worker::cos_chat::core`（Global Core） | Core そのもの（今と同じ） | 既存のまま | コードの変更（review を通す） |
| identity（役割） | `task_worker::cos_chat::standing` の thread の種類ごとの固定文 | 統括・案件・review・受信箱の一次対応の役割の節 | 600 | コードの変更 |
| user（利用者） | KB の `user/*.md` | page ごとの題名と本文の先頭（front matter を除く） | page 600・節 3000 | KB の通常の経路（人の編集・curation） |
| memory: 任された範囲 | authority ledger の grant（`authority_grants`） | 生きている grant の id・範囲・分類・期限・残り回数・人の発言の抜粋 | 節 4000 | 人の受け入れ（D4）・取り消し |
| memory: 覚え書き（権限でない慣習・好み・道具の癖） | KB の `cos/memory.md` | 本文の末尾（新しい追記）を上限まで | 2000 | CoS が KB 候補として出し（`POST /api/v1/knowledge/inbox`、scope `cos`）、日次 curation か人の受け入れで入る |
| KB の入口 | cos の org profile の knowledge の mount | 検索してよい scope と、検索・取得の API の書き方だけ（索引は入れない） | 800 | org の profile（`PATCH /api/v1/org`） |

- Core を写した別 file（SOUL.md・IDENTITY.md）は作らない。役割の節は Core に入れず run 固有の部分に置く（Core は
  thread の種類によらず byte 単位で同じに保つ。ADR 2026-10-08 D6）。
- 合計の上限は 10000 字。節の上限を超えた分は古いものから落とし、「ほか N 件（<読み方>）」の 1 行に置き換える。
  grant は新しい順、user は path の順、覚え書きは末尾を残す。
- `standing_rules`（ADR-0033 D5）は worker の prompt の経路のまま変えない。CoS の権限の正本は grant だけにし、
  standing_rules を CoS へ写さない（正本を 2 つにしない）。

### D2 どの CoS run にも入れる

- CoS の chat run（統括・案件・review の thread、受信箱の一次対応を含む）はすべて `cos_chat::launch` の 1 か所で起動する。
  ここで `CosChatContext.standing`（`CosStandingContext`）を必ず組む。組めない（KB の根が無い等）節は「無い」と書いて空にし、
  run は止めない（KB が無い host でも CoS は動く）。ledger が読めないときは run を起こさない（fail closed。任された範囲を
  欠いたまま答えさせない）。
- `RunContext` は `..RunContext::default()` で組まない。CoS の run が使う欄（`cos_chat`・`skills`・`session`）以外を明示し、
  cos の org profile は `standing.knowledge_scopes` に解決して使う（未使用の profile を残さない）。
- grant の範囲: 案件 thread はその案件の grant と global の grant、統括・review・受信箱は全案件と global の grant。

### D3 入ったことを数える

- 常設の節の先頭に機械向けの 1 行 `<!-- celeris:cos-standing v1 role=<kind> grants=<件数>/<字数> user=<page 数>/<字数>
  memory=<0|1>/<字数> kb=<scope 数>/<字数> omitted=<落とした件数> -->` を置く。`runs/<run_id>/prompt.txt` を数えれば
  節ごとの有無と字数が分かる（新しい列・event を足さない）。

### D4 global の範囲と、期限の無い委任（続きの run で実装）

- `ScopeKind::Global`（`ref = "*"`）を足す。global の grant は `project_id = '*'` の行で、どの案件の範囲も含む
  （`scope_covers`）。migration で `authority_grants.scope_kind` の CHECK を作り直す（行は写す。データを失わない）。
  API は `GET/POST /api/v1/authority`・`…/grants`・`…/proposals`（案件の API と同じ本文・同じ人の token の規則）。
- 分類 `routine_answer` を足す。CoS の定型の代答（受信箱の `job_wait_extend`・`plan_conforms`・衝突の統合など）を
  人に回さずに答えてよいという委任で、grant の新しい欄 `answer_kind` に代答の種類を 1 つ持つ。`routine_answer` の grant
  だけは期限と回数を省ける（取り消すまで有効）。他の分類は今のまま期限・回数が必須。
- 人が期限・範囲を限らずに任せた指示は、CoS が `authority.grant_propose`（根拠は人の発言の id）で 1 度だけ確かめ、
  人が受け入れたら grant になる。統括 CoS は常設の指示を 1 task・24 時間の取り次ぎに絞らない（Core と skill を直す）。
- 取り次ぎ（`chat.relay_forward`）は `grant_id` を持てる。生きた grant がその案件を含むなら、`task_ids` と `expires_at` を
  要らないとする（window.rs の検査を grant の範囲に合わせる）。
- 代答の記録は判断の根拠（grant id・人の発言の出典）と実行者（`cos:<thread>`）を別々に残す（authority_audit の
  `human_decision_ref = authority_grant:<id>`・`actual_actor`。ADR 2026-10-27・`docs/ops/external-operator.md` §4）。

### D5 初期の委任の種

- 本番の relay の原文と operator が預かる指示を grant の提案の一覧として進捗に書き、配備後に operator が提案として登録し、
  人が受け入れる。worker は本番に登録しない。本番の昇格（promote）は含めない（`docs/ops/external-operator.md` の分担）。

### D6 撤去

- `cos_chat.rs` の「CoS chat run には context 依存の節は空」の注釈と、`..RunContext::default()` の CoS の経路を消す。
- D4 の実装で、取り次ぎの task 必須・期限必須の検査（grant の無い取り次ぎには残す）と、skill の「`expires_at` は 24 時間後」
  の書き方を消す。

## 不変条件と試験

- どの種類の CoS thread の prompt にも常設の節が入り、各節と合計が上限内（`task_worker::cos_chat::standing` の snapshot 試験、
  dispatcher の CoS 起動の試験）。
- Core は thread の種類によらず byte 単位で同じ（既存の Core の試験）。
- 許可は人だけが作る。`human_required` は許可で解除できない（既存の authority ledger の試験）。

## 考えた別案

- SOUL.md・USER.md・MEMORY.md を workspace に置いて毎回読む（OpenClaw 型）: 正本が Core・KB・ledger と 2 つになり、
  権限の取り消し・監査が効かない。採らない。
- standing_rules を CoS にも入れる: 範囲・期限・監査が無く、ledger と正本が 2 つになる。採らない。
- 記録を event にする: event の種類を足すと高 risk の path を変える。prompt.txt の 1 行で数えられるので足さない。
