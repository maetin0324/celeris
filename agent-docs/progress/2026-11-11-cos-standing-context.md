---
title: CoS の常設の文脈（任された範囲・利用者・覚え書き・KB の入口）と人の委任の grant 化
tasks: [01M75722QN630BBASEQ759KG4T]
status: done
updated: 2026-11-11
---
# CoS の常設の文脈（任された範囲・利用者・覚え書き・KB の入口）と人の委任の grant 化

完了日: 2026-11-11

設計判断は [ADR 2026-11-11 CoS の常設の文脈](../adr/2026-11-11-cos-standing-context.md)。基準は `git merge-base HEAD main`。

## この run でしたこと（attempt 1）

- ADR を書いた（D1 層と正本・上限・更新の経路、D2 全 CoS run への注入、D3 数え方、D4 global と期限なしの委任、D5 種、D6 撤去）。
- `task_worker::protocol::CosStandingContext`（`context.cos_chat.standing`）を足した。役割・grant・`user/*.md`・`cos/memory.md`・
  KB の scope を持つ。
- `task_worker::cos_chat::standing` が常設の節を描く。節ごとの上限（役割 600・grant 3600・user 3000〈page 600〉・覚え書き 2000・
  KB 800、合計 10000 字）を超えた分は古いものから落とし「ほか N 件」の 1 行にする。先頭に数える行
  `<!-- celeris:cos-standing v1 role=… grants=件数/字数 user=… memory=… kb=… omitted=… -->` を置く（D3）。
- `task_dispatch::cos_context` が ledger の生きている grant（案件 thread はその案件だけ、他は全案件）・KB の page・cos の org profile
  の knowledge の mount を読んで組む。ledger が読めなければ CoS run を起こさない。
- CoS run の `RunContext` を `task_dispatch::cos_context::cos_run_context` で全欄を書いて組むようにし、`launch.rs` の
  `..RunContext::default()` を消した。取り次いだ指示の組み立ても同じ module に移した（dispatcher module の行は減る）。

## 証拠（提出時の HEAD で実行）

- `cargo test -p task-worker --lib cos_standing`: 3 件合格（全 thread の種類の節と上限、空の入力、溢れの省略）。
- `cargo test -p task-dispatch --lib cos_standing`: 2 件合格（grant・KB・scope の読み取り、CoS の RunContext）。
- `cargo test -p task-worker --lib cos_chat`: 64 件合格（既存の CoS prompt の試験。Core が thread によらず同じことを含む）。
- 全体の検査（test-affected・clippy・fmt・progress-index）は下の「検査」に書く。

## 検査

- 件名「phase 1: CoS の全 run に常設の文脈…」の commit で `bash scripts/dev/test-affected.sh`: exit 100。nextest 5881 件中 5880 件合格、
  1 件失敗（`task-worker protocol::tests::committed_schema_matches_generated`。schema の生成物の更新漏れ）。repo-health は
  verdict=pass、worsened=0。
- 件名「phase 1: worker protocol の schema に cos_chat.standing を足す」の commit で schema を `UPDATE_SCHEMA=1` で作り直し、
  その試験だけを再実行して合格。test-affected 全体の再実行は wall の残りが足りず、続きの run で最終の HEAD で行う。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `sh scripts/dev/progress-index.sh --check`: exit 0。

## attempt 2: 受け入れ検査の fixture の隔離

- 前回 reviewer の `test-affected.sh` は 5881 件中 K4 の実物試験 1 件が 600 秒で timeout（exit 100）。
- 原因を持つ fixture 選択から、別 task の本番 workspace の絶対パスを探す fallback を撤去した。
  実物検証は `CAMPAIGN_K4_TGZ` の明示指定で実行し、指定したファイルが無い場合は失敗する。
  [ADR](../adr/2026-11-11-k4-explicit-test-archive.md) に試験契約の変更と維持する採点条件を記録した。
