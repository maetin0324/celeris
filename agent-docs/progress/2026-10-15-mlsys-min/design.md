---
title: "MLSys 最小実装の設計（棚卸し・切替境界・credential 契約・クリティカルパス）"
tasks: [01M4Z1SQTHG4DARR1NSDJKBN1D]
status: done
updated: 2026-10-15
completed: 2026-10-15
---
# MLSys 最小実装の設計（unit adr）

決定は ADR `agent-docs/adr/2026-10-15-mlsys-min-decision-assisted.md`。ここは事実・表・計画。実装はしていない（コードの変更なし）。
base は main `444308ac`。

## 1. 再利用／差分の棚卸し

| 領域 | 既存（再利用） | 差分（足すもの） |
|---|---|---|
| routing 4 層・lane | ADR-0069、`model_router`（`TaskFeatures`→lane、`ShadowClassifier`＝ADR-0069 §9 の予約。コード上は `LaneDecision.shadow` 相当の予約のみで実装の trait は見つからなかった） | `routing.mode`（fixed/decision_assisted）と優先順位の強制。実測根拠表ルータは**触らない** |
| 判断 record | `celeris.task-features/1`・`route-options/1`・`route-decision/1`・`route-outcome/1`（ADR 2026-10-11-task-routing-features。Jev 等の自然言語入力・選択肢・確率を持てる形） | Jev の model snapshot・prompt 版・mode の欄、fallback 理由 |
| 7-model 評価・根拠表 | `crates/routing-eval`（manifest・run・aggregate・recheck・export）、`routing_evidence` | 今回は拡張しない。split/集計の確認だけ（§5） |
| wait / blocked | `Status::Blocked`、`task-core/src/wait.rs` の `TaskWait`、browser の `waiting_for_auth`/`waiting_for_approval`（`browser_wait.rs`、ADR-0080）、`BrowserRunState` | `TaskWait` に credential 待ちを 1 種、`secret_id` と用途 |
| secret / 画面 | `PUT/GET/DELETE /api/v1/secrets[/{id}]`（`task-api/src/handlers/secrets.rs`、ADR-0030、値は応答・ログに出さず fingerprint のみ、`used_by`）、web のアカウント系画面（web-spa-phase4） | Jev 用 secret id と `used_by` 表示、PUT 成功時の event、画面から待ち task への導線 |
| 通知 | `notifications`（`UNIQUE(kind,key)`）、notice store（ADR-0133）、受信箱、root 完了まとめ（2026-10-15-root-done-cos-summary）。現行 daemon では CoS escalation/fallback 系だけが webhook（Discord）へ出る | credential 待ちの受信箱 item と secret id 単位の dedup。Discord は既存の CoS 経路に載せる（刷新しない） |
| 実行 metrics | `RunMetrics`・`execution_metrics.rs`・`execution_metrics_comparison_tests.rs` | campaign 単位の export、Jev 呼び出しの cost／token を含める。cost の欠測・推定ラベル |
| 評価ランナー | routing-eval の manifest/hash/split、Benchfs/Pluvio の既存試験・成果物 | 最小 campaign ランナー（通常実行経路を包む）、A/B/C の manifest、独立 checker |

## 2. 切替境界

ADR D1 の優先順位（人の明示 > 承認・安全 > コードの利用可否 > Jev 分類 > fixed）。Jev は lane の入力にだけ入り、根拠表には書かない。
fixed の方策は既存の lane 規則（cheap = 手順明確・機械検証可能・可逆・影響小、通常 = standard、未決定設計・難しい原因調査・高コストなレビュー = frontier／replan）を投稿用に版固定する。
lane → model/source は現在の設定を初期値にし、許可・予算は拡張しない。

## 3. Jev の公式一次資料（確認日 2026-10-15）

- https://openrouter.ai/docs/guides/community/jev — Decisions API `POST https://openrouter.ai/api/alpha/decisions`、model `typesafe/jev-1.14`（current）／`typesafe/jev-1.13`／alias `~typesafe/jev-latest`、question 型 choice/noul/score/box、応答は確率・confidence・`usage.cost`、理由は返らない。
- https://developers.cloudflare.com/ai/models/typesafe/jev/ — 別経路。model `typesafe/jev`、`answers`/`usage`、context 窓の記載が OpenRouter と食い違う（32,000 vs 262,144）。
- 料金・性能・context 長は固定しない。未確認: OpenRouter の認証 header 名、rate limit・error code、`/api/alpha` の安定性（alpha のため版固定と契約再確認を実装 unit の入口にする）。

