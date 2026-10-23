# agent-platform / Celeris

## 最初に読むもの（毎セッション）
1. `docs/SPEC.md` — 現行の仕様（正本）。設計原則の参照先
2. `agent-docs/README.md` — agent 向け文書の入口と読む順（ADR-0128）
3. `docs/architecture-map.md` — subsystem → crate/module → entry point → ADR の索引。1 つの変更で読む範囲を絞る
4. `agent-docs/adr/` — 過去の設計判断。矛盾する変更をしない
5. 進捗の索引 — `sh scripts/dev/progress-index.sh` が `agent-docs/progress/` から現在地（running・blocked を先に）を出す

## 作業の進め方
- 今回のPhaseだけをやる。次のPhaseの準備を先回りしない
- cargo / nextest はローカル LVM の target-dir を使う（`.cargo/config.toml` を `scripts/dev/worktree-target-dir.sh` が生成。PreToolUse hook が自動で書く。NFS の worktree に `target/` を作らない）
- 設計判断をしたら `agent-docs/adr/YYYY-MM-DD-<slug>.md`（日付+slug。番号は 0128 で止めた。ADR-0128 D5）を追加してから実装する
- 各Phase完了時に必ず:
  - `bash scripts/dev/test-affected.sh`（変更 crate と逆依存 crate の試験だけ。web/・docs/api/、task-api とその通常依存の crate（task-core の event kind 等）の変更では web の型検査（tsc -b）と unit 試験も走らせる。node_modules が無ければ先に offline で pnpm install。基準は `git merge-base HEAD main`、WU では `$CELERIS_WU_BASE`。`--list` で対象を確認）と `cargo clippy --workspace -- -D warnings`、`cargo fmt --all -- --check`（release gate の cargo-fmt-check と同じ。落ちたら `cargo fmt --all` で直す）を実行し、出力の要点を報告に含める
  - 全体の試験（`cargo nextest run --workspace` と doc test の一括・web の e2e・本物の browser/userns の試験）は release gate（`scripts/selfdeploy/release.sh`）だけで走らせる。WU・段の統合・task の受け入れ・review・Phase 完了では走らせない（`agent-docs/adr/2026-10-20-full-tests-at-release-only.md`）
  - task の進捗ファイル `agent-docs/progress/YYYY-MM-DD-<slug>.md` を更新（front matter と完了日、証拠コマンドと結果、未解決事項、提案。並列 WorkUnit は `agent-docs/progress/YYYY-MM-DD-<slug>/<wu-key>.md`。ADR-0128 D3）。共有の索引・`agent-docs/PROGRESS.md` には追記しない
  - `git add -A && git commit -m "phase N: <summary>"`
- 仕様・設計の変更の提案は task の進捗ファイルの「提案」節に書く（ADR-0128 D3）
- 同じアプローチを3回失敗したら、task の進捗ファイルに状況を書き、報告本文で人間に質問する
- 試験・検査・検証で CPU を焼く負荷（busy loop、stress-ng、並走 cargo の負荷台本）をかけない。WU の check にも重い負荷の台本を置かない。時間依存の不具合は時計の差し替え（tokio::time::pause・注入した時計）、出来事待ち、対象 process への SIGSTOP/SIGCONT、試験専用の遅延フックで決定的に再現する（詳細は `agent-docs/guides/testing.md`）
- テストで外部ネットワークに出ない。LLM呼び出しを伴う実機確認は、認証が使える環境ならエージェントが実行し証跡を task の進捗ファイルに残してよい。使えなければ手順を書いて人間に依頼する（ADR-0009 P-34）

## 変更の方針（破壊的変更を是とする）
- 詳細は `agent-docs/adr/2026-10-11-breaking-changes-by-default.md`。Claude 以外の agent も同じ。この `CLAUDE.md` が必須規則の唯一の正本。Claude Code は直接読み、Codex・Pi・ACP の run には worker がこのファイルの全文を prompt の先頭に写す（`crates/task-worker/src/required_rules.rs`、`agent-docs/adr/2026-10-23-required-rules-prompt-delivery.md`。作業ツリーには書かない。読めない・空なら run は落ちる）。この repo は `AGENTS.md` を追跡しない（Codex の skills 一覧だけが生成する）。規則の写しとして `AGENTS.md` を手で作らない
- 仕組みを置き換えるときは、同じ変更で旧い仕組み（コード経路・config 欄・API field・画面・文書・旧挙動の試験）を消す。deprecated・互換分岐・警告だけ出して無視・新旧切替 flag を残さない
- 失敗した変更は git（revert・cherry-pick）と release の rollback で救出する。「念のため」旧い仕組みを残さない
- 消した config 欄は読み込みエラーにし、本番 config の書き換えは進捗ファイルの「本番への移行」節に書く
- データは失わない: DB は migration で移す、events は追記専用、credential は消さない
- review では旧い仕組みの残りを不備として指摘する

## 禁止
- ディスパッチャやストアにLLM呼び出しを入れる
- `unwrap()`（テスト以外）
- タスクの状態を会話やワーカーのメモリに持たせる（真実は SQLite。ワーカーはステートレス）
- ワーカーの自己申告だけで完了にする（完了はレビュアーか決定的な検査が決める）
- events の書き換え・削除（追記専用）

Web UI（`web/`）・ssh によるリモート実行（ADR-0018）・予算/quota の扱いは実装済みの現行機能で、禁止ではない。初期 ADR（0001 等）や旧 DESIGN の「Web UI 禁止」「非目標」は当時の範囲を記した歴史的記録で、現行規則はこのファイルと `docs/SPEC.md`。

## 完了報告の書き方
受け入れ条件ごとに「条件」「実行したコマンド」「出力の要点（exit code、テスト数、差分ゼロなど）」を箇条書きで本文に書く。
評価器はトランスクリプトしか見ないので、ファイルに書いただけでは完了と判定されない。