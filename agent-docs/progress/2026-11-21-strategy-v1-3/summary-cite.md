---
title: 戦略層 v1 子 3 summary-cite — 引用の型と検査・失効の導出（純粋関数）と、strategy の chat 参照の revision・sha256
tasks: [01M7XQ9QE8STND208NXG90F743]
status: done
updated: 2026-11-22
---
# summary-cite（WorkUnit。子 3 の段 cite）

完了日: 2026-11-22（Run #1）。設計は [ADR 2026-11-21-chat-discussion-mode](../../adr/2026-11-21-chat-discussion-mode.md) の D3（引用の欄）・D6・D8.1。
基準は `$CELERIS_WU_BASE`。検査は提出時の HEAD で実行した。dispatcher の添付の検査・返事の `citations`・GET と SSE の表示は次の cite-attach が受け持つ。

## したこと

- `crates/task-core/src/chat_citation.rs`（新規。crate の中の何にも依存しない葉）:
  - 型 `ChatCitation {thread_id, message_id, revision, sha256}`（`deny_unknown_fields`。状態の欄を持てない）を 1 つだけ置いた。
    `task_core::chat` は型だけを再 export する（`chat/mod.rs` に 1 行）。
  - 状態 `ChatCitationState`（`current`・`edited`・`retracted`・`missing`）と導出の関数 `citation_state(c, now)`。
    撤回済み → `retracted`、引いた revision < 今の revision → `edited`、revision と本文の sha256 が今と一致 → `current`、
    それ以外（message が無い・別の thread・今より先の revision・同じ revision で別の本文）→ `missing`。状態は保存しない。
  - 検査の関数 `verify_citation(citing_thread_id, c, now)`: 形（id が空でない、revision ≥ 1、sha256 は 64 桁の小文字 hex）、
    message がある、thread が同じか統括の thread（`kind='general'`）、revision と本文の sha256 が今の message と一致。拒否は
    `ChatCitationRefusal`（code `chat_citation_invalid`・`_missing`・`_other_thread`・`_edited`・`_retracted`）。
  - `celeris.discussion-summary/1` の型 `DiscussionSummary {schema, project_id, thread_id, through_seq, body, citations[]}` と
    検査 `check_discussion_summary`: schema、thread と案件が添付する thread のもの、`through_seq` ≥ 1、`body` は空でなく 16,384 byte まで、
    `citations` は 1〜256 件、各引用は `verify_citation`、同じ thread の引用は `through_seq` 以下の seq。違反は全部返す。
- `crates/task-core/src/discussion_citation.rs`（新規）: 引いた message の今の事実 `CitedMessage`（thread の案件・統括か・seq・
  `metadata_json.revision`・本文の sha256・撤回）を読む `cited_message(conn, …)` と `SqliteStore::chat_cited_message`。
  `chat_messages` と `chat_threads` を読むだけで、何も書かない。cite-attach が添付の transaction の中で同じ関数を使える。
- 子 2 の `celeris.strategy/1`: `Reference::Chat` を `Reference::Chat(ChatCitation)` に変えた（revision と sha256 が必須）。
  `RefStatus::Cited(CitedMessage)` を足し、`check_shape` は `citation_shape_ok`、`check_sources` は `retracted`・`missing` を
  `SourceMissing`（`strategy_source_missing`）、`check_stale` は `edited` を `Stale`（`strategy_stale`）、`check_project` は引いた thread の案件で判定する。
  どれも `citation_state`（要約の引用と同じ関数）で導く。fixture 6 件の chat 参照に `revision`・`sha256` を足し、v1 の sha256 が変わったので
  v2 の 2 件の `supersedes.sha256` を新しい値に直した。
- `crates/task-api/src/cos/strategy_gate.rs`: chat 参照の解決を `chat_message_get` + `chat_thread_get` から `chat_cited_message` に置き換えた
  （旧い解決の経路は消した）。

足していないもの: migration・表・Manager・trait・Agent サービス・状態機械・route。公開の API の wire には今回まだ型が出ない
（`ChatCitation` は `JsonSchema` を derive したが、API の応答に載るのは cite-attach の `ChatCitationView` から。`ApiV1Schema` への登録は cite-attach が行う）。

