---
title: "MLSys 本測定への最終接続: close-out（実装済み／mock 検証済み／実経路検証済み／本測定未実施）"
tasks: [01M51QNB8EC4EG0D5GA4BDVWHQ]
unit: final
status: done-with-gaps
updated: 2026-10-16
completed: 2026-10-16
---
# MLSys 本測定への最終接続 — close-out

**結論**: 接続（実行器・採点・計測・予算）の実装と mock 検証は済んだ。**実経路の検証と本測定は 1 件も実施していない**。mock 試験の通過を、実経路・本測定の完了の根拠にしない。
本 WU はコードを変えていない（文書のみ）。実測値・消費量は作っていない。

## 1. 区分表

凡例: ○ = 済み、× = 未実施、— = 該当なし。根拠の詳細は各 progress を参照。

| 項目 | 実装済み | mock 検証済み | 実経路検証済み | 本測定 |
|---|---|---|---|---|
| A 実行器（`--executor harness`、既存 claude-code ハーネス） | ○ `harness_exec.rs` | ○ stub | × | × |
| B/C 実行器（`--executor celeris`、`POST /api/v1/tasks` の通常経路・待機・再開・子 task 追跡・cancel） | ○ `celeris_exec.rs`・`curl_transport.rs` | ○ 偽 task-api | × 実 HTTP 往復なし | × |
| 冪等な試行台帳（(manifest hash, case, arm, repetition)→task/run） | ○ `ledger.rs` | ○ 応答喪失・再起動・二重起動で 1 試行 1 task | × | — |
| 試行単位の routing 固定（B=fixed、C=decision_assisted。`routing_override`、migration 0084） | ○ | ○ | × 実 daemon での解決は未観測 | × |
| campaign 識別の伝搬・全 run の消費計測（planner/worker/再試行/reviewer/Jev/CoS。欠測と 0 の区別、二重計上なし）、migration 0085–0086 | ○ | ○ temp DB | × | × |
| export と `--verify-connected`（空 export は失敗） | ○ | ○ `campaign_connected` | × 本番 DB・実 campaign では未実施 | × |
| 予算上限（per_case／per_arm／全体）の実行中適用、反復分離、cancelled/timeout/未完了を分母に残す | ○ | ○ | × 実消費では未実施。実 B/C は完了後判定が主（`max_overshoot_*` に出る） | × |
| A の失敗・待機が B/C を止めない（arm 順・試行内隔離） | ○ | ○ | × | × |
| 独立 grader（実装テスト・実行証拠・生データ再計算・既知欠陥。未実行の Markdown は不合格） | ○ `grader.rs` | ○ 偽 Markdown 不合格を含む | △ CLI の判定 JSON と exit code のみ | × K1〜K4 実 case の採点なし |
| 代表 case 5 件（dev: k0/k1/k3、final: k2/k4）と dev/final 隔離 | ○ | ○ ローダ拒否・隔離 hash | △ 外部原票 hash を実ファイルで固定 | × 受入 command の実行なし |
| API キー未登録 → 受信箱通知 → 登録 → 自動再開 | ○ | ○ `credential_wait_notify`（偽 webhook） | × 実 daemon なし | — |
| Jev smoke 判定器（jev_ok/fallback、費用・版、basis 帰属） | ○ `jev_smoke.rs` | ○ 7 件 | × 本番 task の events は読んでいない | × smoke 未実施 |
| 少量 pilot（2 ケース×A/B 各 1 回＋Jev smoke） | 手順のみ（runbook Part P） | — | × 認証なしの決定で待機 | × |
| 消費係数の実測・本測定案の確定 | 式のみ（`../2026-10-16-mlsys-pilot/proposal.md`） | — | × 係数は未取得（推定は実測でない） | × |
| 本測定（A/B/C）・独立採点・export | 手順と固定条件（`../2026-10-16-mlsys-final/final.json`） | mock の通しのみ（mock 12 試行 113,000 tokens は実測でない） | × | **未実施** |

