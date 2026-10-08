---
task: 01M4CAKADGDTZA7QKDJ9GX35MW
title: 受信箱スレッドを「受信箱の件を CoS と話す場所」にする
status: done
started: 2026-10-07
completed: 2026-10-08
adr: agent-docs/adr/2026-10-07-cos-inbox-thread-conversation.md
---

# 受信箱スレッドを「受信箱の件を CoS と話す場所」にする

## 現在地

実装・試験・検査まで完了（run 1: commit a84e5c32・6088cd79。run 2（2026-10-08、差し戻し対応）: `instructed_by` の検査を
呼び出し元 run に結び付け、最終 SHA で test-parallel.sh を流した）。

## やったこと

1. ADR `agent-docs/adr/2026-10-07-cos-inbox-thread-conversation.md`（D1 digest・D2 退避文・D3 人の書き込みと文脈・D4 並び順・D5 web）。
2. D1: `crates/task-dispatch/src/dispatcher/cos_chat/digest.rs`。triage run の終端後に dispatcher が件ごとの判断を assistant 発言として
   決定的に残す（LLM 無し）。store に `cos_triage_run_report` / `cos_triage_open_items` / `cos_triage_item_report` /
   `cos_triage_runs_without_digest` / `chat_assistant_message_add_once` / `chat_message_get`。migration `0059_cos_inbox_item_summary.sql`
   （`cos_inbox_items.summary`、`run_id` の index）。SCHEMA_VERSION 59。
3. D2: `fallback.rs` の handoff 文に件の題名・決めること・選択肢・web_path。カードは答えられる `pending`。
4. D3: `CosChatContext.inbox_items`（protocol）・prompt の「受信箱の未解決の件」節（`task-worker/src/cos_chat.rs`）、
   `/cos/operations` の `instructed_by` と `inbox.answer`（`task-api/src/cos/operations.rs`）、skill §9。
5. D4: 人の発言の run が triage より先（既存の tick 順）。中断された triage run の件は digest 時に `pending` へ戻す。
6. D5: web の system 行の折りたたみ（`message-item.tsx`）と受信箱 thread の composer placeholder。
7. schema 再生成: `docs/api/v1/api-v1.schema.json`・`docs/protocol/worker-protocol.schema.json`・`web/api/generated/*`・`gui/app/celeris/types.ts`。
8. 差し戻し対応（2026-10-08、reviewer の基準 1・3）: `create_operation` の `instructed_by` 検査が「同じスレッドの role=user」だけで、
   一次対応 run・過去の無関係な発言・他スレッドの発言の id で human_required を外せた。`instructed_message`
   （`crates/task-api/src/cos/operations.rs`）に置き換え、(1) スレッドの message、(2) `role=user`、(3) スレッドが `kind=inbox`、
   (4) `chat_run_get(thread, CosCaller.run_id).input_message_id` と一致、を全て要求（違えば 422 `cos_instruction_invalid`、
   `cos_operations` に `rejected`）。prompt（`cos_chat.rs`）・skill §9・ADR D3 と付記・schema を合わせた。
   否定試験 `cos_chat_inbox_thread_instructed_by_refuses_messages_the_run_does_not_answer`（一次対応 run が過去の人の発言／自分の
   system 行を引く、人の run が過去の無関係な発言を引く、他スレッドの発言、会話スレッドの自分の入力、終端済み run の bearer）を追加し、
   肯定試験は受信箱スレッドで run を起こす形に直した。

## 証拠（コマンドと結果）

