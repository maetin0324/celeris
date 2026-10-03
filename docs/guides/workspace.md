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
image = "ghcr.io/…/rust-dev:1.90"            # か dockerfile = ".config/celeris/Dockerfile"
mounts = ["/dev/infiniband:/dev/infiniband"] # 追加のマウント（そのまま runtime に渡る）
env = { CARGO_TARGET_DIR = "/w/.cargo-target" }
```

### 7.1 いつホストにするか（大事）

**迷ったらホスト**（既定）。コンテナにするのは「ツールチェーンをホストに置きたくない」ときだけである。
次のものは**ホスト実行**（リポジトリの `run` を `host` に）にすること:

- **ホストのデバイスが要るもの**（benchfs の InfiniBand、GPU、`/dev/fuse`、特権が要る計測）。
  `[container] mounts` でデバイスを渡すこともできるが、権限と `cgroup` の都合でまず壊れる。
  デバイスが要るなら最初から `run = host` が正しい。
- **リモートのクラスタで動かすもの**（ADR-0018 / 0019 の (a)）。リモートのタスクは常にホスト扱い。
- **`paperqa` / `local-deep-research` のタスク**。この 2 つは**設定に関わらず常にホスト**で走る
  （道具立てがホストの venv にあり、コンテナに持ち込むと別物になる。ADR-0043 Phase 56 追記）。

### 7.2 何がどう見えるか

タスクごとに `<runtime> run --rm -i --network host …` が 1 つ起きる。中身は決定的:

| 何 | どうする | なぜ |
|---|---|---|
| uid | podman: `--userns=keep-id` / docker: `--user <uid>:<gid>` | マウントした worktree にホストと同じ持ち主で書けるように |
| タスクのディレクトリ | `<workspace_root>/<task_id>` を**同じパス**で読み書き可 | worktree・`artifacts/`・`runs/` が前置きに書いたパスのまま見える |
| `dir` のリポジトリ | シンボリックリンクの**実体**を同じパスで読み書き可 | リンクはコンテナの中では辿れないため |
| 認証情報 | `CLAUDE_CONFIG_DIR` / `CLAUDE_SECURESTORAGE_CONFIG_DIR` / `CODEX_HOME` / `OPENCODE_CONFIG` の指す場所を**同じパスで読み取り専用** | アダプタが env に書いた場所だけを、書けない形で渡す（ADR-0024 のアカウントプールが選んだものだけ） |
| ネットワーク | `--network host` | 設定された LLM source の API（celeris proxy を使う場合はその listen アドレス）に届く必要がある |
| cwd | `-w <repos[0] の作業ツリー>` | ホスト実行と同じ場所 |
| 環境変数 | `HOME=<task_dir>` → アダプタの env → `[container] env`（後勝ち） | ホームは**マウントしない**ので、書ける HOME をタスクのディレクトリに置く |
| 目印 | `--label celeris.task=<task_id>` | 取り残したコンテナをラベルで消せるように |

**見せないもの**: ホームディレクトリ、`~/.local/celeris`、`~/.local/celeris` の根。必要なものだけを同じパスで渡す。

### 7.3 イメージ

1. `[container] image` — **そのまま使う**（Celeris はビルドもプルもしない。手元に無ければ runtime が引く）。
2. `[container] dockerfile`（リポジトリ相対。例 `.config/celeris/Dockerfile`）— **Dockerfile の内容と
   `.config/celeris/` の中身の sha** からタグ `celeris-ws-<sha12>` を作り、`~/.local/celeris/containers/<tag>/`
   でビルドしてキャッシュする。**同じ内容なら再ビルドしない**（`<runtime> image inspect` で見る）。
   文脈に送るのは `.config/celeris/` の写しだけで、リポジトリ全体ではない。ビルドは
   `<runtime> build --network host`（run と同じネットワーク。入れ子のコンテナでは `RUN` が
   ブリッジを張れないため）。
   記録は `<task_dir>/runs/container-build.log`。上限は `[containers] build_timeout_secs`（既定 1800 秒）。
3. どちらも無い — `[containers] image_default`（既定 `celeris-worker:latest`）。中身は
   `deploy/containers/celeris-worker/Dockerfile`（Debian stable slim + node LTS と claude/codex CLI +
   rust stable + python3/uv + git・gh・rsync・ssh）。作るのは `scripts/containers/build-worker.sh`
   （**`cargo test` の一部ではない**。ネットワークに出るので人が 1 度叩く）。

タスクが複数のリポジトリを使い、複数がコンテナを要求したときは、**primary → `repos[0]` の順で最初に
要求したもの**のイメージ・`mounts`・`env` を使う（環境は混ぜない。ADR-0043 D3）。

### 7.4 `setup` もコンテナの中

`[commands] setup` は worktree を作った直後に一度だけ走るが、**コンテナのタスクではコンテナの中で走る**
（ADR-0043 D3）。`runs/setup.log` の 1 行目に `# 実行環境: コンテナ <image>（<runtime>）` が出る。

### 7.5 runtime が使えないとき

`[containers] runtime`（既定 `auto` = podman → docker）を celeris が起動時に 1 度だけ `<runtime> info` で
確かめる。結果は `GET /api/v1/daemon` の `containers` とログに出る。**どれも使えなければ、コンテナが要る
タスクは run を始めずに `blocked` になり**、人に質問が積まれる:

> コンテナ runtime が使えません（`benchfs` が `run = container` を要求しています）: podman: … / docker: …。
> `[containers] runtime` の設定か、podman / docker の用意を見てください。
> ホストで走らせてよければ、そのリポジトリの `run` を `host` にしてください。

イメージのビルドが落ちたときも同じ経路（`blocked` + 質問）で、記録は `runs/container-build.log` にある。
`POST /api/v1/tasks/{id}/answer` で答えると次の run から再開する。

## 8. 文書（ADR-0044 D7。Phase 57）

**案件の文書の正本は git のファイル**である（DB には何も持たない）。置き場は

```

人が読む文書を更新するときは `outputs.docs` の下に Markdown を置く。agent の進捗・ADR・調査記録はこのリポジトリでは `agent-docs/` に置く。タスクの worktree で編集し、取り込みはタスクの「変更」画面で確認する。既存文書の監査と整理は [Repository Documentation Maintenance](repository-documentation-maintenance.md) を参照。
