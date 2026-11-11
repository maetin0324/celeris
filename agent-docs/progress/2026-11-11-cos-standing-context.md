---
title: CoS の常設の文脈（任された範囲・利用者・覚え書き・KB の入口）と人の委任の grant 化
tasks: [01M75722QN630BBASEQ759KG4T]
status: running
updated: 2026-11-11
---
# CoS の常設の文脈（任された範囲・利用者・覚え書き・KB の入口）と人の委任の grant 化

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

## 残り（続きの run でする）

1. D4: `ScopeKind::Global`（`project_id = '*'`）と分類 `routine_answer`（`answer_kind` 欄、期限・回数を省ける）。migration で
   `authority_grants` の CHECK を作り直す。global の API（`/api/v1/authority…`）。
2. D4: `chat.relay_forward` に `grant_id`。生きた grant が案件を含めば `task_ids`・`expires_at` を要らないとする（window.rs）。
3. D4: Core と cos-operator・cos-inbox-triage の skill から「`expires_at` は 24 時間後」「1 task に絞る」書き方を消し、常設の指示は
   `authority.grant_propose` にする（policy_version を上げる）。
4. scenario 試験: 統括で「job待ちは常に延長」→ CoS が提案 → 人が受け入れ → 案件 CoS が job 待ちの上限の件を grant id を引いて答え、
   人に回さない。grant に裏付けられた取り次ぎは task の指定と期限なしで通る。

## D4 の実装前 brief（未着手）

1. 根本原因: ledger は案件・期限・回数に限定され、取り次ぎは task と期限を必須にしている。恒久の人の委任を表現できない。
2. owner: 型と失効判定は `task-core/src/authority.rs`、保存・出典照合・使用の監査は `store/authority.rs`、
   HTTP と CoS operation は `task-api/src/handlers/authority.rs`・`cos/ops/projects.rs`、取り次ぎは `chat/window.rs`。
3. 再利用: `source_problem` の人の出典確認、提案の question_key による重複防止、grant の revoke、
   `authority_use_tx` の使用と監査の transaction、`cos/basis.rs` の authority grant 根拠。
4. 選択: 別の常設メモへ権限を書かず、D4 の ledger 拡張を採る。global と案件の grant を同じ検証経路で扱う。
5. 新しい状態: global scope、routine_answer の answer_kind、任意の期限・回数、relay の grant_id。
   無期限を遠い未来の日付や巨大な回数で代用しない。一般の危険操作の上限は維持する。
6. 撤去: grant の裏付けがある取り次ぎへの一律 task・期限必須、Core/skill の一律 24 時間指示。
7. 不変条件: 人だけが許可を作る。使用の直前にも範囲・出典・取消・期限を確認し、判断根拠と実行者を別々に監査する。
   scenario は統括の原文→提案→人の許可→案件の job 待ち代答を実 API/transaction で通す。
   取消・他案件・異なる answer_kind・人以外の許可・grant 無しの期限欠落は拒否する。

## 初期の委任の種（D5。配備後に operator が提案として登録し、人が受け入れる）

本番には worker は登録しない。原文の message id は本番の `chat_messages` の `metadata_json.relay.source` から operator が引く。
本番の昇格（promote）の権限は含めない（`docs/ops/external-operator.md` の分担のまま）。

| # | 人の発言（日付・場所） | 提案する grant |
| --- | --- | --- |
| 1 | 2026-10-30 利用者（web）「job待ちに関しては基本認可するしかないのでそのようなケースも案件CoSで自動対応するようにしてください」 | global・`routine_answer`・`answer_kind = job_wait_extend`（cluster の job 待ちの上限は延長する）・期限なし |
| 2 | 2026-10-27 利用者（operator session、BenchFS の Sirius 実験）「何台落としてもいいので続けてください」 | BenchFS の案件・`routine_answer`・`answer_kind = cluster_node_failure_continue`（node が落ちても実験を続ける）・期限なし |
| 3 | 10-30 前後の利用者「全部うまく対処してください、こっちが判断する必要があるもの以外」 | global・`routine_answer`（triage の定型の代答全般。`human_required` は除く。分類で解除できないのは今のまま） |
| 4 | 利用者「衝突の統合は自動で」の趣旨の指示 | global・`routine_answer`・`answer_kind = integrate_conflict` |
| 5 | 利用者「計画の承認も基本自動化」の趣旨の指示 | global・`routine_answer`・`answer_kind = plan_conforms`（skill の (f) の条件はそのまま） |
| 6 | 本番の relay 33 件のうち残りの常設の指示（operator が原文で確かめる） | 原文ごとに 1 件 |

手順（D4 の配備の後）:

1. operator は本番の `chat_messages` を読み取りで引き、上の各行の人の発言の message id を確かめる。
2. operator は提案を登録する: `POST /api/v1/authority/proposals`（global）か `POST /api/v1/projects/<id>/authority/proposals`、
   本文は `{"question_key":"standing:<#>","scope":{…},"action_class":"routine_answer","answer_kind":"…","source":{"kind":"human_message","ref":"<message id>"},"note":"<人の発言の要約>"}`。
3. 人は受信箱の提案の件を画面で開き、内容を読んで「許可する」を押す（`POST …/authority/grants` に `proposal_id`）。断るなら「断る」。
4. 確認: `GET /api/v1/authority` の grant が `state = live` で、次の CoS run の `runs/<run_id>/prompt.txt` の
   `<!-- celeris:cos-standing v1 … grants=N/…` の N が増えている。

## 本番への移行

- config の欄の追加・削除は無い（この run の時点）。配備の後、CoS run の prompt に常設の節が入る。KB に `cos/memory.md` が
  無い間は「（まだ無い）」と出る。人か curation が `cos/memory.md` を作ると入る。

## 未解決事項

- D4 の実装（上の「残り」）。
- `cos/memory.md` の初期の本文は未作成（KB の変更は人の確認を通す）。

## 提案

- 統括 CoS の relay の期限切れ（4〜48 時間）で人に戻った件数を、D4 の配備の前後で `authority_audit` と受信箱の件数で比べる。