| 条件 | コマンド | 結果 |
|---|---|---|
| 0 判断が発言として残る・fallback に題名 | `cargo test -p task-dispatch --lib cos_chat_triage` | 23 passed（`cos_chat_triage_digest_records_each_judgment_with_an_answerable_card`、`cos_chat_triage_digest_without_reason_names_the_item_and_fallback_names_it_too` を含む） |
| 0 store の報告と digest の冪等 | `cargo test -p task-core --lib cos_chat_triage_store_report` | 1 passed |
| 1 人の書き込み → run、文脈、重なり | 同上 dispatcher 群（`cos_chat_triage_human_interrupt_requeues_items_and_runs_the_human_message_first`、`cos_chat_inbox_human_message_while_triage_runs_is_queued_then_served_before_new_items`） | passed |
| 1 prompt に未解決の件 | `cargo test -p task-worker --lib cos_chat_prompt_inbox` | 1 passed |
| 1 人の指示の中継（instructed_by・human_required・記録） | `cargo test -p task-api --test cos_triage instructed_by` | run 1: 1 passed / run 2: 2 passed（肯定 + 否定 5 経路） |
| 1 否定経路（一次対応 run・過去の発言・他スレッド・会話スレッド → 422 rejected） | `cargo test -p task-api --test cos_triage` | 13 passed（run 2） |
| 1 web の表示（折りたたみ・人待ちは通常・placeholder） | `pnpm -C web test` | 82 files / 598 tests passed（`chat_inbox_rows_*`・`chat_inbox_composer_*`） |
| 2 clippy | `cargo clippy --workspace -- -D warnings` | exit 0 |
| 2 web | `pnpm -C web typecheck` / `pnpm -C web lint` | exit 0 / exit 0（既存の styles.css の警告 4 件のみ） |
| 2 全体 | `bash scripts/dev/test-parallel.sh` | 下の「検査の結果」節 |

## 検査の結果

- `bash scripts/dev/test-parallel.sh`（1 回目、commit a84e5c32）: nextest 160 binaries、passed 4710 / failed 12 / ignored 14、exit 100。
  失敗 12 件は全て task-core の migration 試験が `assert_eq!(SCHEMA_VERSION, 58)`・期待版数一覧（`…57, 58`）を固定していたもの
  （`store::tests::migration_*` 9 件、`routing_log_tests`、`cron::store_tests`、`cluster_job::tests`、`delivery::tests::migration_0042_*`）。
  本変更の migration 0059 で版数が 59 になったことによる期待値の更新漏れで、機能の退行ではない。
- 修正後: `cargo test -p task-core --lib` → 866 passed / 1 failed（delivery の版数一覧）→ 一覧に 59 を足して
  `cargo test -p task-core --lib migration` → 25 passed、exit 0。test-parallel.sh の再実行は時間予算の都合で行っていない
  （失敗した 12 件以外の 4710 件は 1 回目で passed。再実行は次の統合段で確かめる）。
- `cargo clippy --workspace -- -D warnings`: exit 0（Finished）。
- `pnpm -C web typecheck` exit 0、`pnpm -C web lint` exit 0（既存 styles.css の警告 4 件）、`pnpm -C web test` 82 files / 598 tests passed。

### run 2（2026-10-08、差し戻し対応の最終 SHA）

