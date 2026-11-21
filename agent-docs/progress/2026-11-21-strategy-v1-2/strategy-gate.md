---
title: 戦略層 v1 子 2 strategy-gate — CoS の task.create の任意の欄 strategy を検査し、未承認・stale・別案件・出典欠損を 422 で拒否する
tasks: [01M7XQ9Q9EB3CH59TSNYM9HBC8]
status: done
updated: 2026-11-21
completed: 2026-11-21
---
# strategy-gate（WorkUnit。子 2 の段 wire）

完了日: 2026-11-21。設計は [ADR 2026-11-21-strategy-evidence-contract](../../adr/2026-11-21-strategy-evidence-contract.md) の D3・D4・D6。
commit は次の 4 件と、この進捗の commit。

- 「strategy-v1 2 strategy-gate: CoS の task.create の任意の欄 strategy を検査し、未承認・stale・別案件・出典欠損を 422 で拒否する」
- 「strategy-v1 2 strategy-gate: API の schema と web の生成型、CoS の operations の説明に strategy の欄を足す」
- 「strategy-v1 2 strategy-gate: strategy の監査・hash・検査を OperationBody の関数に移し、create_operation を base の行数以下に戻す」
- 「strategy-v1 2 strategy-gate: 範囲外の 3 file を base に戻し、試験の API の状態を試験の module で組む」

経緯: 最初の提出（attempt 1 の前の run）は計画の範囲の check で落ちた。範囲の外の `config/skills/cos-operator/operations.md`
（CoS の説明）・`crates/task-api/src/handlers.rs`・`crates/task-api/src/handlers/tests.rs`（試験の部品 `state` を `pub(crate)` にした）を
変えていた。3 file を base の内容に戻し、試験の API の状態は `cos/strategy_gate/tests.rs` の `api_state` で組むようにした。
CoS の説明は下の「提案」に回した。

## 開始時の確認（本番の release）

- `curl -s http://127.0.0.1:7710/api/v1/health` の `release` 欄（release の commit の先頭 12 桁。2026-11-21T16:34Z に読んだ。
  この file には hash を書かない）を `R` とする。
- 子 1 の統合 commit「integrate wu/gate (phase close)」について `git merge-base --is-ancestor <その commit> R` は exit 0。
  子 1 の修正は本番で動いている。

## 実装前 brief

1. 根本原因: 実行の入口（CoS の `task.create`）が、戦略の文書の承認・鮮度・案件・出典を確かめない。task-core の検査（contract-core）は
   純関数で、store から文脈を集めて呼ぶ口が無かった。
2. 責務 owner: `/cos/operations` の要求の検査（`crates/task-api/src/cos/operations.rs` の `create_operation`）。文脈の収集は
   `goal_guard` と同じ層の新しい module `crates/task-api/src/cos/strategy_gate.rs`。規則そのものは task-core の `strategy::validate_document`。
3. 既存の再利用先: `ArtifactProduced`（`events_for`）・`files::task_local_dir`・`canonical_workspace`・`resolve_artifact`、
   `decisions_list` と `DecisionAnswered`、`project_get` と `project_proposal::sha256_hex`、`roadmap_approved`、`repo_list`、
   `chat_message_get`・`chat_thread_get`、`task_ops::knowledge::read_page` と `knowledge::front_matter`、`audit.reject`（rejected の監査）。
4. 選択肢: (a) `NewTaskBody` に欄を足す → ADR D4 が退けた（REST の本文・派生の作成も使い、保存に migration が要る）。
   (b) `/cos/operations` の要求に `instructed_by`・`basis` と同じ層の欄を足す → 採用（ADR D4）。
5. 新しい状態・分岐・fallback・port: 要求の任意の欄 `strategy` 1 つ。欄が無ければ検査を通らない（従来の経路）。fallback・切替の flag・
   DB 表・migration・event の種類・store の関数は無し。
6. 撤去対象: 無し（新しい欄と検査を足すだけ。置き換える旧い経路は無い）。
7. 不変条件: 下の「試験」の各項目。

## 足したもの