- 常設文脈 ADR の grant 節の上限を実装と同じ 3600 字へ訂正した。
- 差分検査は exit 100（5881 件中 5878 合格、起動前通知の既存3試験が Codex で失敗）。
  fixture が実アカウントの `CODEX_HOME` を継承していたため、`launch_notice_tests::adapters` で一時ディレクトリを明示した。
  通知の順序・回数・失敗時の期待値は変えていない。
- 実物 K4 を原票の明示指定でも実行したが、約5分で結果が出ず中断（exit 130）。実物の完走は未確認。
- D4 は未実装のまま。この修正だけで task 全体の完了とはしない。

## attempt 2 の最終検査（2026-11-11）

基準は `git merge-base HEAD main`。最後の同期の後、件名「phase 1: 起動前通知の Codex fixture を一時 home に隔離する」の
HEAD で実行した。検査後の変更はこの進捗の記録のみ。task 全体は未完了で、完了日はまだ記入しない。

- `bash scripts/dev/test-affected.sh`: exit 0。nextest 5881 件すべて合格、22 skipped、376.851 秒。
  差分対象の doc test も合格。repo-health pass、worsened=0、script bundle SKIP は 0 件。
- `cargo clippy --workspace -- -D warnings`: exit 0（vendor の deno_core の既存 deprecated warning は残る）。
- `cargo fmt --all -- --check`: exit 0。
- `sh scripts/dev/progress-index.sh --check`: exit 0。
- `cargo nextest run -p task-worker --lib -E 'test(launch_notice_tests)'`: exit 0、4 件合格。
- 実物 K4 の明示指定試験は exit 130（中断）。合格とは扱わない。

commit の件名:

- `phase 1: K4 の実物検証を明示した原票に限定する`
- `phase 1: 起動前通知の Codex fixture を一時 home に隔離する`
- `phase 1: 再検査の結果と未実装の委任経路を記録する`

次の run は、検査の再実行からではなく D4 の実装から始める。常設文脈はあるが、global grant を保存・使用する経路はまだ無い。

## attempt 3: D4 の実装（global の委任・grant に裏付けられた取り次ぎ）

ADR の D4 を実装に合わせて書き直した（分類は `job_wait_extend`、期限は 3660 日まで、API は `{id} = global`）。

- `task-core/src/authority.rs`: `ScopeKind::Global`、`GLOBAL_LEDGER = "*"`、分類 `ActionClass::JobWaitExtend`
  （`is_routine_answer`）、`MAX_ROUTINE_GRANT_DAYS = 3660`。`scope_covers` は global を全案件に広げる。
- migration 0123（`authority_grants` を作り直す。行を写し、追記専用の trigger と index を作り直す。CHECK で global の行は
  `project_id = '*'`・`scope_ref = '*'`・`job_wait_extend` だけ）。`SCHEMA_VERSION = 123`。
- `store/authority.rs`: 案件の判定は global の行も候補にする。global の範囲は定型の代答の分類だけ（他は 422）。
  `authority_grant_backs(_tx)`（取り次ぎを裏付けるか: 範囲・取り消し・期限・回数・元の根拠）。global の提案の通知は案件なし。
- `task-api/src/handlers/authority.rs`: `Ledger`（`{id}` が `global` なら全案件の台帳）。CoS の `authority.grant_propose`・
  `authority.check`・`authority.grant_revoke` も同じ。
- `chat.relay_forward`（`chat/window.rs`）: `grant_id` があり裏付けが成り立てば `task_ids`・`expires_at` を省ける。
  `relay.grant_id` に残し、`instructed_by` で使うたびに照合する（`cos/operations/instructions.rs`）。`grant_id` の無い
  取り次ぎは今のまま task と期限が必須。
- `job_wait_extend` の代答（`cos/basis/job_wait_extend.rs`）: basis の refs が生きた `job_wait_extend` の grant を挙げていれば、
  同じ job の上限到達 3 回の制限で人に回さない。applied になった後に `authority_audit` へ 1 行（`grant_id`、
  `human_decision_ref = authority_grant:<id>`、`actual_actor = cos:<thread>`）を書く（受信箱の resolve と `/cos/operations` の両方）。
