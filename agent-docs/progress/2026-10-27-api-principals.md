---
title: API の主体を token ごとに区別する
updated: 2026-10-27
status: running
tasks: [01M5XXF7Q3N2619D5RQA5J7C7N]
---
# API の主体を token ごとに区別する

完了日: 未完了。
基準: `git merge-base HEAD main`。

## 変更

- [ADR](../adr/2026-10-27-api-principals.md) に認証由来の principal と内部 MCP token、移行の境界を記録。
- 中断前の未コミット変更を確認。task-api の principal / middleware / CoS / 一部 task handler は変更途中。
- 2026-10-27 の人のコメントを受領。本番 config の変更直前に operator が利用者へ知らせる手順を下記に固定。
- コミットはまだ作成していない。Phase 完了の報告ではない。

## 証拠

- `git status --short` / `git diff --stat` → exit 0。再開時点で task-api の追跡済み 9 ファイルと未追跡 principal.rs に変更あり。
- `sh scripts/dev/progress-index.sh` → exit 0（作業開始時の索引確認）。
- `sh scripts/dev/check-doc-links.sh` / `sh scripts/dev/progress-index.sh --check` / `git diff --check` → exit 0（文書追加後）。
- 必須の `bash scripts/dev/test-affected.sh`、`cargo clippy --workspace -- -D warnings`、
  `cargo fmt --all -- --check` は未実行。提出時の HEAD での合格はまだ主張しない。

## 本番への移行

この節は operator が昇格前に読む手順。**実装と受け入れ検査が完了するまでは実行しない。**
この run は本番 config・DB・service・credential を変更していない。

1. operator は変更の直前に利用者へ「共有 API token を web / operator に分割し、MCP は内部 token にする。
   LLM proxy の token 設定も独立させる。daemon と BFF の切替中は一時的に API が使えない」と知らせる。
   告知した日時と対象 instance を運用記録に残す。秘密の値は通知・記録に含めない。
2. 実際の config、BFF の service/env、operator/CLI、selfdeploy、langmem、LLM proxy 利用者の
   token_file 参照先を確認する。config と各 env のバックアップを権限 0600 で保管し、戻す旧 release を確認する。
   旧 token は credential の保全のため削除しない。既存 events の human を推測で書き換えない。
3. 各用途に独立した token を発行する。例えば config ディレクトリを確認した上で、次の方法で
   新しいファイルへ直接書く（既存ファイルがあれば中止し、標準出力へ秘密を出さない）。

   ```sh
   python3 - <<'PY'
   import os, pathlib, secrets
   config_dir = pathlib.Path(os.environ['CELERIS_CONFIG_DIR'])
   for name in ('api-web.token', 'api-operator-claude-code.token', 'api-operator-deploy.token'):
       fd = os.open(config_dir / name, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
       with os.fdopen(fd, 'w') as f:
           f.write(secrets.token_hex(32) + '\n')
   PY
   ```

4. `[api] token_file` を削除し、その他の api 設定を維持して次の配列を追加する。
   operator が複数なら、その数だけ別の principal と別のファイルを登録する。

   ```toml
   [[api.tokens]]
   principal = "human:web"
   token_file = "api-web.token"

   [[api.tokens]]
   principal = "operator:claude-code"
   token_file = "api-operator-claude-code.token"

   [[api.tokens]]
   principal = "operator:deploy"
   token_file = "api-operator-deploy.token"
   ```

5. LLM proxy の現行 Bearer を保全したまま `[llm_proxy] token_file` に参照先を設定する。
   proxy だけを利用する client の秘密を今回不要に更新しない。この旧 Bearer は api.tokens に登録しない。
   langmem は新しい proxy 設定を参照する実装が入ったことを確認する。
6. BFF 起動 env の `CELERIS_API_TOKEN_FILE` を `CELERIS_WEB_API_TOKEN_FILE` に置き換え、
   api-web.token の絶対パスだけを渡す。operator/CLI は各自の token ファイル、selfdeploy は deploy 用を参照する。
   BFF へ operator token の値・パス一覧や daemon config を渡さない。
   MCP は daemon 起動時の内部 token を使うため、MCP client の credential を再発行しない。
7. 新 release の設定検証と staging 検証の成功を確認して、通常の昇格手順で daemon / BFF を切り替える。
   実行する config 検証・昇格コマンドは実装完了時にこの節へ追記する。
   `/api/v1/health`、web の読み取り、operator の読み取り、MCP の api_get が成功することを確認する。
   合意した確認用 task へ web と operator からコメントを 1 件ずつ投稿し、event の actor が
   human:web / operator:claude-code に分かれることを API で読む。CoS は thread 付きの actor を確認する。
   新 token の値を shell 引数・trace・ログ・画面に出さない。
8. 失敗時は利用者へ状況を知らせ、旧 release と保存した config / BFF env / client 参照先を一緒に戻す。
   新形式 config のまま旧 binary を起動しない。DB の既存 events・credential は削除しない。

## 未解決

- ApiSettings の token → tokens の変更が呼び出し元に未反映。現在の変更はビルド可能な完成品ではない。
- config と CLI 上書き、LLM proxy、保護対象 token の複数化、MCP 内部 token 配線、BFF、全 handler、
  fixture・現行文書の更新、統合試験と必須検査を完了する必要がある。
- 移行手順の具体的な検証・昇格コマンドは完成した CLI と照合して追記する。

## 提案

なし。今回の認証主体の区別を完了させ、別の権限体系の導入には広げない。