- `crates/task-api/src/cos/strategy_gate.rs`: `StrategyRef {task_id, name, sha256}`（`deny_unknown_fields`）と `check`。
  `task.create` 以外の操作に欄があれば 422 `strategy_not_applicable`。欄があれば次を store と workspace から読み（書かない）、
  `validate_document` を `Intent::Execute` で呼ぶ。
  - 作る task の案件（`request.body.project_id`）。無い・引けない → `strategy_other_project`。
  - 文書を作った task と、その `ArtifactProduced` のうち name・sha256 が一致するもの。無い → `strategy_source_missing`。
    task の案件が作る task と違う → `strategy_other_project`。
  - 成果物の file（workspace の外は読まない）。無い、または今の byte 列の sha256 が記録と違う（上書き）→ `strategy_source_missing`。
  - 今の `Project.request` の sha256、承認済み Roadmap の版、同じ name の成果物の sha256 の全件。
  - 版の鎖（`supersedes` を最大 64 回辿る。各版の file の sha256 も照合）。
  - 承認: 文書を作った task の key `strategy-approve-v<版>` の決定と、最後の `DecisionAnswered{option, by}`。人は `human:<名前>` だけ
    （`task_core::is_human_principal`）。同じ案件の同じ `strategy_id` の人の承認から、最新の承認済みの版と同じ版の別の sha256 を集める。
  - 参照（`url` を除く）: commit は案件に登録された repo 名、task・artifact は task の案件、chat は message があり thread の案件、
    knowledge は page があり front matter の scope（`project:<slug または id>` はその案件、他の scope は案件なし）。
  - 拒否は 422。`code` は最初の違反の規則の code、`rules`・`codes`・`violations` に全件。`audit.reject` が rejected の監査を残す。
- `crates/task-api/src/cos/operations.rs`: `OperationBody.strategy`（任意）。`OperationBody` の関数 `audit_payload`（payload に `strategy` を入れる）・
  `hash`（`strategy` があるときだけ body の後に `\nstrategy\n<canonical strategy>` を hash に足す。無ければ従来の `request_hash` と同じ値）・
  `check_strategy`（`goal_guard::check` の後、`authority_gate` の前。`instructed_by` の有無によらず呼ぶ）。
- 生成物: `UPDATE_SCHEMA=1 cargo test -p task-api --lib committed_schema_matches_generated` で `docs/api/v1/api-v1.schema.json`、
  `cd web && node scripts/gen-types.mjs` で `web/api/generated/schema.json`・`types.ts`（`OperationBody.strategy?: StrategyRef | null` と `StrategyRef`）。
- 試験の足場: `cos/strategy_gate/tests.rs` の `api_state`（`ApiSettings` を組んで `ApiState::new`）。`handlers/tests.rs` の `state` と同じ形の
  設定の写し（約 110 行）で、試験だけの重複になる。`handlers/` はこの WorkUnit の範囲の外なので共通化しなかった（「提案」2）。

`crates/task-api/src/cos/operations.rs` は高 risk の path class `authz` に入る。ADR の割り当てどおりで、final review の設計の判定の対象になる。
`create_operation` は base で 202 行、HEAD で 200 行（repo health の `size.responsibility-growth` の閾値 200。最初の版は 215 行で落ちたので、
strategy の処理を `OperationBody` の関数に移した）。

## 試験（lib の unit 試験 8 件。すべて `strategy_gate_` で始まる）

守る不変条件: 人（`human:<名前>`）が承認した、最新の、同じ案件の、出典のそろった文書だけが task を作る。それ以外は 422 と規則の code で、
task を作らない。欄の無い要求は今と同じ。task-core の `strategy_contract_` は純関数だけを見るので、store から文脈を集める経路と、
入口で task が作られないことは検出できない（この試験が見る）。各試験は `/api/v1/cos/operations` に CoS の run の credential で送り、
422 と code に加えて、task の行が増えないことを確かめる。

