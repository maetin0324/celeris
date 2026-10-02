# リポジトリの設定 `workspace.toml`

---
tasks: [01M3YBGM64RYPEY9NZANF79A0M]
---

案件のリポジトリは、ルートの `.config/celeris/workspace.toml` に実行環境、検査コマンド、文書の置き場を宣言できる。ファイルが無ければ既定値を使う。不正な TOML や未知の項目があれば警告して既定値を使う。実装は `crates/task-core/src/workspace_config.rs`。

```toml
[workspace]
name = "agent-platform"
description = "Celeris 本体"

[run]
mode = "host" # host（既定）または container

[commands]
setup = ["cargo fetch"]
check = ["cargo test --workspace", "cargo clippy --workspace -- -D warnings"]

[outputs]
docs = "docs"
deliverables = "."
```

| 項目 | 用途 |
|---|---|
| `workspace.name` | 案件への登録時に使う名前の候補 |
| `workspace.description` | 計画 run と worker の作業場所に出す説明 |
| `run.mode` | リポジトリの `run = auto` での host/container 選択。明示した `run` が優先 |
| `commands.setup` | worktree 作成後に一度実行。失敗時は run を始めない |
| `commands.check` | worker に示す検査コマンド。task にコマンドの受け入れ条件が無い場合は review でも実行 |
| `outputs.docs` | 人向け文書の根。既定 `docs` |
| `outputs.deliverables` | コード以外の成果物の根。既定 `.` |

コンテナを使う場合は `[run] mode = "container"` とし、`[container]` に `image` または `dockerfile`、必要なら `mounts` と `env` を指定する。イメージを省略すると Celeris 全体設定の既定イメージを使う。リモートの作業と専用の調査 adapter はホストで実行する。コンテナの設定は `crates/task-worker/src/container.rs` が読む。

```toml
[run]
mode = "container"

[container]
image = "celeris-worker:latest"
mounts = []
env = {}
```

人が読む文書を更新するときは `outputs.docs` の下に Markdown を置く。agent の進捗・ADR・調査記録はこのリポジトリでは `agent-docs/` に置く。タスクの worktree で編集し、取り込みはタスクの「変更」画面で確認する。既存文書の監査と整理は [Repository Documentation Maintenance](repository-documentation-maintenance.md) を参照。