- 常設の文脈（`task-dispatch/src/cos_context.rs`）は global の台帳の grant も入れる。統括の役割の節と `cos-inbox-triage` の §10・
  (g) の行を直し、「`expires_at` は 24 時間後」の書き方を消した（policy_version は上げていない。下の「本番への移行」）。
- API の文書（77k・forwards）と生成物（`docs/api/v1/api-v1.schema.json`・`web/api/generated/`）を更新した。

試験:

- `crates/task-api/tests/cos_job_wait_extend.rs` の
  `standing_job_wait_grant_from_the_general_thread_answers_the_project_wait_limit`（受け入れ条件 5 の scenario）: 統括 thread の
  人の発言 → 統括 CoS が `authority.grant_propose`（global）→ 人が受け入れ（global の production は 422）→ grant 無しの取り次ぎは
  422、grant 付きは task と期限なしで通る → 案件 CoS が 4 回目の上限の件を grant 無しでは 422 `cos_basis_extend_limit`、grant 付きで
  200・item answered・承認 once → `authority_audit` に grant id・`authority_grant:<id>`・`cos:<案件 thread>` が分かれて残る → 取り消すと
  裏付けが消える。`cargo nextest run -p task-api --test cos_job_wait_extend`: 4 件合格。
- `task-core` の `authority_ledger_scope_cover_is_narrow` に global の範囲を足した。

## attempt 3 の検査（提出時の HEAD で実行）

基準は `git merge-base HEAD main`。

- 1 回目の `bash scripts/dev/test-affected.sh` は repo health で exit 1（store・chat の module 行数、SqliteStore の method 数、
  `create_operation`・`resolve` の行数）。判定を `task_core::authority` の純関数（`grant_shape_problem`・`grant_backs`・
  `forward_gap`・`proposal_title`）へ移し、SqliteStore の method を足さず、操作の後始末を `goal_guard::settle_after` にまとめた。
- 件名「phase 1: 取り次ぎの許可の行の読み出しを store の free 関数にし、chat の行を減らす」の commit で
  `bash scripts/dev/test-affected.sh`: exit 100。nextest 8224 件中 8223 件合格、23 skipped、失敗 1 件は
  `task-api schema::tests::committed_schema_matches_generated`（`ChatForwardRequest` の `#[serde(default)]` を外した後の
  生成物の更新漏れ）。web 712 件合格。repo health は単独の `bash scripts/dev/check-health.sh` で verdict=pass。
- 件名「phase 1: API schema の生成物を作り直す」の commit で schema と web の生成型を作り直し、その試験を単独で再実行して合格。
  test-affected 全体（約 30 分）の再実行は run の壁時計が足りず、この run ではしていない。
- `cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check`・`sh scripts/dev/progress-index.sh --check`: 下の result に記録。

## attempt 4: main の取り込みと提出時の検査

- task branch を main（件名「phase 1: 消した config 欄・旧い退避規則・ブラウザ通知の文書の残りを直す」）へ rebase した。
  衝突は `crates/task-dispatch/src/lib.rs` の module 宣言だけで、`cos_context` と main の `cos_skill_gate` の両方を残した。
- 件名「phase 1: main の取り込みで重なった Codex fixture の CODEX_HOME の指定を 1 つにする」: main にも同じ fixture の隔離が
  入っていたため、attempt 2 の重複した `env` 欄（E0062）を消して main の版に合わせた。
- 件名「phase 1: 取り次ぎの判定 forward_gap を Result にし、main の取り込み後の chat の行を閾値に収める」: rebase の後、
  repo health の `size.responsibility-growth` が `crates/task-core/src/chat` の本体の行 8001（閾値 8000）で fail した。
  `task_core::authority::forward_gap` の戻り値を `Result<(), String>` にし、`chat/window.rs` の呼び出しを 3 行から 2 行にした
  （8000）。判定の中身と拒否の文言は変えていない。呼び出し元（`cos/operations/instructions.rs`）と scenario 試験の比較を合わせた。
