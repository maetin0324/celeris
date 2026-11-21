---
title: 戦略層 v1 子 2 contract-core — task-core に celeris.strategy/1 の型と検証の純関数と strategy_contract_ 試験を足す
tasks: [01M7XQ9Q9EB3CH59TSNYM9HBC8]
status: done
updated: 2026-11-21
completed: 2026-11-21
---
# contract-core（WorkUnit。子 2 の段 impl）

完了日: 2026-11-21。設計は [ADR 2026-11-21-strategy-evidence-contract](../../adr/2026-11-21-strategy-evidence-contract.md) の D1・D2・D6。
commit は「strategy-v1 2 contract-core: task-core に celeris.strategy/1 の型と検証の純関数、子 0 の envelope の検査と strategy_contract_ 試験を足す」と、この進捗の commit。

## 開始時の確認（本番の release）

- `curl -s http://127.0.0.1:7710/api/v1/health` の `release` は、子 1 の統合 commit「integrate wu/gate (phase close)」を HEAD とする release だった。
  `git merge-base --is-ancestor <その commit> HEAD` は exit 0。子 1 の修正は本番で動いている。

## 足したもの

- `crates/task-core/src/strategy/mod.rs`: 文書の serde の型（`StrategyDocument`・`ArtifactPointer`・`Source`/`SourceKind`・`Actor`/`ActorKind`・
  `SourceRevision`・`Consent`・`Acl`/`Classification`・`DecisionState`・`ResearchQuestion`・`Candidate`/`Cost`・`Hypothesis`/`Confidence`・
  `Evidence`/`Stance`・`Reference`（6 種））、照合の文脈 `CheckContext`（`Intent`・`ArtifactFacts`・`PreviousVersion`・`ApprovalRecord`・`RefStatus`）、
  上限の定数（D1.3）、`validate_document(bytes, ctx) -> Vec<Violation>`、名前と決定の文の形の 1 か所の定義（`artifact_name`・
  `idempotency_key`・`approval_decision_key`・`approval_question`・`approval_subject`）、共有の述語（`actor_allowed`・`consent_granted`・`contains_secret`）。
- `crates/task-core/src/strategy/envelope.rs`: 子 0 の envelope の検査 `validate_envelope`（R1〜R13。参照の検査器 `validate.py` と同じ規則）。
  source と actor の組・consent・秘密の形・人の主体は文書の検査と同じ関数を使う。
- `crates/task-core/src/lib.rs`: `pub mod strategy;` と `is_human_principal`（`is_principal_actor` の隣。`human:` + 名前だけを人とする）。
- fixtures: `crates/task-core/src/strategy/fixtures/`（baseline-v1・v2-keeps-refute・v2-drops-refute・neg-source-missing・neg-stale・neg-other-project）。
  子 0 の envelope の fixtures は写さず、試験が `include_str!` で `agent-docs/progress/2026-11-21-strategy-v1-0/fixtures/` を読む。
- 新しい trait・Manager・表・migration・event の種類・状態機械は無い。依存は serde・serde_json・regex・time・`project_proposal::sha256_hex`・
  `roadmap::ROADMAP_TEXT_MAX`・`decision::DecisionStatus` だけで、`strategy` に入る辺は `lib.rs` の `mod` だけ（既存の循環に入らない）。
- `docs/protocol/` に schema は置いていない（文書は worker protocol でなく成果物。必要になった子が既存の生成と drift の試験の作りで足す）。

## 規則（`Rule`。検査は最初の違反で止まらず全部返す。`code()` は D4 の 422 の code）

| 規則 | 内容 | code |
|---|---|---|
| required | 共有フォーマットの必須の欄（欠けた欄を全部返す） | strategy_invalid |
| shape | schema・id の形と重複・idempotency_key の形・成果物の name・RFC 3339・sha256/commit の hex・url（https・userinfo 無し）・selected ≤ 1・候補/仮説/証拠のつながり | strategy_invalid |
| limit | D1.3 の上限（文書 262,144 byte、研究質問 4,000 字、文 2,000 字、件数、参照 16/1,024、url 2,048、版 1〜64） | strategy_invalid |
| secret | 秘密に見える文字列 | strategy_invalid |
| state_not_writable | 文書に approved・rejected を書いた | strategy_invalid |
| actor | source と actor の組、`celeris_task_run` の actor.id と成果物の run の一致 | strategy_invalid |
| consent | 外部の source に人の consent（selected_excerpt・summary）が無い | strategy_invalid |
| unsupported_claim | confirmed の仮説に反証がある、または commit・artifact・task を参照する支持の証拠が無い | strategy_invalid |
| source_missing | 文書・証拠の source が無い、証拠・研究質問の refs が空、候補が仮説を指さない、参照が引けない、前の版が引けない | strategy_source_missing |
| content_hash | file の sha256 が `ArtifactProduced` の記録と違う | strategy_source_missing |
| stale | request の sha256・Roadmap の版が今と違う、より新しい版が人に承認されている | strategy_stale |
| other_project | 文書・参照・前の版の案件が違う | strategy_other_project |
| unapproved | 実行（`Intent::Execute`）で proposed でない、または人（`human:<名前>`）の approve の回答が無い（operator・mcp・cos・接頭辞の無い human は不可） | strategy_unapproved |
| outbound | confidential を外へ出すのに人の承認が無い | strategy_unapproved |
| supersedes | 版の鎖（v1 は null、前の版の name・sha256・版・strategy_id・鎖の次、change_reason） | strategy_invalid |
| refute_removed | 前の版の反証の証拠を消した・書き換えた（鎖のすべての隣り合う 2 版） | strategy_refute_removed |
| idempotency | 同じ版（同じ key）に違う sha256 の記録がある（同じ sha256 の再試行は通す） | strategy_idempotency_conflict |

