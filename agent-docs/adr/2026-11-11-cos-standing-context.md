---
tasks: [01M75722QN630BBASEQ759KG4T]
---
# ADR 2026-11-11: CoS の常設の文脈（役割・利用者・任された範囲・覚え書き・KB の入口）と、人の委任の grant 化

- 状態: 採用。D1〜D4・D6 は実装済み。D5 の登録は配備後に operator と人が行う（[進捗](../progress/2026-11-11-cos-standing-context.md)）。
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
| memory: 任された範囲 | authority ledger の grant（`authority_grants`） | 生きている grant の id・範囲・分類・期限・残り回数・人の発言の抜粋 | 節 3600 | 人の受け入れ（D4）・取り消し |
| memory: 覚え書き（権限でない運用の慣習・道具の癖） | KB の `environment/celeris/cos-memory.md`（`task_worker::protocol::COS_MEMORY_PAGE`） | 本文の末尾（新しい追記）を上限まで | 2000 | CoS が KB 候補として出し（`knowledge.record`、`POST /api/v1/knowledge/inbox` の body に scope `environment/celeris`・path `environment/celeris/cos-memory.md`）、日次 curation か人の受け入れで入る |
| KB の入口 | cos の org profile の knowledge の mount | 検索してよい scope と、検索・取得の API の書き方だけ（索引は入れない） | 800 | org の profile（`PATCH /api/v1/org`） |

- 覚え書きの置き場は KB の既存の分類 `environment/celeris/`（Celeris 自身の運用の癖）に置く。KB の書き込みはどの経路
  （`knowledge.record`・`celerisctl knowledge record`・curation の `apply_candidates`）も置き場のガード
  `task_core::knowledge::layout::place` を通り、ガードが受ける最上位は `user/`・`environment/<分類>/`・`projects/<slug>/`・
  `experience/YYYY/MM/` だけである。当初の `cos/memory.md`（scope `cos`）はガードに拒まれ、CoS も curation も書けなかった。
  最上位の `cos/` を足す案は、置き場の規則・scope の語彙・mount の scope を増やすので採らない。利用者の好みは覚え書きに
  入れず `user/preferences.md` の候補にする。prompt が示す scope・path と dispatcher が読む page は同じ定数
  （`COS_MEMORY_PAGE`・`COS_MEMORY_SCOPE`）から作り、試験でガードを通ることを確かめる。
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

### D4 global の範囲と、期限・task に縛られない委任

- `ScopeKind::Global`（`ref = "*"`）を足す。global の grant は `project_id = '*'` の行で、どの案件の範囲も含む
  （`scope_covers`。案件の判定は自分の案件の行と global の行を候補にする）。migration 0123 で `authority_grants` を
  作り直して CHECK に `global` と新しい分類を足す（行は写す。追記専用の trigger と index も作り直す）。表の CHECK で
  「global の行は `project_id = '*'`・`scope_ref = '*'`・定型の代答の分類だけ」を固定する。
  API は案件の台帳と同じ `/api/v1/projects/{id}/authority…` で、`{id}` に `global` を書く（本文・人の token の規則は同じ。
  新しい route を足さない）。案件の `GET …/projects/<案件>/authority` は、その案件の行に加えて global の台帳の行
  （許可と提案。`project_id = '*'`）も返す。行への操作（受け入れ・断る・取り消し）は行の台帳の path（`*` は `global`）へ
  送る。案件の path で global の行を取り消すと `grant_missing`（404）になる。web の許可の画面は行の `project_id` から
  path を作り、global の行に「全案件に効く」と出す。global の提案の通知は「どの案件の画面の『許可』からでも決められる」
  と書く（`authority::proposal_summary`）。global の台帳だけの画面は作らない。
- 定型の代答の委任は、代答の種類ごとの分類にする（初めは `job_wait_extend`。`ActionClass::is_routine_answer`）。
  当初案の「分類 `routine_answer` と欄 `answer_kind`」は採らない: 欄を足すと grant の型と表の全経路が変わり、分類 1 つで
  種類を表せば判定（分類の一致）と CHECK だけで足りるため。種類を足すときは分類を足す。
