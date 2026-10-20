---
title: workspace.toml・CLAUDE.md・文書を「全体の試験は release gate だけ」に合わせる
tasks: [01M5BZY0AMGFA150CPDPY1YBV1]
status: done
updated: 2026-10-20
completed: 2026-10-20
---
# workspace.toml・CLAUDE.md・文書の整合（WorkUnit docs-config）

完了日: 2026-10-20

## 変えたもの

- `.config/celeris/workspace.toml` の `[commands] check` を `["bash scripts/dev/test-affected.sh", "cargo clippy --workspace -- -D warnings", "cargo fmt --all -- --check"]` にし、コメントも直した。
- `CLAUDE.md`（`AGENTS.md` は symlink）の「各Phase完了時に必ず」を test-affected + clippy + fmt にし、全体の試験は release gate だけと明記した。
- `README.md`、`docs/guides/workspace.md`、`docs/ops/nextest.md`、`docs/ops/selfdeploy.md`（開発者向けの 1 文だけ）、`agent-docs/GOAL_TEMPLATE.md`、`.claude/agents/auditor.md` の「`cargo test --workspace` を必須とする」記述を直した。
- `.claude/skills` 配下に該当記述はなかった。

## 残した test-parallel 参照とその理由

- `scripts/selfdeploy/`・`release.sh`・`docs/ops/selfdeploy.md` の gate 説明: release gate は全体の試験のまま。
- `docs/ops/nextest.md` の導入手順、`.config/nextest.toml`、`docs/ops/build-tmp-hygiene.md`、`docs/architecture-map.md`: script 自体の説明で、必須化ではない。
- `agent-docs/ops/dev-builds-local-target-dir.md`: hook が target-dir を生成する対象コマンドの事実の記述。
- 過去の ADR・進捗・reports、`tests/e2e` の「`cargo test --workspace` で build される」メッセージ: 履歴 / 前提の説明。
- `crates/task-worker` の指示文: 並行 WU `prompts` の担当。

## 証拠

- `grep -n test-affected .config/celeris/workspace.toml` → check の先頭に出る。
- `cargo test -p task-core workspace_config`（解析試験）の結果は下記「検査」。

## 提案

- 起票定型の受け入れ条件から `bash scripts/dev/test-parallel.sh` を外す。cos-operator skill や `~/.claude` は本リポジトリ外なので触っていない。運用者・CoS が起票するときの定型を、`bash scripts/dev/test-affected.sh` に置き換える。

## 本番への移行

本番の `~/.config/celeris/workspace.toml` が（あれば）旧い check を持つ場合、運用者が書き換える。リポジトリ側の `.config/celeris/workspace.toml` は本変更で更新済みで、これは別物。

1. 確認: `grep -n -A3 '^\[commands\]' ~/.config/celeris/workspace.toml`（ファイルが無ければ作業は不要）。
2. `check` を次の値にする:
   `check = ["bash scripts/dev/test-affected.sh", "cargo clippy --workspace -- -D warnings", "cargo fmt --all -- --check"]`
3. 確認: `grep -c 'test-parallel' ~/.config/celeris/workspace.toml` が 0、`grep -c 'test-affected' ~/.config/celeris/workspace.toml` が 1。
4. daemon の再起動は不要（検査コマンドは run のたびに読まれる。読まれない場合は release の昇格時に反映される）。この作業は agent は行わない。

## 検査

- `cargo test -p task-core workspace_config` → exit 0、4 passed / 0 failed（workspace.toml の解析試験）。
- `cargo fmt --all -- --check` → exit 0。
- 変更は文書・設定のみ（crates 差分なし）。範囲 check（`CELERIS_WU_SCOPE_PATHS`）の出力は上記の変更 path 9 件だけ。
