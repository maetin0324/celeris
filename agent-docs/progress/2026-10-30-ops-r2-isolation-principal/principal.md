---
task: 01M669QB069GND2ZKA1T97QPY9
unit: principal
status: done
completed: 2026-10-30
---
# principal: 依頼受付を operator principal の出典付き relay にし、判断者・実行者・承認者を分けて記録する

結論: `celeris-campaign ops-r1-intake` の `--send`（人の token で送る）を消し、`--relay`（operator principal で `POST …/relays`）と必須の `--source`（人の指示・承認の出典）に替えた。記録は `ops-r1-intake/2` で、`source`・`decided_by`・`executed_by`・`approved_by`・`relayed_by` と、human-ops /2 の op 行に渡す `human_op` を持つ。出典の無い operator の message と人の token の message は記録しない。測定 daemon の通し試験は人の token を足さず relay で通る。ADR `agent-docs/adr/2026-10-30-ops-r2-isolation-principal.md` D5・D6 の実装。基準は `$CELERIS_WU_BASE`（`git merge-base HEAD main` と同じ系列）。

人の決定 `relay-approved-request` の答え（承認済み依頼文を operator が relay し、出典に承認の id と依頼文 sha256 を残す）に従い、`source.kind=approved_request` は `decision_id` を必須、`said_at` は承認の時刻、`request_sha256` は依頼文と一致しなければ送る前に拒否する。

## 変えたもの

- `crates/campaign/src/ops_r1_intake.rs`: `Mode::Relay`、`SourceInput`/`Source`/`SourceKind`、`HumanOpRoles`、`--source`・`--operator-session`。
  bind・relay とも、message が `role=user`・`author` が `operator:`/`mcp:`・`relay.relayed_by == author`・`relay.said_at` が出典と同じ時点（daemon は `…Z` を `….000Z` に正規化するので時点で比べる）のときだけ記録する。
- `crates/campaign/src/ops_r1_intake_tests.rs`: 6 件に書き直し（relay 1 件化、役割の分離、bind の受け入れと拒否 9 種、出典の拒否 7 種で何も送らない、CLI、引数）。
- `crates/campaign/tests/ops_r1_intake.rs`: 実 task-api router で operator token の relay → 記録、`…/messages` で送った operator の message（relay 無し）と人の token の message は拒否、人の token の `--relay` は 403、task 0 件。記録の `human_op` をそのまま `human-op record` に渡し、relay の op 行で `decided_by`/`executed_by`/`approved_by` が別の欄に残り `counts_as_human_op` が false、人が記録した区間だけが true。
- `crates/campaign/src/ops_r1_assemble.rs`: `--b-intake` は `/2` と K4 の `/1` の両方を読む（`is_intake_schema`。K4 の記録の再集計のため。この道具は `/1` を書かない）。ADR の範囲表に無い file だが、`SCHEMA` を `/2` にすると K4 の記録が読めなくなるので 1 か所だけ変えた。
- `crates/celeris/tests/ops_r1_measure.rs`: 試験だけで足していた人の token（`human:ops-r1-e2e`）を消し、生成 config の `operator:ops-r1` で relay を送る。生成 config に `principal = "human:` が無いことも確かめる。
- `docs/ops/ops-r1-intake.md`（書き直し）・`docs/ops/ops-r1-measure-daemon.md`（人の token の経路の記述を relay に替えた）・`docs/ops/eval-raw-record-definitions.md` §3.3（relay の op 行の欄と、人の能動時間は記録区間だけ・点は長さ 0 として足さない・0 件は null）。
- `scripts/campaign/ops-r1-measure-config.sh` は変えていない（もともと `operator:ops-r1` の token だけを書く。isolate unit が同じ file を変える）。

## 証拠（提出時の HEAD で実行）

- `cargo test -p celeris-campaign --lib ops_r1_intake` → 6 passed
- `cargo test -p celeris-campaign --test ops_r1_intake --test ops_r1_pipeline_assemble` → 1 passed、10 passed
- `cargo test -p celeris --test ops_r1_measure` → 1 passed（9 秒）
- `bash scripts/dev/test-affected.sh` → exit 0、2858 tests run: 2858 passed、9 skipped（対象 celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 未解決事項

- 実測定（実 LLM・本番の承認票）では未確認。fixture と in-process router、stub harness の測定 daemon で確かめた範囲だけ。
- 人の能動時間の 0 件 → null は K4 証拠確定 task 01M669QAZE の cov-not-recorded で report 側が実装する。ここでは文書の規則だけ揃えた（probe unit が統合後に確かめる）。
- 試行区間の operator 変更操作と human-ops の op 行の 1 対 1 照合（D5 の最後）は probe unit の担当。

## 提案

- human-op の役割は `<actor_kind>:<id>` で、id に principal を入れると `human:human:rmaeda` になる。読みにくいが ADR-human-ops の書式どおりなので変えていない。id を `rmaeda` に寄せるかは評価定義の版を上げるときに決める。
