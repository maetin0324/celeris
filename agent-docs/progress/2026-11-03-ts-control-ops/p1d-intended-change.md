---
title: P1-D — 意図した TS の方針改善（behavior change）を parity と分けて数える
tasks: [01M6HGFR1TD2G3H4JF6J67CTBM]
status: done
completed: 2026-11-03
updated: 2026-11-03
---
# P1-D — 意図した TS の方針改善を parity と分けて数える

完了日: 2026-11-03。親 task 01M6HG7T6ZKTX239KGH0QY4HFQ（段階移行 2）の子 p1d（p0b の後）。設計は
[ADR 2026-11-03-ts-intended-behavior-change](../../adr/2026-11-03-ts-intended-behavior-change.md)。
p0b（[cos_digest の原子化](p0b-cos-digest-persist.md)）で apply・記録・計数が 1 transaction になったので、純粋領域でなく `cos_digest` で行った。

## 結論
- 人が承認した改善仕様（版・hash 付き）の期待差分と **byte 一致する差分だけ** を outcome `explained` として数える。script-guard は `explained` で戻さず、
  昇格の前提（`clean_streak`）に入れる。承認の無い差分・期待と 1 文字でも違う差分・安全違反・`ts_error`・`invalid_output`・`unavailable` は今までどおり
  （`mismatch` は 1 件で `rust` へ戻す）。正規化は足していない（`normalized` は空のまま、`ts_commands_head` も残す）。
- 承認は人の API token の主体だけ（CoS・外部 operator・`system:*` は 403）。取り消しは誰でも（説明を減らす向き）。
- S6 pilot の 200 文字の打ち切り（`scripts/dev/ts-pilot.sh` の候補 patch）を改善仕様 `cos-digest-cap-run-reason` v1 で再評価した:
  旧挙動の fixture 2066 件の差分 0、pilot の 15 件のうち差分 9 件が全部「説明済み」、追加の境界 4 件も期待どおり。Rust 参照は直していない。
- 本番・release・mode の昇格・guard の解除・K2/K4/IEEE Access の環境には触れていない。

## 変更の要点（最新 main の file・関数）
| file | 関数・型 | 要点 |
|---|---|---|
| `crates/task-core/src/script_behavior_change.rs` | `BehaviorChangeSpec`・`validate`・`sha256`・`applies_to`・`expected_output`・`explains`・`explain`・`domain_supported` | 改善仕様（D1）。期待出力 = 仕様の編集を Rust の出力に当てたもの。`find` が 2 回以上なら作れない（曖昧）。 |
| `crates/task-core/src/script_compare.rs` | `CompareOutcome::Explained` | 新しい outcome（D2） |
| `crates/task-core/src/script_compare/cos_digest_record.rs` | `CosDigestCompareRecord::explained_by` | 説明した仕様（id・version・spec_sha256）。他の行では欄を書かない |
| `crates/task-core/src/script_guard.rs` | `ScriptGuardCall::Explained`（窓の `e`）・`explained_in_window` | 戻す条件に入らず、`clean_streak` を進める |
| `crates/task-core/migrations/0108_script_behavior_changes.sql`・`store/script_behavior_changes.rs` | `script_behavior_change_approve`・`_revoke`・`script_behavior_changes`・`_events`・`_approved` | 承認・取り消し・版の置き換え。`supersedes` は同じ `id` の最大の版の hash（行が無ければ `null`）と照合する。記録の表の UPDATE・DELETE は trigger が拒む |
| `crates/task-dispatch/src/script_policy.rs` | `classify_cos_digest_pass(…, approved)`・`explain_cos_digest_run`・`ScriptDomainStore::script_behavior_changes_approved` | 違った run だけを説明にかける。検証に落ちた出力は説明しない |
| `crates/task-dispatch/src/dispatcher/cos_chat/digest.rs` | `PassScript::behavior_changes`・`cos_digest_behavior_changes` | pass の始めに承認済みを読む（読めなければ空＝説明しない） |
| `crates/task-dispatch/src/dispatcher/script_compare.rs`・`cos_chat/digest.rs` | `ScriptCompareState::next_digest_pass_id`（`#[cfg(test)]`） | 試験だけが pass id を決める。canary の側を乱数なしで選ぶため。本番の build には無い |
| `crates/task-dispatch/src/dispatcher/tests/cos_digest_behavior_change.rs` | `canary_pass`・`cos_digest_behavior_change_canary_*` | mode `canary` の e2e（下の証拠） |
| `crates/task-api/src/script_behavior_changes.rs` | `GET`・`POST /script/behavior-changes`、`POST …/{spec_sha256}/revoke` | 403・422・409 の code は ADR D4。CoS の operations は EXCLUDED（`human_only`・`script_api_only`） |
| `control-ts/behavior-changes/cos_digest/cos-digest-cap-run-reason.v1.json`・`.cases.jsonl` | 仕様と期待差分 fixture（19 行） | 期待差分 fixture は Rust の試験が参照実装と仕様から作る（`UPDATE_BEHAVIOR_CASES=1`） |
| `scripts/dev/ts-behavior-eval.sh` | 独立した評価 | 候補 patch を使い捨ての作業ツリーに当て、旧 fixture と期待差分 fixture で TS を数える。cargo 0 回 |

