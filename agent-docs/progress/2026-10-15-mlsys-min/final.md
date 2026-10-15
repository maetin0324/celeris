---
title: "MLSys 最小実装: 最終検証と完了報告（実装済み / 未検証の区別）"
unit: final
status: done-with-gaps
updated: 2026-10-15
completed: 2026-10-15
---
# final

## 結論
- 実装は mock / fake server 試験の範囲で揃い、全体検査は通った。**Jev 実 API 分類の成功は 0 件 = 未検証**。実 LLM の測定は 0 試行。
- **未解決の不具合（要対応）**: 実 Jev 呼び出しは本番で全件 `client_error`（fixed へ fallback）。人の指摘（2026-10-15 22:53Z）の原因候補は **この branch に未修正**:
  - `crates/jev-classifier/src/question.rs:79-85` — choice の `criteria` を文字列配列で送っている（公式は label→説明 の object）。
  - `crates/celeris/src/daemon/jev_verifier.rs:33` — noul の `"criteria": "yes"` は不正。
  - `classify_stage.rs` が HTTP status を捨てる（`client_error:400` と残す、4xx の error.code/message だけ記録）。fake server が公式 schema 形を検査していない。`credential_resume.rs:345-350` は 400 を Transient 扱い。
  - 修正は final の範囲外（検証のみ）なので followup task として起票する（`artifacts/followups.json`）。reload-fix（`config.jev` 書き戻し・info ログ）は入っている。

## 検査（受け入れ条件 1）
| 検査 | コマンド | 結果 |
|---|---|---|
| fmt | `cargo fmt --all -- --check` | exit 0 |
| clippy | `cargo clippy --workspace -- -D warnings` | exit 0 |
| test-parallel | `bash scripts/dev/test-parallel.sh` | exit 0。passed 5366 / failed 0 / ignored 13、`test-parallel: ok` |
| ADR 番号 | `bash scripts/dev/check-adr-numbers.sh` | exit 0 |
| 文書 layout | `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | ok |
| 文書 link | `bash scripts/dev/check-doc-links.sh` | exit 0 |
| architecture-map | `python3 scripts/dev/check-architecture-map.py` | exit 0 |
| tmp 残り | `bash scripts/dev/check-test-tmp-leftovers.sh` | **exit 101**。plain `cargo test -p celeris` で `browser_doctor` 2 件が失敗（`browser_doctor_tests.rs:110`）。worker sandbox は userns 不可（KB: celeris-worker-sandbox-userns…）で、同じ crate は test-parallel（nextest）で通る。この変更の差分ではなく sandbox 由来と判断。運用環境での再確認は未実施 |

## 受け入れ条件（Task 全体）ごとの状態
| 条件 | 状態 | 根拠 |
|---|---|---|
| 0. fixed/decision-assisted 切替（既定 fixed）、Jev adapter が fake server で成功・不正 schema・timeout・auth・rate limit を覆い、故障時 fixed | 実装済み（fake server 試験）。**実 API は未検証** | jev-adapter.md、policy.md、live-smoke.md。ただし fake server は公式 schema 形を検査しないため実 API の schema 不一致を見逃した（上記） |
| 1. 鍵未登録で依存 WU だけ credential 待ち、受信箱/Discord 通知、WebUI 登録後に一度だけ自動再開（再起動・重複・キャンセル・誤キー・秘匿） | 実装済み（cw-e2e / jev-e2e の fake server 試験）。**実経路は未検証** | credential-wait.md。本番は鍵を先に登録したため待ちが発生していない |
| 2. 判断・実行・成果・消費ログ、campaign export、最小評価基盤（manifest/fixture/runner/checker、dry-run・見積り）、区別した報告 | 実装済み（mock 試験） | decision-log.md、campaign.md、pilot.md |
| 3. test-parallel exit 0 | 達成 | 上表 |

## 実装済み
campaign 基盤（manifest/hash/split 固定、dry-run 見積り、runner、独立 checker、export、check-report）、credential 待ち導線、Jev 分類 adapter と daemon 配線（Config `[jev]`、ApiKeyProvider、tick 連携、reload 書き戻し、分類 info ログ）、固定方策と decision-assisted の統合（既定 fixed、故障時 fixed）、判断・消費ログと campaign export（migration 0081 credential_waits / 0082 decision_log）、mock E2E。

## 未検証（Jev 実 API は検証状態を分ける）
| 項目 | 状態 |
|---|---|
| 分類段が実経路で起動し実 endpoint へ試行する | 検証済み（22:31:30Z の `fell_back_to_fixed{client_error}`） |
| 故障時に固定方策へ戻る | 検証済み（final_lane = base_lane） |
| 実 Jev での分類**成功**（answers の実形 choice:{type,choice,confidence,probabilities}） | **未検証**（成功 0 件） |
| client_error の原因（HTTP status、402 残高不足か 400 schema か） | **未検証**（status が残らない。上記修正後に判別） |
| credential 待ち→通知→登録→再開の実経路 | **未検証** |
| 実 LLM での A/B/C 比較、Celeris 実行器、見積り係数の較正、統計的主張 | **未実施**（measure は 0 試行。pilot/measure の数値は mock・推定） |

## 延期項目
- Celeris 実行器（`Executor` の task 起票→RunMetrics 採取）と実測、係数較正。
- campaign_id / arm / routing_mode / Jev 欄の書き手の本接続、CoS・cluster 待機の消費行。
- Jev schema 修正（上記）と昇格後の live-smoke 再確認。

## 本番への移行（人 / 運用セッションが実施。worker は本番に触れない）
1. config 差分（`~/.config/celeris/config.toml`）: 既に `[routing] mode = "decision_assisted"` と `[jev] enabled = true` が入っている。追加の config 変更は不要。`endpoint`/`model_id` は既定（`https://openrouter.ai/api/alpha/decisions`、`typesafe/jev-1.14`）のまま。切り戻しは `[routing] mode = "fixed"`（または `[jev] enabled = false`）にして reload。消えた config 欄は無い。
2. 鍵登録: GUI『アカウント』で `jev_api_key` を登録（登録済み 22:12Z）。値はチャット・ログ・git に出さない。
3. release: `scripts/selfdeploy/release.sh <sha12> && scripts/selfdeploy/verify.sh <sha12>` で `verify.json` の ok を確認し、GUI『リリース』で昇格。SCHEMA_VERSION 82（0081, 0082 は追加のみ）。n-1-compat は SchemaTooNew で不合格が想定どおり。
4. 昇格後: Jev schema 修正の followup を入れた release で live-smoke（ready から 1 tick 以上おいた WU の `routing_decided.lane_assist.classifier_status`、daemon ログ `jev: classification ...`）。
5. rollback: `scripts/selfdeploy/rollback.sh`（previous を昇格し直す）。DB を戻すのは `--restore-db` のみ（昇格後の仕事は失われる。通常不要）。昇格前バックアップは `/local/celeris/state/backups/`。詳細 docs/ops/selfdeploy.md §5。

## 提案
- Jev schema 修正 unit を最優先（followup）。fake server に公式 schema 検査を足す。
- 402/4xx を status で区別できるログにして、昇格後に原因を確定する。
