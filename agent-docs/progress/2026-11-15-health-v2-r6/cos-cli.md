---
title: 健全性v2 R6 cos-cli 接続設定の集約と未参照公開関数の撤去
tasks: [01M6TYVJT7EJ6WWHEF0EBH3JPS]
status: running
updated: 2026-11-15
---
# 健全性v2 R6 cos-cli

## 実装前 brief と owner

R6 の計画が API の解決を `ApiConfig::from_config` へ寄せると決め、挙動の差は進捗に書くと指定した。
この unit の範囲は `agent-docs/adr/` を含まないため、ADR は足さず、brief と挙動の差をこの進捗に書く。
前の run は ADR `2026-11-15-cli-api-resolution.md` を足したが、範囲 check で落ちたので消した。

1. 根本原因: celerisctl の command ごとに config・listen・token の解決の写しがあり、接続先と token の扱いが command で違っていた。
2. 責務 owner: architecture-map の celerisctl の行。接続設定は `commands/cron.rs` の `ApiConfig`、未参照の定義は各定義の module。
3. 既存の再利用先: `celeris::Config::load`、`api.client_token()`、cos_ops の URL の優先順、cron の HTTP transport と既存の試験。
4. 選択肢: 各写しを個別に直す案と、`ApiConfig` に 1 つの解決関数を置く案を比べ、後者を採った。base の時点で `ApiConfig::from_config` は無かったので足した。新しい共通基盤は作らない。
5. 新しい状態・分岐・fallback・port: 無し。既存の URL override と token file の解決を owner に移しただけ。
6. 撤去対象: 6 command の解決の写し、`cos_ops::api_config`・`API_URL_ENV`、未参照の 8 定義と、それ専用の private helper・import。
7. 不変条件: credential を表示・記録しない。CoS run credential があるときは人の token を読まない。明示 URL のときは config と token を読まない。時計注入の発行・KB の validator・review の判定は残す。既存の CoS・cron・browser の試験と、追加した共通 resolver の実 CLI 試験で確かめる。

`ApiConfig::from_config(path, url, token_file)` の優先順:

- URL: 引数の URL（`--api-url`）、`CELERIS_API_URL`（空は未指定）の順。どちらかがあれば `/api/v1` で終わることを求め、config と人の token を読まない。
- config: 引数の path、`CELERIS_CONFIG`、既定 `~/.config/celeris/config.toml` の順。`[api] listen` と `client_token` を読む。`CELERIS_COS_RUN_CREDENTIAL` があれば人の token を読まない。
- token file（browser doctor の `--token-file`）は最後に当て、token を上書きする。空や制御文字を含む token は拒む。

範囲は architecture-map の celerisctl、知識ベースの置き場、CoS チャット設定の owner に限った。
credential の時計注入の実装、KB の置き場の validator、matching の cluster 対応の実装は変えていない。

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

- `bash scripts/dev/test-affected.sh --list` → exit 0。変更した crate と逆依存の 14 crate、web の型検査と unit、ts-verify を選んだ。
- `PNPM_CONFIG_STORE_DIR="$PWD/.pnpm-store" CELERIS_SCRIPT_RUNNER_CACHE="$CARGO_TARGET_DIR/script-runner-cache" bash scripts/dev/test-affected.sh` → exit 0（911 秒）。health は verdict=pass（new 0・worsened 0）。web は 103 file・714 件 passed、gateway は 113 件 passed、ts-verify は 20 passed・0 failed、Rust は 8528 件 passed・23 skipped。
- `cargo nextest run --workspace -E 'package(=celerisctl) & binary(=cos_ops)'` → exit 0、8 passed（前の run）。既存の URL の優先順・エラー文の試験と、追加した共通 resolver の実 CLI 試験を含む。全体の affected tests にも含まれる。
- `cargo clippy --workspace -- -D warnings` → exit 0。vendor の deno_core の既存の deprecated warning 1 件は出るが、workspace の検査は合格。
- `cargo fmt --all -- --check` → exit 0。
- 計画の範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"` の path を許可 path と照合）→ exit 0、17 path、範囲外 0。K2・K4・IEEE Access の fixture・評価器・比較条件の変更は 0。

検査の環境: 共有の runner cache は読み取り専用なので、`CELERIS_SCRIPT_RUNNER_CACHE="$CARGO_TARGET_DIR/script-runner-cache"` を指定した。
pnpm v12 は `npm_config_store_dir` を読まないので、`PNPM_CONFIG_STORE_DIR="$PWD/.pnpm-store"` を指定した（`.pnpm-store` は `.git/info/exclude` で除外済み）。試験の install は offline のまま。

## 計画の check の不備（plan_issue として申告）

前の run の後、計画の check が 2 つ落ちた。

1. 範囲 check: 前の run が足した `agent-docs/adr/2026-11-15-cli-api-resolution.md` が範囲外だった。この run で ADR を消し、brief を上の節に移した。範囲 check は exit 0 になった。
2. 定義の撤去の check: `! grep -n 'pub fn cos_run_credential_issue' crates/task-core/src/chat/credential.rs` が前方一致のため、残すべき `pub fn cos_run_credential_issue_at(`（53 行目）に当たって exit 1 になる。
   `cos_run_credential_issue_at` は `crates/task-api/src/cos/mod.rs:77` と `crates/task-dispatch/src/dispatcher/cos_chat/launch.rs:713` が呼ぶ生きた API で、名前の変更や移動はこの unit の範囲外の file を変える。check だけを通すための改名や file の移動はしない。
   check を `grep -nE 'pub fn cos_run_credential_issue\b'`（または `'pub fn cos_run_credential_issue('`）に直せば、現在の HEAD で exit 0 になることを確かめた。

## commit

- `phase 1: consolidate CLI API config and remove unused cos knowledge helpers`
- `cos-cli: ADR を消して brief を進捗に移し、affected tests の完走の結果を書く`

## 未解決

計画の check 2 の前方一致の修正（replan）を待つ。実装・試験・範囲は完了している。人が判断する事項は無い。

## 提案

無し。