API の文書は `docs/api/v1/api.md` §3.134.5（エンドポイント 233〜235）、schema は `docs/api/v1/api-v1.schema.json`・`web/api/generated/` を再生成した。
CoS の操作の表は `agent-docs/adr/2026-10-09-cos-operations-all-mutations.md` に 2 行足した。schema 版は 108（`crates/campaign/src/ops_r1_trace.rs` の対応版にも足した）。

## 証拠（試験コマンドと exit）
個別の試験（作業中に実行）:
- `cargo nextest run -p task-core -E 'test(/behavior_change/)'`: exit 0、13 passed（仕様の検証・期待差分 fixture と参照実装の一致・pilot の 9 件・199 文字/「…」無し/別の差分/曖昧/未承認/別領域/入力条件の否定・承認の人だけ・版の鎖（最初の版は `null`、取り消した版の次も鎖を切らない、古い版の hash は拒否）・取り消し・記録の UPDATE/DELETE を trigger が拒む）。
- `cargo nextest run -p task-core -E 'test(/script_guard/)'`: `script_guard_explained_counts_toward_readiness_but_mismatch_still_stops` を含め passed。
- `cargo nextest run -p task-dispatch -E 'test(/cos_digest_behavior_change/)'`: exit 0、4 passed（e2e: mode `ts` で TS の改善を apply → 記録 `explained`・mode は `ts` のまま、未承認・取り消し・199 文字・「…」無し・「...」は `mismatch` で `rust` へ戻る、compare では `explained` が `clean_streak` を進め `mismatch` が 0 に戻す）。
- `cargo nextest run -p task-dispatch -E 'test(/cos_digest_behavior_change/)'`: exit 0、8 passed。上の 4 件に mode `canary` の e2e 4 件を足した。実際の digest pass の入力（失敗した run の理由 500 文字）で:
  - TS の側（選ばれた pass）: TS の改善を apply → 記録 `explained` → `ts_side_clean_streak` が 199 から 200 へ進み、`readiness(CosDigest, Canary, …)` が ready、人の `canary→ts` が通る（段階昇格の候補）。1 件足りないと `ready` にならず `canary→ts` は拒まれる。
  - Rust の側（選ばれない pass）: Rust を apply、影の比較で `explained`、`clean_streak` だけが進み `ts_side_clean_streak` は進まない。mode は `canary` のまま。
  - 説明できない差分（承認なしの pilot・承認ありの 199 文字）: 両方の側で `mismatch`、script-guard が `rust` へ戻し（guard の戻しは 1 件）、連続は 0、`ready` にならない。
  - 新しい 4 件と既存の 4 件は `tokio::test(start_paused = true)` にした（tick の待ちの予算 200 ms に実時間が当たらないように）。
- `UPDATE_SCHEMA=1 cargo nextest run -p task-api -E 'test(committed_schema_matches_generated) | test(/script_behavior_change/) | test(/cos_ops_/) | test(/script_compare_event_types/)'`: exit 0、42 passed。

独立した評価（`bash scripts/dev/ts-behavior-eval.sh --out agent-docs/progress/2026-11-03-ts-control-ops/p1d-eval [--patch …]`）:
| label | 候補 | 旧 fixture 2066 件の差分 | 期待差分 fixture 19 件 | exit |
|---|---|---|---|---|
| `head` | 改善なし（HEAD の TS） | 0 | equal 8・食い違い 11（改善が無い） | 1 |
| `pilot-candidate` | S6 pilot の `candidate.patch` | 0 | explained 11（pilot 9）・equal 8・食い違い 0 | 0 |
| `off-by-one` | 199 文字で切る patch | 0 | equal 5・食い違い 14 | 1 |