## 4. credential 待ちの契約（unit credential-wait の入力）

| 論点 | 契約 |
|---|---|
| 状態 | Status は `Blocked`。理由名 `waiting_for_auth`（browser と同名）。`TaskWait` に credential 種。承認待ちと混ぜない |
| 保管 | 既存 secrets_dir のみ。event・通知・待ち行は secret id と fingerprint だけ |
| 通知 | secret id 単位で 1 件（`UNIQUE(kind,key)`／`group_key`）。待つ task は更新。本文は用途・待つ task・アカウント画面リンク・自動再開の説明。鍵は絶対に載せない |
| 登録イベント | PUT 成功時に secret_stored（id・fingerprint）を追記。取り逃しは tick の fingerprint 比較で reconcile（LLM・polling token なし） |
| 検証 | 最小費用の認証確認 1 回。成功→解決、401/403→待ち維持＋同通知を更新、429/予算/5xx→通常 backoff（再登録要求にしない） |
| 自動再開 | `resume_key`＋version CAS で一度だけ。重複 event は no-op |
| キャンセル | cancelled は再開しない（解決時に Status 確認） |
| 再起動 | DB（待ち行・events）から復元 |
| 承認との分離 | 登録は連携の有効化のみ。測定予算の承認ではない |
| 区別 | 「fixed へ fallback」と「Jev 実 API 統合試験済み」は別の記録。キー未登録で後者を立てない |

## 5. 過去の ChatGPT 静的監査（原文未入手）と routing-eval の再確認

原文は Celeris の repo・KB・親 task に無く、**原文未入手**。指摘内容は不明のまま推測しない。依頼文が挙げた論点を現行コード（`444308ac`）で確認した結果:

- `EvidenceKey`（`crates/task-core/src/routing_evidence.rs:8`）は genre・harness_id・harness_version・source_id・model_id の 5 欄。**manifest hash と split を含まない**。
  split の絞りは `aggregate.rs` の dev 限定フィルタ（`p.split == "dev"`、約 :384）に依存する。dev と最終評価、hash の違う結果を同じ cell に混ぜない保証は key では無く運用側にある → 後続 unit `campaign` で、routing-eval の再利用箇所に「別 manifest／split の trial は同じ cell に集計されない」回帰試験を足す（要確認事項として残す。今回は修正しない）。
- `aggregate::build` は `eval_status` が `error`/`cancelled` の trial を合否の分母から除き、`ExcludedCount` に数える（`excluded_status`、:40）。根拠表としては意図した除外だが、
  **system 評価では失敗・timeout を分母から除かない**要件と逆になり得る → campaign の集計は routing-eval の `aggregate` を流用せず、除外 0 の別集計か、除外数を必ず併記する。
- `build` の `version` は manifest sha256 と実行日（`now_utc().date()`）で、再実行の byte 一致を日付が崩す。campaign の保存物には manifest hash と明示の版を使う。
- 問題ごとの合否は `by_problem` の先頭 trial（`trial_no` 最小）で決める。再試行を許す campaign では「最初の試行のみ」を使う意味を明記する。
- 上記は静的な読み取り。実行・修正はしていない。

## 6. 必須／延期

| 項目 | 区分 | 理由 |
|---|---|---|
| fixed/decision-assisted 切替（既定 fixed、可逆） | 必須 | 比較 B/C の前提 |
| Jev adapter（mock で単体試験、bounded retry・circuit breaker、fixed 復帰） | 必須 | キー無しで前進 |
| credential 待ち→通知→登録→検証→自動再開・冪等・cancel・再起動 | 必須（ユーザー明示） | 実鍵が要る時にユーザーが動ける通知 |
| 最小ログ（decision/run/outcome 対応）、RunMetrics の campaign export | 必須 | 比較の根拠 |
| 最小 campaign ランナー・A/B/C manifest・独立 checker・dry-run・予算見積り | 必須 | 少数 pilot まで |
| mock E2E（再起動・依存待ち・障害・予算上限） | 必須 | 実 LLM を節約 |
| 実 Jev 接続スモーク（小）／pilot | 必須（キー登録後） | 統合の実証 |
| 承認済みの実測 | 条件付き（`measure-budget` の決定待ち） | 予算の包括承認にしない |
| 動的クラスタ配置、オンライン学習・反実仮想評価、汎用知識来歴グラフ、別 decision model 横並び、CoS 通知の全面刷新、独自ルータ学習、大規模問題集、TypeSafe 直営経路 | 延期 | 依頼の範囲外 |