- 期限: 定型の代答の分類だけ期限の上限を 3660 日（`MAX_ROUTINE_GRANT_DAYS`）にする。他の分類は今のまま 366 日。
  期限の欄を省ける形（NULL）は採らない: 期限の欄は判定・表示・監査の全経路が必須として読むため、型を変えずに
  「人が取り消すまで続く」を表す。回数は 1000 回まで（使うたびに 1 消費）。
- 効果: 受信箱の cluster job の待ちの上限（`basis.category = job_wait_extend`）で、basis の refs が生きた `job_wait_extend`
  の grant（global かその task の案件、範囲は global か案件全体）を挙げていれば、同じ job の上限到達 3 回の制限
  （ADR 2026-10-30-cos-job-wait-extend D3）で人に回さない。job が全て待ち（Q）であることなど他の条件は変えない。
- 人が期限・範囲を限らずに任せた指示は、CoS が `authority.grant_propose`（根拠は人の発言の id）で 1 度だけ確かめ、
  人が受け入れたら grant になる。統括 CoS は常設の指示を 1 task・24 時間の取り次ぎに絞らない（常設の文脈の役割の節と
  `cos-inbox-triage` の §10 を直す）。
- 取り次ぎ（`chat.relay_forward`）は `grant_id` を持てる。その grant が生きていて（取り消し・期限・回数・元の根拠）、
  global の台帳か取り次ぎ先の案件の台帳の、範囲が global か案件全体の行なら、`task_ids` と `expires_at` を省ける。
  `relay.grant_id` に残し、`instructed_by` に使うたびに同じ照合をやり直す（取り消した grant の取り次ぎは根拠にならない）。
  取り次ぎがまだ根拠になるか（期限・grant）は `task_core::chat::RelayLapse` の 1 つの判定にし、`instructed_by` と
  prompt に入れる取り次ぎの一覧（`chat_relayed_instructions`）が同じ判定を使う（取り消した grant の取り次ぎは一覧からも消える）。
  `grant_id` の無い取り次ぎは今のまま task と期限が必須。
- 代答の記録は判断の根拠と実行者を別々に残す: 代答が applied になった後に `authority_audit` へ 1 行（`grant_id`、
  `human_decision_ref = authority_grant:<id>`、`actual_actor = cos:<thread>`、`outcome_state = applied`）を書き、grant の
  元の根拠（人の発言の id）は grant の行から引ける（ADR 2026-10-27-relayed-instructions、`docs/ops/external-operator.md` §4）。
  `cos_operations` の監査は今のまま basis（refs に grant の id）と CoS の actor を持つ。

### D5 初期の委任の種

- 本番の relay の原文と operator が預かる指示を grant の提案の一覧として進捗に書き、配備後に operator が提案として登録し、
  人が受け入れる。worker は本番に登録しない。本番の昇格（promote）は含めない（`docs/ops/external-operator.md` の分担）。

### D6 撤去

- `cos_chat.rs` の「CoS chat run には context 依存の節は空」の注釈と、`..RunContext::default()` の CoS の経路を消す。
- 取り次ぎの task 必須・期限必須の検査は grant の無い取り次ぎにだけ残し、skill の「`expires_at` は 24 時間後」の
  書き方を消した（D4）。

## 不変条件と試験

- どの種類の CoS thread の prompt にも常設の節が入り、各節と合計が上限内（`task_worker::cos_chat::standing` の snapshot 試験、
  dispatcher の CoS 起動の試験）。
- Core は thread の種類によらず byte 単位で同じ（既存の Core の試験）。
- 許可は人だけが作る。`human_required` は許可で解除できない（既存の authority ledger の試験）。
- prompt が示す覚え書きの足し方（scope・path）は KB の置き場のガードを通り、受け入れると dispatcher が読む page に入る
  （`cos_standing_memory_note_route_passes_the_kb_placement_guard`、task-api の
  `the_cos_memory_note_route_records_and_accepts_into_the_memory_page`）。

## 考えた別案

- SOUL.md・USER.md・MEMORY.md を workspace に置いて毎回読む（OpenClaw 型）: 正本が Core・KB・ledger と 2 つになり、
  権限の取り消し・監査が効かない。採らない。
- standing_rules を CoS にも入れる: 範囲・期限・監査が無く、ledger と正本が 2 つになる。採らない。
- 記録を event にする: event の種類を足すと高 risk の path を変える。prompt.txt の 1 行で数えられるので足さない。