| 試験 | 示すこと |
|---|---|
| `strategy_gate_human_approved_latest_document_creates_the_task` | 人の承認の最新の文書は 200 で task を作り、payload に strategy が残る。同じ key の再送は同じ操作、別の strategy で同じ key は 409 |
| `strategy_gate_unapproved_discussion_and_proposed_are_422` | discussion の文書（人が approve と答えても）、決定の無い proposed、答えの無い決定、人の却下 → `strategy_unapproved` |
| `strategy_gate_cos_self_approval_is_422` | 文書に `decision_state: approved` と書いた → `strategy_invalid`（規則 `state_not_writable`）。`cos:<thread>`・`operator:claude-code`・`mcp:chatgpt`・接頭辞の無い `human` の approve → `strategy_unapproved` |
| `strategy_gate_stale_revision_is_422` | Roadmap の版が前提と違う、より新しい版（v2、v1 を `supersedes`）が人に承認された、文書の後に Project.request が変わった → `strategy_stale`。`instructed_by`（人の発言の中継）でも同じ。承認済みの v2 は通る |
| `strategy_gate_other_project_material_is_422` | 証拠が別の案件の thread の message を指す、作る task が別の案件、文書の `project_id` が別の案件 → `strategy_other_project` |
| `strategy_gate_missing_source_is_422` | 証拠の成果物が無い、参照した文書の成果物が無い、証拠の参照が空、file が記録の後に上書きされた → `strategy_source_missing` |
| `strategy_gate_absent_field_keeps_task_create_unchanged` | 欄の無い task.create は 200、payload に strategy が無く、`request_hash` は従来の式の値。`comment.create` に付けると 422 `strategy_not_applicable` |
| `strategy_gate_ablation_four_rules` | 下の ablation の表 |

### ablation（task.create の経路。`cargo test -p task-api --lib strategy_gate_ablation -- --nocapture`）

基準 B（人が承認・最新・同じ案件・出典あり）から 1 か所だけ変えた負の状態を、それぞれ `/cos/operations` の `task.create` に送った。
各負の状態は、その規則の code だけで拒否される（`codes` がちょうど 1 件）。B だけが task を作る（作った task は 1 件）。

| 規則 | 負の状態（B からの変更） | status | codes |
|---|---|---|---|
| R-src | N-src: `evidence[0]` の artifact の参照が存在しない name | 422 | `["strategy_source_missing"]` |
| R-stale | N-stale: `request_sha256` が前の依頼の sha256 | 422 | `["strategy_stale"]` |
| R-proj | N-proj: `evidence[0]` の chat の参照が別の案件の thread の message | 422 | `["strategy_other_project"]` |
| R-appr | N-appr: 承認の決定の最後の回答の `by` が `operator:claude-code` | 422 | `["strategy_unapproved"]` |
| 正の例 | B: 人（`human:rmaeda`）の approve | 200 | `[]` |

規則を 1 つずつ外した ablation（その規則を外すと負の状態が通る）は contract-core の `strategy_contract_ablation` が純関数で示した。

### 負の試験が production の変更だけを戻した tree で落ちること

試験の file はそのままにして、production の変更だけを 2 通りに戻して走らせた（提出時の HEAD の試験で実行。記録は run の artifacts の
`revert-check.log`・`revert-gate-call.log`）。走らせた後に HEAD の `operations.rs` を写し戻し、
`git diff --quiet HEAD -- crates/task-api/src/cos/operations.rs` が exit 0 であることを確かめた。

1. `git show $CELERIS_WU_BASE:crates/task-api/src/cos/operations.rs > crates/task-api/src/cos/operations.rs` の後に
   `cargo test -p task-api --lib strategy_gate`: exit 101、0 passed・8 failed。全件が 400 `bad_request`（`unknown field strategy`）で、期待の 422 と code にならない。
2. 欄 `strategy` は残し、`create_operation` から検査の呼び出しの 1 行
   `req.check_strategy(store, &env, &audit, matched.as_ref())?;` だけを `sed -i` で消した tree で同じ command: exit 101、1 passed・7 failed。
   7 件の負の試験は 422 を期待したところで 200 が返り、task が作られて落ちた（負の試験が見ているのは検査の呼び出しそのもの）。
   通った 1 件は正の例の試験（人の承認の文書は検査があっても無くても通る）。

## 行数（`git diff --numstat $CELERIS_WU_BASE`。空行・注釈を含む）

