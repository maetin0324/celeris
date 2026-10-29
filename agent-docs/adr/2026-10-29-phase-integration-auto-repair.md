# ADR 2026-10-29: 段の統合の衝突は人を待たずに merge の repair WU が解き、尽きたときだけ人に回す

- 状態: 実装済み（2026-10-29）
- tasks: [01M6320TM5XCF2QA8XXTT57BRE]
- 関係: ADR 2026-10-02-parallel-integration-auto-resolve（D3 人に回す条件・D4 統合の依頼）、ADR 2026-10-28-cos-integrate-task
  （delivery の依頼の integrate task）、ADR-0074 D1.4（段の統合と merge の repair WU）、ADR 2026-10-22-stalled-auto-replan

## 問題

人の指示（統括 CoS スレッド seq 4066）:「衝突の統合は自動で判断して統合するようにしてください、基本的に人ではいらないはずです」。

段の統合（子 task・WU の branch を親 task の作業 branch に取り込む。依頼の origin は `phase:<統合 WU の key>`）で
auto_resolve が解けない衝突（`NeedsHuman`）は、統合の依頼を記録して統合 WU を `blocked(question)` にし、人の回答を待っていた。
integrate task（ADR 2026-10-28-cos-integrate-task）は origin `delivery` だけが対象で、段の依頼は 422
`integrate_task_unsupported_origin`。受信箱の選択肢は「統合した／見送る／再試行する」だけで、人が手で merge するまで親 task が止まる。
実例: task 01M62S8XWCZDCY2ZBVQ35176MH の段 fix の統合 WU `integrate-fix`（依頼 01M62ZEN0WPZ0YY64N8YGFGPPF、`crates/campaign/src/lib.rs`・
`main.rs` の内容衝突。両側とも subcommand・module の追加だけ）。

段の統合には ADR-0074 D1.4 の merge の repair WU（`merge-<phase>-<unit>`、親 task の worktree で走る）が既にあり、2026-10-03 に
「内容衝突は人へ依頼し、修復 run を自動投入しない」として使われなくなっていた。

## 決定

### D1. 段の統合の衝突は、まず merge の repair WU に解かせる

`on_integration_finished` は衝突（auto_resolve の `NeedsHuman` を含む）を統合の依頼にせず、`schedule_merge_repair` に渡す。

- repair WU の objective は従来の内容（衝突 file・両側の WU の目的と決定・`git diff --stat`）に、auto_resolve の依頼の markdown
  （両側の commit・理由）と「両側の意図を比べ、両方の変更を保って解く。片側を捨てない」を足す。
- repair WU の検査は `git merge-base --is-ancestor <子の branch> HEAD`。repair が done になると統合 WU が続きから再開し
  （済んだ merge は飛ばす）、段の統合の検査（その段の WU の checks と `workspace.toml` の check）を流す。検査の失敗は従来どおり
  統合後の検査の repair か replan に回る。
- 親 task の最終 review（決定的な検査と reviewer）が統合後の branch を判定する。worker の自己申告では done にならない。
- 人の回答は待たない。delivery の依頼の integrate task と違い、新しい root task は起こさない（段の統合は親 task の branch の上で
  閉じ、親の最終 review と配送が main への取り込みを決める）。

### D2. 自動の解消が尽きたときだけ人に回す

- 上限は衝突した WU ごとに 2 回（`MAX_MERGE_REPAIRS_PER_UNIT`。同じ計画の `merge-<phase>-<unit>` と `merge-<phase>-<unit>-<n>`
  を状態を問わず数える）。従来の段ごとの上限（`MAX_MERGE_REPAIRS_PER_PHASE`）を置き換えた。
- merge の repair WU が失敗した（再試行を使い切った・進まない・plan_issue）ら、replan せずにその WU を `cancelled`
  （`WorkUnitTransitioned.reason = merge_repair_failed`）にし、統合 WU を `pending` に戻して task を `Continue{advance}` にする。
  やり直した統合が同じ衝突に当たると次の repair WU を作る。