## 試験（守る不変条件）

task-core `chat_discussion_summary_*`（8 件、`crates/task-core/src/discussion_citation_tests.rs`。message の投稿・配達・編集・撤回は本物の store の経路）:

| 試験 | 不変条件 |
|---|---|
| `chat_discussion_summary_citation_current` | 今の版の引用は `current` で、要約の検査を通る |
| `chat_discussion_summary_stale_after_edit` | 編集で同じ引用が `edited` になり、添付は `chat_citation_edited` で拒否。新しい版の引用は `current` |
| `chat_discussion_summary_stale_after_retract` | 撤回で `retracted`。撤回後の版を引いても拒否 |
| `chat_discussion_summary_citation_other_thread_refused` | 別の案件の thread の発言は `chat_citation_other_thread`、統括の thread の発言は通る。別の thread の要約は `discussion_summary_other_thread` |
| `chat_discussion_summary_citation_wrong_revision_refused` | 今より先の revision・別の本文の sha256・別の thread の id は `missing`、形の誤り（revision 0・大文字 hex・空の id）は `chat_citation_invalid` |
| `chat_discussion_summary_missing_source_refused` | 引用 0 件の要約は `discussion_summary_source_missing`、無い発言の引用は `chat_citation_missing`、`citations` の無い file は読めない |
| `chat_discussion_summary_state_is_not_stored` | 引用の wire は 4 欄だけで `state` の欄は拒否。引用の値を変えずに編集だけで `edited` になり、message の `metadata_json` に状態が無い |
| `chat_discussion_summary_limits_and_shape` | body 16,384 byte は通り 16,385 は `discussion_summary_limit`、引用 257 件も limit、schema 違い・`through_seq` 0・空の body・`through_seq` の後の発言の引用は invalid、未知の欄は読めない |

task-core strategy（`crates/task-core/src/strategy/tests.rs`）:
- `chat_discussion_summary_strategy_ref_edited_is_stale`: 引いた発言の revision が上がると `Rule::Stale` だけ（`strategy_stale`）、撤回・無い発言・同じ revision の別本文は `strategy_source_missing`。
- `strategy_contract_chat_ref_requires_revision_and_sha256`: revision・sha256 の欠け、revision 0、大文字 hex、`state` の欄は `Rule::Shape`。

task-api（`crates/task-api/src/cos/strategy_gate/tests.rs`）:
- `chat_discussion_summary_strategy_gate_edited_is_stale`: 統括の thread の人の発言を引いた承認済みの戦略は今の版なら task を作る。編集の後は 422 `strategy_stale`
  （`codes` は `strategy_stale` だけ）、撤回の後は 422 `strategy_source_missing`、revision・sha256 の無い参照は 422 `strategy_invalid`。拒否はどれも task を作らない。
- 既存の gate の試験 9 件は、chat 参照を `chat_ref(…, 1, MESSAGE_TEXT)`（revision 1 と本文の sha256）に直しただけで、主張は変えていない（規約 10: 弱めた試験は無い）。

既存の試験で検出できない理由: base には引用の型・失効の導出が無く、base の strategy の chat 参照は `{thread_id, message_id}` だけで、
発言があれば通した（`Reference` の struct variant は未知の欄を黙って無視するので、revision を書いても照合されなかった）。

### 変更前に落ちること（実行した）

本番のコードの 4 file（`strategy/mod.rs`・`lib.rs`・`chat/mod.rs`・`cos/strategy_gate.rs`）だけを HEAD（実装の commit の前）の版に戻し、
新しい gate の試験を走らせた。確かめた後に元へ戻した。

```
cargo test -p task-api --lib -- chat_discussion_summary_strategy_gate_edited_is_stale
test cos::strategy_gate::tests::chat_discussion_summary_strategy_gate_edited_is_stale ... FAILED
panicked at crates/task-api/src/cos/strategy_gate/tests.rs:548:5:
assertion `left == right` failed: {"operation":{"action":"task.create", … "state":"applied", … }}
  left: 200
 right: 422
test result: FAILED. 0 passed; 1 failed
```