各 1.5〜1.8 秒、cargo 0 回。raw records は [p1d-eval/records.jsonl](p1d-eval/records.jsonl)（label ごとの JSON も同じ dir）。patch は同じ dir の `pilot-candidate.patch`・`off-by-one.patch`。

workspace の検査（提出時の HEAD で実行。結果は下の「提出時の検査」）。

## 性能
- TS だけの変更の経路（`ts-verify.sh`）は変えていない。edit→verify→activate の warm p50 は S6 の 7.47〜7.52 秒のまま（この task で再計測していない。未計測）。
- 独立した評価は 1 回 1.5〜1.8 秒（上の表）。cargo を起動しない。
- dispatcher の追加は pass ごとの select 1 回（承認済みの仕様の読み出し）と、違った run だけの JSON の組み直し。計測はしていない（未計測）。

## attempt 2 の修正（review の差し戻し 3 点）
1. mode `canary` の e2e が無かった → 上の 4 件を足した。
2. `script_behavior_change_events` に追記専用の trigger が無かった → migration 0108 に足した（0108 は未 release なので file を直した）。試験 `behavior_change_events_append_only`。
3. 版の鎖を、承認中の版があるときしか照合していなかった → 同じ `id` の最大の版と照合する。ADR D1 の表に規則を書いた。試験 `behavior_change_first_version_must_not_name_a_previous_version`・`behavior_change_chain_continues_across_a_revoked_version`。

## 未解決
1. `ts-verify.sh` の golden の段は改善仕様を読まない。旧挙動の fixture に当たる改善（今回の仕様は当たらない。試験で 0 件を固定）を出すと、
   golden の段が落ちる。期待差分を旧 fixture にも作る overlay（Rust の試験が書く）と、`compare.mjs`・runner の採点がそれを読む変更が次の gate。
2. 純粋領域（`lane_rules`・`route_priority`）は承認で 422 `behavior_change_domain_unsupported`。結線は未実装。
3. 改善仕様の `edits` は文字列の置き換え（`find`・`replace`、部品 `lit`・`input`・`truncate_chars`）だけ。数値・配列の並びの改善は表せない。
4. 本番では承認も `explained` の観測もまだ無い。本番で使うには人の承認（下の「本番への移行」）と compare の実 pass 200 件が要る。
5. 承認の取り消しは CoS の操作に無い（API だけ）。CoS が取り消せるようにするかは未決定。

## 次の gate
- 人が本番で仕様 `cos-digest-cap-run-reason` v1 を承認するか決める（承認は人の token の API）。
- 承認の後、候補 bundle を登録して `compare` で実 pass を観測し、`explained` と `mismatch` の件数を見て canary を判断する（S5-4 の人の手順のまま）。
- 未解決 1 の overlay を入れる task。

## 本番への移行
- migration 0108 は daemon の起動で当たる（表を足すだけ）。config の欄は足していない。
- 承認は人が行う（この task は行っていない）:
  `curl -X POST -H "Authorization: Bearer <人の token>" -H 'content-type: application/json' https://<host>/api/v1/script/behavior-changes -d "{\"spec\": $(cat control-ts/behavior-changes/cos_digest/cos-digest-cap-run-reason.v1.json), \"spec_sha256\": \"<hash>\", \"reason\": \"S6 pilot の改善\"}"`。
  `<hash>` は 422 `behavior_change_hash_mismatch` の本文の `computed` で確かめられる（hash は仕様の内容だけで決まる）。確認は `GET /api/v1/script/behavior-changes` の `state` が `approved`。
  戻すときは `POST /api/v1/script/behavior-changes/<hash>/revoke`。

## 提案
- 承認の画面（web）を足す: 仕様の本文・hash・期待差分 fixture の差分の例を並べ、人が 1 押しで承認できるようにする。
- S6-3 の 2（golden の凍結）に進むときは、期待差分 fixture を凍結 golden の更新の手順（人の review）にそのまま使う。

## 提出時の検査
提出時の HEAD（基準は `git merge-base HEAD main`）で実行した。
- `bash scripts/dev/test-affected.sh`: exit 0。nextest 7216 passed・21 skipped、web の tsc -b と unit 試験（697 passed・100 passed）、script bundle SKIP 0 件。
  1 回目は `task-core store::tests::event_row_schema_matches_committed` の 1 件が落ちた（`docs/api/v1/event.schema.json` が `explained` を持たない）。
  `UPDATE_SCHEMA=1` で再生成し、commit「p1d: event.schema.json を再生成する（CompareOutcome の explained）」の後に再実行して通った。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- `sh scripts/dev/progress-index.sh --check`: exit 0。