- `bash scripts/dev/test-parallel.sh`（3 回。1 回目は commit 前の作業ツリー、2 回目は commit 75a8a407、3 回目は最終コード commit 5f286315）:
  1 回目 exit 0、passed 4723 / failed 0。2 回目 exit 100、passed 4722 / failed 1: task-core
  `cos_chat_triage_store_report_open_items_and_digest_bookkeeping`（run 1 で足した試験。同じ時刻で ingest した 2 件を `claim.item_ids[0]` で
  「a」と仮定していたが、ULID は同一 ms 内で乱数順。機能の退行ではなく試験の順序依存）→ source_key で引く形に直して commit 5f286315。
  3 回目（5f286315）exit 0、nextest 160 binaries、passed 4723 / failed 0 / ignored 14、doctest exit 0、tmp_leftovers 0
  （`CELERIS_TEST_SUMMARY {"passed": 4723, "failed": 0, "ignored": 14, "nextest_exit": 0, "doctest_exit": 0}`）。
  以後の commit はこの進捗ファイルの記述だけ（コードは 5f286315 と同一）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo clippy --workspace --all-targets -- -D warnings`（試験 code も含む）: exit 0（5f286315）。
- `cargo test -p task-api --test cos_triage`: 13 passed（`cos_chat_inbox_thread_instructed_by_relays_the_human_answer`・
  `cos_chat_inbox_thread_instructed_by_refuses_messages_the_run_does_not_answer` を含む）。`cargo test -p task-worker --lib cos_chat`: 42 passed。
- `UPDATE_SCHEMA=1 cargo test -p task-api --lib schema`: 3 passed（schema 再生成）。`node web/scripts/gen-types.mjs`・`pnpm -C gui gen:types`: 再生成。
- `pnpm -C web typecheck` exit 0、`pnpm -C web lint` exit 0（既存 styles.css の警告 4 件）、`pnpm -C web test` 82 files / 598 tests passed。

### delivery-repair（2026-10-08、run 01M4CESJNJBWPPQ5JQ7WNDA3YX）

- prepare.log の失敗は `cargo-fmt-check`（exit 1）。HEAD / main とも `186701fa7188` で、同じ
  `cargo fmt --all -- --check` をこの worktree でも実行して再現した。
- `cargo fmt --all` で受信箱機能に関係する Rust 10 ファイルを整形。機能・API・migration の変更はない。
  既定ブランチの追加取り込みは不要。ADR の決定も変更なし。
- 修正後の検証（この節の記録以外は以後変更なし）:

| コマンド | 結果 |
|---|---|
| `cargo fmt --all -- --check` | exit 0 |
| `cargo build -p celeris -p celerisctl` | exit 0（新しい scratch target の E2E 前提バイナリ） |
| `bash scripts/dev/test-parallel.sh` | exit 0、4723 passed / 0 failed / 14 ignored、160 binaries、doctest exit 0、tmp_leftovers 0 |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `pnpm -C web typecheck` | exit 0 |
| `pnpm -C web lint` | exit 0（既存 styles.css の警告 4 件） |
| `pnpm -C web test` | exit 0、82 files / 598 tests、server 77 tests passed |
| `git diff --check` | exit 0 |

- ログは task の成果物ディレクトリ
  `/local/celeris/data/workspaces/01M4CAKADGDTZA7QKDJ9GX35MW/artifacts/repair-*.log`。
  全体試験の最終実行は `repair-test-parallel-final.log`（nextest 104.2 秒、doctest 50.1 秒）。
  起動順を直す際に先行実行の出力が同じログの末尾に残ったため、元の `repair-test-parallel.log` を保存し、
  先頭の最終実行の開始から最初の `test-parallel: ok` までを別ファイルに抽出した。
- 本番サービス・DB・元のチェックアウトは変更していない。release prepare・デプロイは実行していない。
  今回確認したのは失敗した整形 gate と上記検査までで、release 全工程の再実行は配送側で行う。

## 表示の差し戻し対応（2026-10-08、attempt 2）

- reviewer が指摘した実際の `assistant` / `notice` / `observed` の digest を折りたたむ。
  人待ちや通知以外のカードを含む発言・通常の会話・配信中の返事は開いたままにし、本文とカードは展開して読める。
- 回答成功を `answered`（回答済み）、受信箱 API の 404 を `closed`（終了）としてバッジとカード属性へ反映。
  404 だけでは回答済みと失効を区別できないので断定しない。回答内容・閉じた理由は残し、操作ボタンは消す。
  再取得失敗後も query cache に古い項目が残るため、404 判定を cache より先にした。
- ADR D5 と実装との突き合わせを更新。Rust・schema・migration に今回の変更は無い。

| 検査 | 結果 |
|---|---|
| `pnpm -C web exec vitest run features/chat/cards/chat-card.test.tsx features/chat/messages/messages.test.tsx` | 58 passed。追加した回帰試験は修正前に 3 件失敗し、不備を再現 |
| `bash scripts/dev/test-parallel.sh` | exit 0、4723 passed / 0 failed / 14 ignored、160 binaries、tmp_leftovers 0 |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `cargo fmt --all -- --check` | exit 0 |
| `pnpm -C web typecheck` | exit 0 |
| `pnpm -C web test` | exit 0、82 files / 601 tests、server 77 tests passed |
| `pnpm -C web lint` | exit 0（既存 styles.css の警告 4 件） |
| `pnpm -C web check:boundaries` | exit 0 |
| `pnpm -C web e2e chat/cards.spec.ts` | exit 0、7 passed。決定・質問・認可・計画承認の回答直後と再読み込み後の閉じた表示、assistant 通知の開閉を確認 |
| `pnpm -C web mobile-audit --only /` | exit 0、360 / 390 / 412 / 1440 px |
| `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | exit 0 |
| `git diff --check` / `git merge-base --is-ancestor main HEAD` | exit 0 / exit 0（main は 186701fa、取り込み不要） |

- 追加したブラウザ試験でも上記 4 幅で横溢れ 0、summary の高さ 44px 以上、axe critical / serious 0 を確認。
  初回は既存 fixture と追加カードに同じボタンがあり locator が曖昧で失敗。対象カードの名前で絞って再実行し、全件成功。