## 7. クリティカルパス

| 日付 | 到達点 |
|---|---|
| 10/15 | adr（本 unit）。次に credential-wait ∥ jev-adapter ∥ decision-log |
| 10/17 | 3 並列 unit 完了、policy・campaign 着手 |
| **10/18** | **小さい統合**: mode 切替＋mock の Jev＋credential 待ち／再開が mock E2E で通る、release-1 の準備 |
| 10/19 | release-1 を人が昇格（GUI のリリース画面）。実キー登録を依頼（live-smoke の待ち） |
| **10/20** | **pilot と評価条件固定**: dry-run・予算見積り・少数 pilot、manifest/hash/split/方策・prompt・model 版を固定。`measure-budget` を人に提示 |
| **10/21〜26** | 承認済みの測定のみ（A/B 主比較、B/C は事前選定の小 subset） |
| **10/27 以降** | 新機能停止（修正・集計・文書のみ） |
| 10/30 | 内的目標。正確な提出時刻は必要な時点で公式確認 |

阻害要因: 実キー登録（人）、`measure-budget` の決定（人）、release の昇格（人）。キー待ちの間も mock 系 unit は止めない。10/20 に達しなければ、追加実装でなく延期項目と最小の阻害要因を通知する。

## 8. 後続 unit の受入条件

| unit | 受入条件（check の目安） |
|---|---|
| credential-wait | `Blocked`＋`waiting_for_auth` の待ちが secret id 単位で 1 通知。ダミー鍵登録→mock 認証成功→同 WU が 1 回だけ再開。誤キー→待機継続→更新で再開。重複 event・再起動・cancel・reconcile・ログ秘匿（鍵の値が event/通知/log に出ない grep 検査）の試験。`cargo test -p` 該当 crate と clippy が exit 0。新しい秘密保管系が無い |
| jev-adapter | endpoint・model ID・schema/prompt 版・timeout/retry/cache 上限が config。mock server で成功・不正 schema・timeout・401・429・5xx の試験。fallback は fixed。401/403 のみ credential 待ちへ。鍵なしで単体試験が通り、「統合試験未実施」が記録に残る。契約は §3 の URL を再取得して確認 |
| decision-log | decision/run/outcome の対応・mode・model snapshot・版・fallback 理由が record に載る。confidence を成功確率／propensity と別名で保存。二重計上しない試験 |
| policy | 既定 fixed。明示 model・承認・安全制約が Jev に勝つ試験。mode を戻せば元の挙動（差分ゼロ）。unknown の quota/cost を 0 扱いしない試験。根拠表ルータの試験は無変更で通る |
| campaign | manifest/hash/split の保存、試行ごとの workspace/KB 分離、A/B/C の manifest、dry-run、予算見積り、独立 checker（再計算・数値一致）。失敗・timeout を分母に残す。別 manifest/split が混ざらない回帰試験（§5） |
| mock-e2e | 再起動・依存待ち・障害・予算上限を mock で通す。実 LLM なしの結果を実性能として報告しない |
| release-1 / live-smoke / pilot / measure / final | 既存の release 手順。live-smoke は実鍵登録後に最小呼び出し 1〜数回。measure は `measure-budget` の承認範囲内のみ。final は実装済み／未検証を分けて報告 |

## 証拠

- 棚卸し: `crates/task-core/src/{wait.rs,browser_wait.rs,routing_evidence.rs}`、`crates/task-api/src/handlers/secrets.rs`、`crates/routing-eval/src/aggregate.rs` を読んだ（実行なし）。
- Jev: 上記 2 URL を 2026-10-15 に取得。
- 文書検査は完了報告に記載。

## 未解決事項

- OpenRouter の認証 header 名・rate limit・error code（公式ページに無し）。
- `ShadowClassifier` のコード上の実体（ADR にのみ確認）。
- ChatGPT 静的監査の原文（人が持っていれば共有を依頼）。

## 提案

- 実測前に routing-eval 集計の split/hash 混在の回帰試験を足す（campaign unit）。