- 上限に達した衝突だけを統合の依頼として記録し、統合 WU を `blocked(question)` にする（従来の依頼と同じ受信箱の 1 項目）。
  依頼の `reason` は「自動の解消が失敗: phase <phase> の統合で WorkUnit <unit> の merge が衝突し、merge の repair WU（<key> 失敗、…）を
  上限の 2 回走らせても衝突が残った。auto_resolve の判定: <元の理由>」で、受信箱の本文の「理由」に出る。
- 統合後の検査の失敗・git の失敗で統合を諦める経路（`integration_gives_up`）は変えない。受信箱の質問の本文に理由が出る。

### D3. 既存の未回答の段の依頼は sweep が自動の解消へ戻す

`task_ops::integrate_task::phase::retry_unrepaired_requests` が stranded sweep（起動直後と 300 秒ごと、single-flight、Active の
instance だけ）の中で、integrate task の決着の後に走る。LLM は呼ばない。

- 対象: origin `phase:*` の未回答の依頼で、task が `blocked`、その統合 WU が `blocked(question|continuation_cap)`、かつ同じ段に
  merge の repair WU が 1 件も無いもの（この ADR の前に記録された依頼。D2 で記録された依頼は必ず repair WU を持つので対象外）。
- 依頼を `IntegrationAnswered { answer: "retry", by: "daemon" }` で閉じ、統合 WU を `pending` に戻して task を回答（`Trigger::Answer`）で
  再開する。受信箱の「再試行する」と同じ処理（`blocked_integration_unit`・`resume_integration_unit` を共有する）。
  やり直した統合は D1 で repair WU に解かせる。
- 依頼 01M62ZEN0WPZ0YY64N8YGFGPPF も、この ADR を含む release に昇格すれば最初の sweep で対象になる。人の操作は要らない。

### D4. delivery の依頼は既定で自動起票しない（CoS の一次対応が integrate task で答える）

delivery の依頼（done 後に main へ取り込む段の衝突）の integrate task は、daemon が自動で起票しない。

- 理由: integrate task は通常の root task（担当・lane の決定、案件・リポジトリの継承、worker と reviewer の費用）で、起票は
  task-api の作成経路（`create_task_linked_op`）と主体の記録を通す。daemon に task の起票を持たせない。
- 代わりに、受信箱の推奨が `integrate_task` で、CoS の一次対応（cos-inbox-triage）が `documented_default` の根拠で人を待たずに
  答える（ADR 2026-10-28-cos-routine-delegated-answers）。人の回答は要らない。
- 段の依頼には integrate task を開かない（422 `integrate_task_unsupported_origin` のまま）。段の衝突は D1 の repair WU が解き、
  尽きた依頼は人が決める。

## 消したもの

- 段の統合で `NeedsHuman` を即座に統合の依頼にする分岐（`on_integration_finished`）と、それを固定していた試験の期待。
- 段ごとの merge repair の上限 `MAX_MERGE_REPAIRS_PER_PHASE`（衝突した WU ごとの上限に置き換えた）。
- merge の repair WU の失敗を通常の WU の失敗として replan に回す挙動（D2 で統合のやり直しに置き換えた）。
- 受信箱の回答 API が持っていた、段の依頼の統合 WU の特定と再開の実装（task-ops の共有関数へ移した）。

## 本番への影響

- config 欄・migration・event の追加は無い（`IntegrationAnswered` の `retry` と `WorkUnitTransitioned` の新しい reason だけ）。
- skill の変更（`config/skills/cos-inbox-triage`）は本番 KB の `skills/` に人が取り込むまで CoS に届かない。policy_version は 6 のまま。
- 配備（release・promote）は人が行う。昇格後の最初の stranded sweep で、自動の解消を経ていない段の依頼が再試行に回る。

## Consequences

- 段の統合の衝突は、worker の repair WU・段の検査・親の最終 review を経て親 branch に入り、人の回答を待たない。
- 人に回るのは、repair WU が 2 回とも解けなかった衝突（理由付きの統合の依頼）と、統合後の検査・git の失敗（理由付きの質問）だけ。
- repair WU の分だけ worker の費用がかかる。衝突した WU ごとに 2 回までなので連鎖しない。