base の gate は、引いた後に編集された発言を根拠にした戦略で task を作った（200、`state: applied`）。変更後は 422 `strategy_stale` で task を作らない。
task-core の `chat_discussion_summary_*` 8 件と strategy の 2 件は、base に `chat_citation`・`ChatCitation`・`RefStatus::Cited` が無いので compile できない（ADR D8.2 のとおり）。

## 検査（提出時の HEAD で実行）

- `cargo test -p task-core --lib -- strategy chat_discussion_summary`: `29 passed; 0 failed`（chat_discussion_summary 9 件を含む）。
- `cargo test -p task-api --lib -- strategy_gate chat_discussion_summary`: `10 passed; 0 failed`。
- `bash scripts/dev/test-affected.sh`: exit 0（927 秒）。`repo-health: verdict=pass … fail_grade=0 new=0 worsened=2`、`repo_health_gate: all checks passed`、
  web の型検査と unit 試験 101 file・705 件 pass、`test-ts-verify: 20 passed, 0 failed`、Rust `8629 tests run: 8629 passed (9 slow), 31 skipped`。
- `cargo clippy --workspace -- -D warnings`: exit 0（警告は vendor の `deno_core` の 1 件だけで、既存）。
- `cargo fmt --all -- --check`: exit 0。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の path は上の file・fixture 6 件・`lib.rs`（新しい 2 module の宣言）・この進捗だけ。

## LoC（`git diff --numstat $CELERIS_WU_BASE HEAD`、進捗 file を除く）

- production Rust: +430 / −28（純増 402）。内訳: `chat_citation.rs` +293、`discussion_citation.rs` +70、`strategy/mod.rs` +53 −18、
  `cos/strategy_gate.rs` +9 −10、`lib.rs` +4、`chat/mod.rs` +1。
- 試験と fixture: +713 / −37（`discussion_citation_tests.rs` +429、`strategy/tests.rs` +122 −12、`strategy_gate/tests.rs` +114 −5、fixture +32 −12）。
- 純増の理由: 引用の型・4 つの状態の導出・要約の schema の検査は base に無い契約で、置き換える旧い実装が無い。strategy の chat 参照の形の検査と
  解決は、旧い inline の検査と `chat_message_get`+`chat_thread_get` の経路を消して共通の関数に置き換えた。task-core の chat の本体は 1 行だけ増えた。

## 親の目的への寄与（確認済み・推定・未確認）

- 確認済み（試験）: 改善 B の「version・引用付きで共有」の土台。要約と戦略の chat の根拠が、引いた版の revision と本文の sha256 を持ち、
  元の発言の編集・撤回で失効する（要約の検査と、子 2 の実行の入口 `strategy_gate` の両方）。別の案件の発言を根拠にできない（T4）。
- 推定: observability（子 7）で「議論の出典」と失効を表示する材料になる。表示は cite-attach・web-cite の後で、今は未確認。
- 未確認: dispatcher の添付の時点の検査（cite-attach）、GET・SSE での失効の表示、本番での動作。

## 未解決事項

- ADR 2026-11-21-chat-discussion-mode D6 は「`ChatCitation` を `task_core::chat` に置く」と書く。実装は葉の `task_core::chat_citation` に置き、
  `task_core::chat::ChatCitation` として再 export した（repo health の `size.responsibility-growth` のため。型は 1 つ）。ADR への付記は範囲の外なので書いていない。
- 子 2 の ADR 2026-11-21-strategy-evidence-contract D1.2 の参照の表の `chat` の行は `{thread_id, message_id}` のまま（範囲の外）。

## 提案

- close か integrate-cite で、ADR 2026-11-21-chat-discussion-mode に付記する: D6 の型の置き場所は `task_core::chat_citation`（`task_core::chat` が再 export）、
  事実の読み出しは `task_core::discussion_citation::cited_message`。`missing` には「同じ revision で別の本文」「今より先の revision」を含める。
- 同じく ADR 2026-11-21-strategy-evidence-contract D1.2 の `chat` の行を `{thread_id, message_id, revision, sha256}` に直す。