- schema の生成物: `cargo nextest run -p task-api -p task-worker --lib -E 'test(committed_schema_matches_generated)'` は 2 件合格、
  `node web/scripts/gen-types.mjs` の差分は 0。

## attempt 4 の検査（提出時の HEAD で実行。基準は `git merge-base HEAD main`）

- `bash scripts/dev/test-affected.sh` → exit 0（1273 秒、開始時の load average 24）。nextest 8301 件合格・23 件 skip、
  web の型検査と unit 710 件合格、ts-verify 20 件合格、repo health は verdict=pass（fail_grade=0）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/progress-index.sh --check` → exit 0。
- review の検査の上限は 1200 秒で、この負荷の test-affected は 1273 秒かかった。review が時間切れになったら、負荷の低い時に
  再検査すれば足りる（コードの直しは要らない）。

## D4 の実装前 brief

1. 根本原因: ledger は案件・期限 366 日に限られ、取り次ぎは task と期限を必須にしていた。恒久の人の委任を表せず、job 待ちの
   4 回目以降は必ず人に回った。
2. owner: 型と判定は `task-core/src/authority.rs`・`store/authority.rs`、HTTP と CoS operation は `handlers/authority.rs`、
   取り次ぎは `chat/window.rs`、代答の条件は `cos/basis/job_wait_extend.rs`。
3. 再利用: `source_problem`（人の発言の確認）、提案の `question_key`、`authority_use_tx`（消費と監査を同じ transaction）、
   basis の `authority_grant` の ref。
4. 選択: 別の常設メモに権限を書かず ledger を広げる。分類 1 つで代答の種類を表す（`answer_kind` 欄は足さない）。
5. 新しい状態: global の範囲（`project_id = '*'`）、分類 `job_wait_extend`、`relay.grant_id`、`ChatForwardRequest.grant_id`。
6. 撤去: grant に裏付けられた取り次ぎへの一律の task・期限必須、skill の一律 24 時間の指示。
7. 不変条件: 人だけが grant を作る（既存の `authority_ledger_grant_is_human_only`）。使うたびに範囲・出典・取り消し・期限を照合する。
   判断の根拠と実行者を別に監査する（scenario 試験）。

## 初期の委任の種（D5。配備後に operator が提案として登録し、人が受け入れる）

本番には worker は登録しない。原文の message id は本番の `chat_messages` の `metadata_json.relay.source` から operator が引く。
本番の昇格（promote）の権限は含めない（`docs/ops/external-operator.md` の分担のまま）。

| # | 人の発言（日付・場所） | 提案する grant |
| --- | --- | --- |
| 1 | 2026-10-30 利用者（web）「job待ちに関しては基本認可するしかないのでそのようなケースも案件CoSで自動対応するようにしてください」 | global・分類 `job_wait_extend`（cluster の job 待ちの上限は延長する）・期限 3660 日・回数 1000。今の実装で登録できる |
| 2 | 2026-10-27 利用者（operator session、BenchFS の Sirius 実験）「何台落としてもいいので続けてください」 | BenchFS の案件・node 障害でも実験を続ける委任。対応する分類がまだ無い（提案 1）。分類ができるまでは登録しない |
| 3 | 10-30 前後の利用者「全部うまく対処してください、こっちが判断する必要があるもの以外」 | 定型の代答全般。分類ごとに分けて登録する（今は #1 だけ）。`human_required` は分類で解除できないまま |
| 4 | 利用者「衝突の統合は自動で」の趣旨の指示 | 衝突の統合の委任。分類がまだ無い（提案 1） |
| 5 | 利用者「計画の承認も基本自動化」の趣旨の指示 | 計画の承認（`plan_conforms`）の委任。分類がまだ無い（提案 1）。今も (f) の条件を満たす計画は CoS が承認できる |
| 6 | 本番の relay 33 件のうち残りの常設の指示（operator が原文で確かめる） | 原文ごとに、対応する分類があれば 1 件 |

手順（この task が main に入り、配備された後）:

1. operator は本番の `chat_messages` を読み取りで引き、各行の人の発言の message id を確かめる（統括 thread の人の発言。取り次がれた
   relay の写しは根拠にならないので `metadata_json.relay.source.message_id` の元の id を使う）。
2. operator は提案を登録する（operator の token。許可そのものは作れない）:
   `POST /api/v1/projects/global/authority/proposals`、本文
   `{"question_key":"standing:1","scope":{"kind":"global","ref":"*"},"action_class":"job_wait_extend","source":{"kind":"human_message","ref":"<message id>"},"expires_at":"<今から 3650 日後の RFC3339>","budget":1000,"note":"<人の発言の要約>"}`。
   受信箱に「統括 CoS が全案件に効く許可（job_wait_extend）を提案しました」の通知が 1 件出る。
3. 人が受け入れる: web には global の台帳の画面がまだ無い（未解決事項）。それまでは人の web token で
   `POST /api/v1/projects/global/authority/grants` に同じ本文と `"proposal_id":"<提案の id>"` を送る（人の token 以外は 403）。
   断るなら `POST /api/v1/projects/global/authority/proposals/<id>/dismiss`。
4. 確認: `GET /api/v1/projects/global/authority` の grant が `state = live`。次の CoS run の `runs/<run_id>/prompt.txt` の
   `<!-- celeris:cos-standing v1 … grants=N/…` の N が 1 増える。案件の job 待ちの上限の件を CoS が答えたら、
   `GET /api/v1/projects/<案件>/authority` の audit に `human_decision_ref = authority_grant:<id>` の行が出る。
5. 本番の昇格（promote）の権限は登録しない（`docs/ops/external-operator.md` の分担のまま）。

## 本番への移行

- config の欄の追加・削除は無い。配備の後、CoS run の prompt に常設の節が入る。KB に `cos/memory.md` が
  無い間は「（まだ無い）」と出る。人か curation が `cos/memory.md` を作ると入る。
- DB: migration 0123 が `authority_grants` を作り直す（本番は 0 行なので写す行は無い。行があっても写す）。schema 版 123 の
  release の後で 122 の release へ rollback すると、版の新しい DB を旧 binary が開けない点は他の migration と同じ。
- KB の skill: `cos-inbox-triage` の本文を変えた（§10 の取り次ぎ、(g) の行）。policy_version は "11" のまま（判定の
  API の契約は grant の無い代答では変わらない）。配備の後、KB の `skills/cos-inbox-triage/SKILL.md` を repo の
  `config/skills/cos-inbox-triage/SKILL.md` に差し替える（ADR 2026-11-11-cos-skill-sync の手順）。

## 未解決事項

- web の許可の画面は案件ごとで、global の台帳の画面が無い。人が global の提案を受け入れるには今は API を使う。
- 種 #2〜#5 に当たる分類（node 障害での継続・衝突の統合・計画の承認）は未実装。
- `cos/memory.md` の初期の本文は未作成（KB の変更は人の確認を通す）。
- 実物 K4 の明示指定試験（attempt 2）は完走を確かめていない。

## 提案

1. 定型の代答の分類を足す: `plan_conforms`（計画の承認）、衝突の統合、node 障害での実験の継続。足すたびに migration で
   CHECK を作り直し、basis の各 category の検査で grant の効果（何を人に回さなくなるか）を決める。
2. web の許可の画面に global の台帳（`/projects/global/authority`）を出し、人が画面で受け入れ・取り消しできるようにする。
- 統括 CoS の relay の期限切れ（4〜48 時間）で人に戻った件数を、D4 の配備の前後で `authority_audit` と受信箱の件数で比べる。