## 2. 未実施として残してある項目（done で消していない）
1. **pilot**（実 LLM）: 係数 a,b,c,j,ρ,μ,u_x 未取得。人の決定 auth-ready（認証なし、手順と検証を残して待機）。
2. **Jev smoke** と、base standard→final frontier で basis=`fixed_rule` の帰属確認（`jev-live.md`: ロジックの誤帰属は見つからないが、記録に fixed 規則自身の lane が無く `Unverifiable`）。
3. **本測定**: 人の決定 final-run = 実行しない／できない（昇格前）。final-out・final-export は存在しない。A/B 主比較 0 件、B/C 副比較 0 件、消費 0。
4. **本測定の規模の乖離**: 承認枠は A/B 各 5 問だが、final case は 2 件（k2・k4）。dev は混ぜないため **A/B 各 2・C 2・試行 6**で固定（承認 12 以内）。5 問化は別作業。結果は 2 問の比較で、結論に使わない。
5. **grade の接続の欠け**: `grade` CLI は評価 command を走らせない（`NoRunner`、fail-closed）。`cases/` の公開 case から `celeris.grade-case/1` を作る変換と、`CommandRunner` 実装が無い。K1〜K4 の実採点は不可（`e2e.md` の提案）。
6. **計測の欠け**: CoS の実 turn から `cos_turn` を呼ぶ配線（turn 単位の usage event が無い）、Jev の token 数、`decision_log_interventions` の書き手。
7. **本体側の記録の欠け**（提案）: `LaneAssistRecord` に `fixed_lane`/`jev_lane`、Jev の版（`model_snapshot`）の記録。task-api `POST /tasks` の `idempotency_key`（現状は label で代用）。
8. 未解決の運用: cases の R1 node-h 上限承認・R2 BenchFS submodule 固定元・R3 K2 隠し試験の移植・R5 K3 統計量。cluster job は自動 cancel せず `cluster-cancel.jsonl` に人手手順を出すだけ。
9. flaky の疑い: `harness_exec::runner_drives_three_arms_with_stub_and_resumes_without_restart` が高負荷の全体並列で一度落ちた（単独は合格）。

## 3. 予算（受け入れ条件: 承認範囲内）
- 実 API・LLM・クラスタの消費は **0**。承認済み範囲（試行 12・tokens 1,000,000・7 日使用量 codex 21%／claude 24% 以内・fable は 2026-10-19 23:00Z まで不使用）を超える実行はしていない。
- 範囲を超える本測定（r>1、5 問化など）は新しい決定 `final-measure-budget` の承認待ち。API キー登録は予算承認ではない。

## 4. 本番への移行（人が実行する手順。agent は実行しない）
- config の書き換えは無い（消した config 欄なし）。DB は migration 0084〜0086（SCHEMA_VERSION=86）が起動時に適用される。events は追記専用のまま。
- 昇格: 人が GUI の「リリース」画面で行い、sha12 を控える（H1）。以降の順は `runbook.md` Part H → Part P（pilot）→ 係数から上限確定 → `../../../docs/ops/measure-run-final.md`（本測定）→ check / grade / export と `--verify-connected`。
- `final.json` の `<release sha12 at H1>` は昇格後の sha12 に置換し、validate の sha256 を控え直す。`CELERIS_CAMPAIGN_TOKEN` は運用セッションが自分の shell で設定する（argv・file に残さない）。
- 中断・`waiting>0` は同じ引数で再実行して再開する。

## 5. 証拠
- `cargo test -p celeris-campaign`: 141 passed, 0 failed（本 run で再実行）。
- 全体検査（test-parallel／clippy／fmt）と doc 検査の結果は末尾の「検査結果」。
- 先行 WU の証跡: `../2026-10-16-mlsys-measure/*.md`（executor, ledger, adapter-a/bc, metering, export-join, grader, cases, isolation, e2e, task-routing）、`jev-live.md`、`runbook.md`、`../2026-10-16-mlsys-pilot/`、`../2026-10-16-mlsys-final/`。

## 提案
- 実測の前に作る順: ①grade-case 変換と `CommandRunner`、②final case の追加（5 問化するなら人の承認）、③`LaneAssistRecord` の fixed_lane/jev_lane・版、④昇格後に pilot → 本測定。

## 検査結果
- `bash scripts/dev/test-parallel.sh` → exit 0、passed 5499 / failed 0 / ignored 14
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `sh scripts/dev/check-doc-links.sh` → ok、`check-adr-numbers.sh` → exit 0
