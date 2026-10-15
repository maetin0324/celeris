---
tasks: [01M4Z1SQTHG4DARR1NSDJKBN1D]
title: MLSys 最小実装 — fixed / decision-assisted の切替境界、Jev 差込、credential 待ち、締切必須と延期
---
# ADR 2026-10-15: MLSys 最小実装 — fixed / decision-assisted の切替境界、Jev 差込、credential 待ち、締切必須と延期

- 日付: 2026-10-15
- 状態: 採用（設計のみ。実装は後続 unit）
- 親 task: 01M4Z1QJX5FYBXTPSEF0WKB1CK（依頼原文が正本）
- 関連: ADR-0069（routing 4 層、D9 の `ShadowClassifier` 予約）、2026-10-11-task-routing-features（判断 record）、
  2026-10-11-task-wait-reasons、ADR-0080（browser の `waiting_for_auth`）、ADR-0030（secret）、ADR-0133（notice store）、
  2026-10-15-root-done-cos-summary、2026-10-11-breaking-changes-by-default
- 棚卸し・事実・クリティカルパス・後続 unit の受入条件: `agent-docs/progress/2026-10-15-mlsys-min/design.md`

## Context

MLSys 投稿（内的目標 2026-10-30）の主張は「異種で依存する研究ワークフローを Celeris が統合して人の調整負担を減らす」ことで、
独自の最適ルータの学習ではない。既製の decision model（Jev）で**意味的な要求だけ**を分類し、固定の保守的方策とコードの制約判定で
実行先を決める。routing の 4 層（Ownership / Harness / Model / Review）と source / model / account の分離は保つ。
既存の実測根拠表ルータ（`model_router`・`routing-eval`）は削除しない。

## Decision

### D1. 実験設定 `routing.mode = fixed | decision_assisted`（既定 fixed、可逆）

- 既定は **fixed**。`decision_assisted` は設定（config）で明示したときだけ有効で、設定を戻せば即 fixed に戻る。DB の migration も旧データの書き換えも要らない。
- **優先順位（上が勝つ）**: ① 人の明示（assignee・`tier:`・model 指定、ADR-0069 D1）→ ② 承認・安全制約（approval、db_guard、allowed_tiers、budget）→ ③ コード側の利用可否（account/source の残量・互換・同時実行）→ ④ decision-assisted の分類結果 → ⑤ fixed 方策。
  Jev は④だけで、①〜③を上書きできない。
- Jev の出力は **lane の選択入力（手順確定度など型付きの狭い分類）に限る**。実測根拠表（`routing_evidence`・`feedback`）の成功率に埋めない。
  confidence / probabilities は「Jev の分類の確信」であり、LLM の成功確率でも実行方策の propensity でもない（記録するが名前を分ける）。
- 構造化済みの事実（genre・check の有無・依存完了・job 終了・残予算）はコードで決め、Jev に聞き直さない。
- 差込位置は ADR-0069 の `ShadowClassifier`／`TaskFeatures` の境界。dispatcher・store の中では Jev を呼ばず、別の段で分類し結果を判断 record（`celeris.route-decision/1`）に記録して `RoutePolicy` が読む。
- 失敗時は fixed へ戻る: timeout・429・5xx・不正 schema・未知分類は bounded retry + circuit breaker のあと fixed。
  「fixed へ戻った」と「Jev の実 API 統合試験が済んだ」は別の記録にする。キー未登録のまま Jev 統合を検証済みにしない。
- 認証系（401/403）だけは D4 の credential 待ちへ接続する。429・予算不足を credential 要求にしない。

### D2. Jev の API 契約と model ID（公式一次資料。確認日 2026-10-15）

契約は下の公式ページで確認した範囲だけを書く。**料金・性能・context 長は固定せず**、config と版（確認日つき）で持つ。

| 項目 | 確認した内容 | 出典 |
|---|---|---|
| Decisions API | `POST https://openrouter.ai/api/alpha/decisions`。body: `model`、`state`（文字列／object の配列）、`questions`（名前 → `{type: choice\|noul\|score\|box, instructions, criteria}`）。認証は OpenRouter の API key（header 名は同ページに明記なし → 実装 unit が OpenRouter の一般 API 文書で確認する） | https://openrouter.ai/docs/guides/community/jev |
| 応答 | choice: 選択・各選択肢の確率・`confidence`。noul: yes の確率。score: 確率重み付き位置・各水準の確率・`confidence`。`usage.cost`（USD）を含む。理由・説明は返らない | 同上 |
| model ID | `typesafe/jev-1.14`（同ページで current）、`~typesafe/jev-latest`（最新追従の alias。**使わない**）、`typesafe/jev-1.13`（利用可）。応答の `model` は日付つき snapshot を返し得る（記録する） | 同上 |
| 別経路 | Cloudflare Workers AI: model `typesafe/jev`、`POST https://api.cloudflare.com/client/v4/accounts/<account>/ai/run`、body `{"model","input":{state,questions}}`、応答 `answers`・`usage`。ページ記載の context 窓は 32,000 token で、OpenRouter 記載の 262,144 と食い違う | https://developers.cloudflare.com/ai/models/typesafe/jev/ |
| 限界・注意 | OpenRouter 側: 約 32,000 byte 超の text と画像は long-context mode（遅く精度が落ちると記載）。Box は 1.13 で HTTP 400。rate limit・その他の error code は記載なし | OpenRouter 同上 |

