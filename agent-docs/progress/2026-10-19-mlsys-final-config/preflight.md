---
tasks: [01M58VFRBA6HG0SSF3CG5BKM91]
unit: runbook-preflight
status: done-pending-operator-input
updated: 2026-10-19
---
# runbook 整合と launcher prepare/preflight の確認（有料実行 0 件・実 LLM なし・外部ネットなし）

## runbook（`docs/ops/measure-run-final.md`）の更新
- 実在の `scripts/campaign/measure-launcher.sh <prepare|start|preflight|watch|grade|export|stop> --env <file>` と必須 env 項目（スクリプトの `need` と同じ 12 項目。`start` は `CELERIS_BIN`・`RELEASE` 追加）に一致。
- §3 を「確定済み構成 + H1 後に埋める値」に置換。`evidence_table_id` / `celerisctl routing evidence` の旧手順を削除。B/C 同一 run_harnesses=[claude-code]、C は no_evidence_table=true、B は none、K2 judged_scope=[implementation, experiment]（analysis/review は not_judged）を明記。§3b に測定専用 daemon の cap-probe 宣言を追加。
- `GRADE_CASES_ROOT` の旧値（存在しない `…/grade-case`）を実在の `crates/campaign/fixtures/grade-cases` へ（runbook と `measure-launcher.example.env`）。
- env 雛形の `<operator 入力待ち: …>` を引用符で囲んだ。以前は sh の構文エラー（`Syntax error: newline unexpected`、exit 2）で落ちており、拒否理由が読めなかった。今は未置換値が path 不在などで明示的に拒否される。

## 実行した出力（CARGO_TARGET_DIR の debug/celeris-campaign、一時 dir `$TMPDIR/pf` で。本番 config は読むだけ）
1. `celeris-campaign validate --manifest …/final.json` → exit 1: `config A: harness_version is an unresolved placeholder: claude-code@<claude-code version at H1>`（意図どおりの拒否）。
2. 雛形 env のまま `prepare` → exit 1: `daemon_config not found: …/<operator 入力待ち: …>`。
3. dir・雛形 toml・空 canaries を仮置きした env で:
   - `prepare` → exit 1: `config A: harness_version is an unresolved placeholder …` / `manifest validate failed`
   - `preflight` → exit 1: `run prepare first (no pinned daemon binary)`
   - `start` → exit 1（daemon は起動せず）。`celeris.campaign-config-check/1`, `ok:false`, `paid_calls_made:0`, missing:
     - measurement workspace_root が通常運用の workspaces と重なる（雛形が `<abs local path>` のまま）
     - measurement `[[clusters]]` が通常運用と違う（`NORMAL_CONFIG` の複写が未実施）
     - canaries.json に forbidden と required の canary が各 1 件以上要る
4. 宣言検証: `cargo test -p celeris-campaign --test measure_daemon_example` → 3 passed（cap-probe-claude-code 宣言あり・probe は実候補に入らない・宣言欠落は拒否）。

## 不足一覧（operator 入力待ち。埋めたふりはしない）
| # | 不足 | 担当 |
|---|---|---|
| 1 | B/C `harness_version`＝昇格 release sha12（H1）。`RELEASE` と `CELERIS_BIN`（その release の celeris 実行 file） | operator |
| 2 | A `harness_version`＝`claude-code@<実際の版>` | operator |
| 3 | `A_ACCOUNT`（claude_oauth account ID。`GET /api/v1/llm/accounts`） | operator |
| 4 | 測定専用 daemon config の実値化・配置（port・db・workspace・KB・token_file・`CAMPAIGN_BIN` 実体・`[[clusters]]` 複写）と起動（launcher `start`） | operator |
| 5 | `canaries.json`（forbidden / required 各 1 件以上）、`cap-env.json`（承認値）、materials/KB/repo/inputs の配送（runbook §4） | operator |
| 6 | 置換後の `validate` の campaign sha256、env-lock/3・campaign lock（`watch` 初回）、実 daemon 下の cap-probe 実走 | operator（有料/daemon 起動を伴うため未実施） |

## operator が実行する手順
runbook §0（H1〜H6）→ §3（値の置換と `validate`/`dry-run`）→ §3b（daemon config）→ §4 → §5 `prepare` → `start` → `preflight`（exit 0 を確認）→ `watch`。停止・再開は §7、採点・export は §8。

## 検査（commit 前）
test-parallel exit 0（nextest 5813 passed, 0 failed, 15 ignored）、clippy -D warnings exit 0、fmt --check exit 0。

## 未解決事項（範囲外）
- `sh scripts/dev/check-doc-links.sh` が `crates/campaign/tests/measure_daemon_example.rs:10` の `../../agent-docs/progress/2026-10-16-mlsys-final` を壊れた参照として 1 件報告する（daemon-decl 由来、exit は 0 だが警告）。この WU の範囲（docs/ops/measure-run-final.md・agent-docs/progress・adr）外のため直していない。一度 path 連結へ書き換えたが範囲 check に落ちたため戻した。integrate 段か別 WU で直す。