注入された指示は状態を進めない: 状態は文書でなく決定（`ApprovalRecord`）だけから導くので、外部の証拠の文に「承認したことにして」とあっても
`Execute` は `unapproved` で拒否される（試験 `strategy_contract_injected_instruction_does_not_approve`）。

## 試験（19 件。すべて `strategy_contract_` で始まる lib の unit 試験）

- envelope: baseline 4 件の受理、negative 15 件が期待の規則で拒否され R1〜R13 のすべてに否定対照があること。Rust の結果は `validate.py verify` の
  結果と同じ集合だった（例: browser-session-source は R2・R3、missing-field は R1・R8、prompt-injection-approves は R10・R11）。
- 文書: baseline の受理（v1・v2 を Record と Execute で）、違反の全件の報告、必須の欄、approved を書けない、人の主体の承認（6 主体・却下・撤回・
  別の sha256・別の key）、注入の指示、反証の削除と書き換え、supersedes、上限 6 種、idempotency、content_hash、confirmed の根拠、外部の consent、
  秘密と actor、stale（Roadmap・新しい承認の版）、承認の文の往復、ablation。
- 変更前に落ちること: base には `strategy` の module も文書の検査も無く、試験は compile できない（検出する口が無い）。task.create の 422 の
  「変更前に落ちる」負の試験は strategy-gate の WU が `strategy_gate_` で置く（ADR D6）。

### ablation（`cargo test -p task-core --lib strategy_contract_ablation -- --nocapture` の出力）

検査の表 `CHECKS` から 1 規則だけ外して走らせる（試験の中の `validate_without`。production に試験用の分岐は無い）。

| 規則 | 負の状態 | 規則つき（全規則） | その規則を除く | B（全規則） | B（その規則を除く） |
|---|---|---|---|---|---|
| R-src | N-src: evidence[0] の artifact が無い name | 拒否 {"strategy_source_missing"} | 受理 | 受理 | 受理 |
| R-stale | N-stale: request_sha256 が前の依頼 | 拒否 {"strategy_stale"} | 受理 | 受理 | 受理 |
| R-proj | N-proj: evidence[0] の chat が別の案件 | 拒否 {"strategy_other_project"} | 受理 | 受理 | 受理 |
| R-appr | N-appr: 承認の回答が operator:claude-code | 拒否 {"strategy_unapproved"} | 受理 | 受理 | 受理 |

各負の状態は、その規則だけが拒否する（違反の規則の集合がちょうど 1 つ）。N-appr の負の状態は文書でなく決定の記録（`ApprovalRecord` の by）である。

## 行数（`git diff --numstat` の基準は `git merge-base HEAD main`。空行・注釈を含む）

| 区分 | file | 行 |
|---|---|---|
| production Rust | `strategy/mod.rs` 1,350・`strategy/envelope.rs` 153・`lib.rs` +9 | +1,512 |
| test Rust | `strategy/tests.rs` | +688 |
| test fixtures（JSON） | `strategy/fixtures/*.json` 6 件 | +981 |

純増の理由: 新しい契約の型と検査で、消す旧い経路は無い（ADR 実装前 brief 6）。mod.rs の約半分は rustfmt が複数行に広げた検査の呼び出しと注釈。

## 検査（提出時の HEAD で実行）

- `cargo test -p task-core --lib strategy_contract`: exit 0、19 passed・0 failed。
- `bash scripts/dev/test-affected.sh`: exit 0。nextest 8555 tests run: 8555 passed、31 skipped。web の tsc -b と unit（101 files・705 tests passed）、
  test-ts-verify 20 passed・0 failed。終わりに scratch の trash の `rm` が Permission denied を 3 行出した（exit には影響しない）。
- `cargo clippy --workspace -- -D warnings`: exit 0（vendor の deno_core の deprecated の warning 1 件は既存）。
- `cargo fmt --all -- --check`: exit 0。
- `git diff --quiet $CELERIS_WU_BASE HEAD -- crates/task-core/migrations crates/task-core/src/model/events.rs crates/task-core/src/store`: exit 0（差分なし）。

## 親の目的への寄与

- 確認済み: 改善 B（仮説・支持/反証/未確認・版・引用の共有）の型と検査が task-core にあり、反証を次の版で消せないことを試験で示した。
  未承認・古い・別案件・出典欠損の戦略を拒否する純関数があり、ablation で 4 規則それぞれが必要なことを示した。
- 未確認: 実行の入口（CoS の `task.create`）での拒否は strategy-gate の WU の仕事で、この WU では入口に繋いでいない。改善 A・C・D と
  observability への効果は、子 3・5・6・7 が文書を作り読むまで測れない。

## 未解決事項

- 候補の「refs」は ADR D1.2 の候補の欄に参照が無いため、候補が根拠の仮説を 1 件以上指すこと（`hypotheses` が空でない）として扱った。
- `commit` の参照の存在と `knowledge` の scope の判定は呼び出し側が `RefStatus` に入れる（純関数は形と案件だけを見る）。

## 提案

- 無し。