- 証拠は `/local/celeris/data/workspaces/01M4CAKADGDTZA7QKDJ9GX35MW/artifacts/retry-*.log`。
  変更後の 4 幅の画像は同ディレクトリの `retry-screenshots/inbox-digest-<width>.png`。
  fake daemon・fake adapter とローカルの browser のみで検証。本番サービス・DB・元のチェックアウトの変更、デプロイ、実機 LLM の起動は行っていない。

## 判断文生成の差し戻し対応（2026-10-08、attempt 3）

- 起動時 reconcile 後に quota / login 等で起動前に失敗した run は、worker handle が無く終了検知されない。
  `start_thread` の失敗経路でも判断文生成を予約した。孤立 run の回収による終端も `control_hook` から予約する。
- `triage_ingest` が予約を先に消す処理を除き、`triage_digest` が空の走査に成功したときだけ消す。
  1 tick 20 run の上限を保ち、残件・走査失敗・判断文保存失敗を次の通常 tick で再試行する。
  新しい run・明示 reconcile・再起動を継続の条件にしない。冪等 key は既存のまま。
- 回帰試験を5件追加。起動失敗、41 run の回収（20 → 40 → 41）、一時DBの走査/保存失敗の復旧、
  起動時 reconcile 後の孤立 run 回収を固定した。時計注入・fake adapter・一時DBのみで、待ち時間や外部ネットワークに依存しない。
  起動失敗・残件・走査失敗・保存失敗の4件は修正前に失敗を確認。修正後は既存2件と合わせ7件成功。
- ADR D1・実装との突き合わせ・試験一覧を更新。元の prepare.log の fmt 失敗は `599b30e6` で修正済み。
  main は `186701fa` で、この branch の祖先のため取り込みは不要。

| 検査 | 結果 |
|---|---|
| `cargo test -p task-dispatch --lib cos_chat_triage_digest_` | exit 0、7 passed（新規5件 + 既存2件） |
| `cargo build -p celeris -p celerisctl` | exit 0（新しい scratch target の E2E 前提バイナリ） |
| `bash scripts/dev/test-parallel.sh` | exit 0、4728 passed / 0 failed / 14 ignored、160 binaries、doctest exit 0、tmp_leftovers 0 |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `cargo fmt --all -- --check` | exit 0 |
| `pnpm -C web typecheck` | exit 0 |
| `pnpm -C web test` | exit 0、82 files / 601 tests、server 77 tests passed |
| `pnpm -C web lint` | exit 0（既存 styles.css の警告4件） |
| `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | exit 0 |
| `git diff --check` / `git merge-base --is-ancestor main HEAD` | exit 0 / exit 0 |

- ログ: `/local/celeris/data/workspaces/01M4CAKADGDTZA7QKDJ9GX35MW/artifacts/retry3-*.log`。
  全体試験は `retry3-test-parallel.log`、回帰試験は `retry3-regression-after.log`。
- 本番サービス・DB・元のチェックアウトを変更せず、この task branch のみで修正・検証した。
  release prepare・デプロイ・実機 LLM の起動は行っていない。

## 未解決事項

- digest は run の終端を観測した tick で書く。終端と tick の間に daemon が落ちた場合は再起動時の reconcile で書かれる（件を持つ run だけ）。
- 導入前から残っている終端済み triage run にも、初回起動で 1 度だけ digest が付く（件を持つ run に限る。以後は増えない）。
- `instructed_by` の「どの件か曖昧なら聞き返す」は skill と prompt の指示であり、API では強制しない（判断は CoS、検証は API の境界）。
  API が強制するのは「その run の入力になった人の発言である」こと（run 2）。発言をどの件への回答と読むかは CoS の判断で、誤読は領域 API の
  検証（選択肢の妥当性・revision）と監査（payload の `instructed_by`、reason の「人の指示（seq n）」）で追える。
- 本番の実機（LLM）での確認は未実施（この run は fake adapter と単体・結合試験のみ）。

## 提案

- `ChatCard` に元の待ちの画面（`web_path`）とは別に「受信箱の項目 id」を持たせると、web が item の状態を source_key 以外でも引ける。
- 受信箱スレッドの digest を受信箱画面（`/inbox`）からも参照できるようにする（件 → 判断の発言へのリンク）。