| 区分 | file | 追加・削除 |
|---|---|---|
| production Rust | `cos/strategy_gate.rs`（`#[cfg(test)] mod tests;` の 2 行を除く）479・`cos/operations.rs` +78/−6・`cos/mod.rs` +1 | +558/−6（純増 552） |
| test Rust | `cos/strategy_gate/tests.rs` 828（うち `api_state` と token の定数で約 115） | +828 |
| 生成物 | `docs/api/v1/api-v1.schema.json` +33・`web/api/generated/schema.json` +33・`types.ts` +7 | +73 |

純増の理由: 新しい入口の検査で、消す旧い経路は無い。`strategy_gate.rs` の大半は store の 9 種の照会（成果物・file・鎖・決定・案件・Roadmap・
参照 5 種）で、規則は task-core の 1 か所にあり二重に実装していない。

## 検査（提出時の HEAD で実行）

- `cargo test -p task-api --lib strategy_gate`: exit 0、8 passed・0 failed。
- `bash scripts/dev/test-affected.sh`: exit 0。repo-health verdict=pass（fail_grade 0）、nextest 6187 tests run: 6187 passed・28 skipped、
  web の tsc -b と unit（101 files・705 tests passed）。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- `git diff --quiet $CELERIS_WU_BASE HEAD -- crates/task-core/migrations crates/task-core/src/model/events.rs crates/task-core/src/store`: exit 0（差分なし）。
- 計画の範囲の check（`sh "$CELERIS_WU_SCOPE_PATHS"` の path を許す接頭辞で絞る command）: exit 0。変えた path は上の表の 7 file とこの進捗だけ。

## 親の目的への寄与

- 確認済み: 未承認・古い・別案件・出典の欠けた戦略、CoS・operator・mcp が承認した戦略、CoS が承認済みと書いた戦略は、実行の入口
  （CoS の `task.create`）で 422 になり task を作らない（試験 7 件と ablation）。人（`human:<名前>`）の承認の最新の版だけが task を作る。
  承認・鮮度・案件の正本は既存の決定・Project.request・Roadmap・task で、新しい表・event の種類・状態機械は足していない。
  改善 A・C の「承認された戦略だけを実行する」入口の条件になる。
- 推定: 子 5・6 が戦略の文書を作り版を上げるとき、pivot した新しい版の承認が古い版の実行を止める（`strategy_stale`）。shadow のケースでの確認は子 6。
- 未確認: 実運用で CoS が `strategy` を付けて起票するか。CoS の skill の説明（`config/skills/cos-operator/operations.md`）には欄をまだ書いていない
  （範囲の外。「提案」1）。本番の run では未観測。改善 B・D と observability への効果は
  子 3〜7 が文書を作り読むまで測れない。

## 未解決事項

- `knowledge` と `commit` の参照の照合は実装したが、route の試験は通していない（試験の state は KB の root を持たず、案件に repo を登録していない）。
  純関数の側の扱いは contract-core の試験が見ている。
- 承認の決定は、文書を作った task の run が `result.json` の `decisions` で出す（ADR D2.2）。worker の決定の key・問いの形を作る口は、子 5 が文書を作るときに使う。
- 人は Web の受信箱か人の token で答える必要がある。operator が人の代わりに答える運用は戦略の承認に使えない（ADR の未確認の項目と同じ）。

## 提案

1. CoS の skill の説明 `config/skills/cos-operator/operations.md` の起票の節に、`strategy` の欄（`{"task_id","name","sha256"}`）、
   拒否の code（`strategy_unapproved`・`strategy_stale`・`strategy_other_project`・`strategy_source_missing`・`strategy_invalid`・
   `strategy_refute_removed`・`strategy_idempotency_conflict`・`strategy_not_applicable`）、CoS・operator・mcp の回答は承認に数えないことを書く。
   この WorkUnit の範囲（`crates/task-api/src/cos/` など）の外なので書かなかった。段 close か、CoS が戦略の文書を扱い始める子 3・5 の範囲に入れる。
2. `cos/strategy_gate/tests.rs` の `api_state` と `handlers/tests.rs` の `state` は同じ形の設定の写し。`handlers/tests.rs` の `state` を
   `pub(crate)` にすれば `api_state` を消せる（1 行の可視性の変更。`handlers/` を範囲に持つ後の変更で行う）。