- 固定する値（版つき config）: endpoint、model ID（`typesafe/jev-1.14` を初期値、pin）、question schema の版、prompt/state 組み立て規則の版、timeout・retry 上限・cache 上限。
- 経路は **OpenRouter を初期値**（既存の `[llm.sources]` / HTTP 基盤を再利用。SDK は入れない）。Cloudflare は代替で、採るなら別 source として足す（source/model/account の分離を保つ）。
- 料金記載（Cloudflare: 入力 1M token あたり 0.042 USD、出力 0。OpenRouter: 出力 token 無料、入力単価は別ページ）は確認日つきの参考で、`usage.cost` の実測を記録し、unknown を 0 扱いしない。
- 実装 unit は live 呼び出しの前に上記 URL を再取得して契約差異を確認し、差異は進捗に書く。TypeSafe 直営エンドポイント（第三者記事のみ）は一次資料で未確認のため採らない。

### D3. 判断・実行・成果の最小ログ

既存の判断 record（`celeris.route-decision/1`）と `RunMetrics`・events を再利用し、新しい保管系を作らない。足すのは、campaign / task / WU / run / decision の対応、
mode、Jev の model snapshot と prompt・policy の版、実際に実行した source/model/account/harness、除外・fallback 理由。
結果は「最初の推奨候補」に帰属させず実行候補に付ける。run 終了・WU 検証・campaign 合否は別ラベル。失敗・timeout・cancel は分母から除かない。再処理で二重計上しない（run id で冪等）。

### D4. credential 待ち（既存の blocked/wait に合わせる）

- **task の Status は増やさない。** 待つ WU／task は `Blocked`。理由名は既存の browser 登録待ちと同じ **`waiting_for_auth`**（`BrowserWaitReason::WaitingForAuth`）に揃え、
  `TaskWait`（task-wait-reasons）に「credential 待ち」を 1 種足して `secret_id`（例 `jev_api_key`）と用途を載せる（値は載せない）。承認待ち `waiting_for_approval` と混ぜない。
- 秘密の保管は既存の secret（ADR-0030: `secrets_dir`、`PUT /api/v1/secrets/{id}`、アカウント画面）だけ。別の保管系を作らない。worker・prompt・event・KB・git・ログには **secret id と fingerprint だけ**を出す。
  注入は adapter／broker の境界で行う（Jev 呼び出しは dispatcher の外の分類段で、worker に鍵を渡さない）。
- **要求**: Jev の鍵が必要になった時点で、未登録（`GET /api/v1/secrets` の `fingerprint` 無し）なら、待つ WU だけを `Blocked`(`waiting_for_auth`) に移し、
  受信箱 item と通知を 1 件出す。dedup key は secret id 単位（`UNIQUE(kind,key)`／notice の `group_key`）で、同じ鍵を待つ複数 task は 1 通知に束ね、待つ task 一覧を更新する。本文は用途・待っている task・アカウント画面へのリンク・「登録すれば自動で続く」。
  Discord へは現行の送信経路（CoS escalation 系の kind だけが webhook に出る）に載せる。通知経路の刷新はしない。
- **登録イベント**: secret の PUT 成功時に「secret_stored（id・fingerprint のみ）」を追記専用の event として足す。PUT が成功しても event を取り逃した場合に備え、dispatcher の tick が「待ちの secret id の fingerprint が待ち記録の値と違う」を見て回復する（reconcile。LLM を使わないので token は消費しない）。
- **検証**: 登録後、最小費用の認証確認（Jev への最小 1 呼び出し。有料測定の承認とは別）を行い、成功したら待ちを解決して同じ WU を再 queue／checkpoint 再開（`resume_key` と version の CAS で一度だけ）。
  401/403 は待ちを維持し、同じ通知を「鍵が無効／権限不足」に更新（新規通知を重ねない）。429・予算不足・5xx は再登録要求にせず通常の backoff。
- **キャンセル**: cancelled の task は、後から鍵が登録されても再開しない（解決時に Status を CAS で確認）。
- **再起動**: 待ちと解決状態は DB（wait の行と events）が正で、daemon 再起動後は tick で復元する。
- **登録 ≠ 承認**: 鍵の登録は当該連携の有効化だけ。測定予算の承認ではない（D6）。

### D5. 過去の ChatGPT 静的監査

原文は Celeris の repo・KB に**見つからず、原文未入手**。指摘内容は推測しない。依頼文が名指しした「routing-eval の集計識別／split」だけ現行コードで再確認した（結果は進捗 design.md §5）。

### D6. 締切に対する必須／延期

必須と延期の表は進捗 design.md §6。要点: 必須 = fixed/decision-assisted 切替、Jev adapter（mock で単体試験）、credential 待ち→自動再開、最小ログ、最小 campaign ランナーと A/B/C 比較計画、mock E2E。
延期 = 投稿用の動的クラスタ配置、オンライン学習・反実仮想評価、汎用知識来歴グラフ、別 decision model との横並び、CoS 通知の全面刷新、独自ルータ学習、大規模問題集。
実測（有料）は予算の決定 `measure-budget` が承認するまで始めない。

## Consequences

- 旧い仕組みの削除はしない（実測根拠表ルータは別機能として残る）。新規の config 欄 `routing.mode` は読み込みで検証し、未知の値は読み込みエラー（本番 config の書き換えは後続 unit の進捗「本番への移行」に書く）。
- 後続 unit は D1 の優先順位と D4 の状態名を変えない。変える場合は新しい ADR で理由を書く。
