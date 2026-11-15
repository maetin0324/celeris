---
title: 健全性v2 R6 cos-cli 接続設定の集約と未参照公開関数の撤去
tasks: [01M6TYVJT7EJ6WWHEF0EBH3JPS]
status: running
updated: 2026-11-15
---
# 健全性v2 R6 cos-cli

## 実装前 brief と owner

[設計判断と brief](../../adr/2026-11-15-cli-api-resolution.md) に記録した。
architecture-map の celerisctl、知識ベースの置き場、CoS チャット設定の owner に限定した。
credential の時計注入実装、KB の置き場 validator、matching の cluster 対応実装は変更しない。

## 項目ごとの判定

削除前に次を実行した（exit 0）。全8定義が各1件だけ見つかり、定義以外の参照は無かった。
`rg -n -w 'cos_harness_capability_warnings|cos_run_credential_issue|TOP_DIRS|project_slugs|page_file|unroutable_question|has_reply|next_version' crates web control-ts scripts docs config/celeris.example.toml`
対象ディレクトリ内の cfg・feature・試験・wire・文書・設定例にも同名の参照が無い。
部分一致の検索でも確認し、`cos_run_credential_issue_at` の API/dispatcher/試験からの呼び出しと、別機能の project slug migration は保持した。

| 項目 | 判定 | 根拠・残す契約 |
|---|---|---|
| celerisctl-api-endpoint-resolution | 寄せた | dup-candidates.json の files 全件を確認。ApiConfig::from_config は現行ソースに存在しなかったため追加。6 command の写しと cos_ops::api_config を撤去。knowledge と main の呼び出しも同経路へ。既定 config 定数も共有 |
| unref-cos-harness-capability-warnings | 消した | config/cos.rs の公開 method 1 件のみ。専用 private helper と import も撤去。harness 能力表そのものは維持 |
| unref-cos-run-credential-issue | 消した | chat/credential.rs の実時計 wrapper の定義のみ。時計注入版と credential 保存・失効の実装は維持 |
| unref-top-dirs | 消した | knowledge/layout.rs の TOP_DIRS 定義のみ。place validator と実際の許可規則は維持 |
| unref-project-slugs | 消した | Layout accessor 定義のみ。projects データと slug 検証は維持 |
| unref-page-file | 消した | knowledge.rs の wrapper 定義のみ。page_path の安全検証は維持 |
| unref-unroutable-question | 消した | matching.rs の wrapper 定義のみ。cluster 対応の質問生成は引き続き実呼び出しあり |
| unref-has-reply | 消した | ReviewState accessor 定義のみ。reply と review_state は維持 |
| unref-next-version | 消した | ProjectPlanState accessor 定義のみ。計画の版と承認状態は維持 |

API の wire、schema、events、履歴、credential 本体、認可の契約の廃止は無い。
未参照定義を直接固定する試験は無く、既存試験を削除・緩和していない。
追加の実 CLI 試験は cron/models/script の共通 resolver 利用と URL 環境変数優先、明示 URL 時に config/token を読まない不変条件を確認する。
既存 transport 試験は ApiConfig を直に作るため command の写しを検出できなかった。

## 挙動の差と本番への移行

- cron・integration・models・script は従来明示 config/既定だけだったが、共通経路で CELERIS_CONFIG と CELERIS_API_URL も読む。
- CoS の URL 明示/環境変数優先、URL 時は config/token を読まない規則、run credential 時は人の token を読まない規則を維持。
- browser doctor の明示 token file 優先と秘密を表示しない失敗表示を維持。空の CELERIS_API_URL は他 command と同じく未指定扱い。
- token の空文字/control character 拒否を共通 owner で行う。
- release notes の任意 HTTP lookup は --api（/api/v1 で終わる制約の無い別契約）と任意 token file を使う。dup-candidates の対象外で維持。
- knowledge の root 解決は API の接続先ではなく、壊れた config も許す独立した契約なので維持。
- 本番 config 変更・DB migration は不要。本番操作は行わない。

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）

- `bash scripts/dev/test-affected.sh --list` → exit 0。変更 crate と逆依存14 crate、web 型検査・unit、TS 検証を選択。
- `PNPM_CONFIG_STORE_DIR="$PWD/.pnpm-store" CELERIS_SCRIPT_RUNNER_CACHE="$CARGO_TARGET_DIR/script-runner-cache" bash scripts/dev/test-affected.sh` → 未完走（中断 exit 130）。health pass（新規0・悪化0）、web 型検査・unit714件・gateway113件、TS検証20件は通過。Rust 8528件の実行中、既存 CoS 試験が URL エラー文の指定元の欠落を検出した。修正して下記対象8件は通過。run の残り時間を考慮し全体の affected tests は引き継ぐ。
- `cargo nextest run --workspace -E 'package(=celerisctl) & binary(=cos_ops)'` → exit 0、8 passed。既存の URL 優先順・エラー文の試験と、追加の共通 resolver 実 CLI 試験を含む。host 共通枠を取得して実行した。
- `cargo clippy --workspace -- -D warnings` → exit 0。vendor deno_core の既存 deprecated warning 1件はあるが workspace の検査は合格。
- `cargo fmt --all -- --check` → exit 0。
- 文書リンク・ADR名・進捗書式検査 → 各 exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` と担当範囲の照合 → exit 0、18 path、範囲外0。K2/K4/IEEE Access の fixture・評価器・比較条件の変更0。

検査環境の解決: 共有 runner cache の読取専用を避け、`CELERIS_SCRIPT_RUNNER_CACHE="$CARGO_TARGET_DIR/script-runner-cache"` を指定した。
pnpm の offline store の不足は、先に offline install が失敗することを確認してから、試験とは別に依存を補充した。
pnpm v12 は `npm_config_store_dir` を参照しないため、`PNPM_CONFIG_STORE_DIR="$PWD/.pnpm-store"` を指定した。
試験の install は offline のまま。host 共通枠への書き込みには sandbox 外実行の自動承認を得た。枠の回避はしていない。
初回の health は main の整形による行数増加で失敗した。import と説明を整理し、関数の行数を元に戻して解決した。

## commit

- `phase 1: consolidate CLI API config and remove unused cos knowledge helpers`

## 未解決

実装と対象試験8件は完了。最終 source で affected tests 全体を完走し、その結果を追記して status を done にする必要がある。
再実行には上記の PNPM_CONFIG_STORE_DIR と CELERIS_SCRIPT_RUNNER_CACHE を指定する。sandbox 内では host 共通枠が read-only のため sandbox 外実行が必要（本 run は自動承認済み）。共有枠を迂回しない。人の判断を要する事項は無い。

## 提案

無し。
