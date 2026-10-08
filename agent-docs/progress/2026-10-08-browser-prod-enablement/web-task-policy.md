# web-task-policy: task 詳細の browser policy 確認・編集

---
tasks: [01M4CDNAYX6J68WTX7SKF0DJ64]
wu: web-task-policy
---

## 実装

- task 詳細の概要に browser policy を表示する。実行がまだ無い task も対象。許可サイト・日本語の許可操作・credential policy ID・追加承認対象を確認できる。
- 下書き・実行待ち・`browser_prerequisite` による停止中だけ編集する。待機理由は生成型の timeline event と最後の transition から判定し、認証・承認待ちの停止中は編集を出さない。
- 既存 `GET/PUT /api/v1/tasks/{id}/browser/policy` を `/api/tasks/{id}/browser/policy` の same-origin relay 経由で利用する。PUT 本文は policy 自体。revision を増やし、artifact policy・domain mode・navigation origins を保持、追加承認対象は選択された操作の部分集合を保持する。
- クリック・ダウンロード・credential_use の毎回承認は変更しない。ID/password の入力欄は設けない。
- 保存中はフォームを無効にし、ref による二重送信防止を行う。保存後に policy・task 詳細・timeline を再取得し、結果は画面内に残す。403・入力不備・状態競合・結果不明を区別する。結果不明の自動再送は行わず、再取得ボタンを用意する。
- 台帳などの前提不足で停止した task は全 tab 共通の先頭に、ledger-gate の event の message を表示する。`/browser/settings` と、画面内で開ける運用手順（doctor・台帳再生成・自動再開の確認、`docs/ops/browser-prod.md` の参照）への導線を設ける。
- API の生成型は既存の `BrowserAction`・task・timeline event を利用。policy の GET/PUT は schema に型が登録されていないため、既存 Rust の wire contract に対応する局所型を定義した。`node web/scripts/gen-types.mjs --check` で生成型の同期を確認した。

## 出自情報の制約（close-out への引き継ぎ）

依存の task-policy-auto 葉が記録しているとおり、ADR D4 の `BrowserTaskPolicySet { source }` event と task 詳細の出自欄は未実装。ledger-gate も task 詳細の prerequisite 欄を追加していない。本葉の制約は crates/ 変更禁止なので、これらの API 欄は追加していない。

- `policy_id = auto` は自動付与の識別子として表示する。同じ ID を残した過去の手動 PUT や retry の有無は識別できないことを明記する。
- 本画面からの保存は `policy_id = web-human` とし、人が web で編集した設定（retry の引き継ぎを含む）として区別する。その他は「既存 policy（出自は未記録）」と表示する。
- 完全な監査として auto / retry / human の出自を判別するには、後続の core/API で source event・欄を実装する必要がある。識別子による表示は承認や能力解放の判定には使わない。
- prerequisite は既存 event の seq 順から取得し、再開後・別の停止理由になった後は表示しない。

## 検証（2026-10-08）

| コマンド | 結果 |
|---|---|
| `corepack pnpm@12.6.0 -C web typecheck` | exit 0 |
| `corepack pnpm@12.6.0 -C web lint` | exit 0（既存 styles.css の !important 警告 4 件） |
| `corepack pnpm@12.6.0 -C web test` | exit 0、vitest 83 files / 604 tests、gateway 77 tests、全件合格 |
| `corepack pnpm@12.6.0 -C web build` | exit 0（既存の chunk size 警告） |
| `corepack pnpm@12.6.0 -C web check:boundaries` | exit 0 |
| `node web/scripts/gen-types.mjs --check` | exit 0 |
| `git diff --check` | exit 0 |
| `git diff --name-only "$CELERIS_WU_BASE" -- crates/` | 空（Rust 差分なし） |

新規 vitest 10 件: 保存前の policy 表示、未実行 task への配置、未設定・読み取り専用、GET、PUT 本文・revision・制約保持、policy 新規作成、失敗時の再送なし・文言、event 順・再開、編集可能状態、台帳理由と運用導線。

差分範囲は本葉の browser policy 関連 component/model/test、task 詳細への配置、この進捗ファイルのみ。Rust 全体の test-parallel / clippy は crates/ 差分がないため本葉では実行せず、全体の close-out に委ねる。本番 host 操作・デプロイ・push は実施していない。
