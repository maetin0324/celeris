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
  - `bash scripts/dev/test-parallel.sh`（nextest。`cargo test --workspace` と同じ範囲で、release gate と同じ）と `cargo clippy --workspace -- -D warnings` を実行し、出力の要点を報告に含める
  - task の進捗ファイル `agent-docs/progress/YYYY-MM-DD-<slug>.md` を更新（front matter と完了日、証拠コマンドと結果、未解決事項、提案。並列 WorkUnit は `agent-docs/progress/YYYY-MM-DD-<slug>/<wu-key>.md`。ADR-0128 D3）。共有の索引・`agent-docs/PROGRESS.md` には追記しない
  - `git add -A && git commit -m "phase N: <summary>"`
- 仕様・設計の変更の提案は task の進捗ファイルの「提案」節に書く（ADR-0128 D3）
- 同じアプローチを3回失敗したら、task の進捗ファイルに状況を書き、報告本文で人間に質問する
- 試験・検査・検証で CPU を焼く負荷（busy loop、stress-ng、並走 cargo の負荷台本）をかけない。WU の check にも重い負荷の台本を置かない。時間依存の不具合は時計の差し替え（tokio::time::pause・注入した時計）、出来事待ち、対象 process への SIGSTOP/SIGCONT、試験専用の遅延フックで決定的に再現する（詳細は `agent-docs/guides/testing.md`）
- テストで外部ネットワークに出ない。LLM呼び出しを伴う実機確認は、認証が使える環境ならエージェントが実行し証跡を task の進捗ファイルに残してよい。使えなければ手順を書いて人間に依頼する（ADR-0009 P-34）

## 禁止
- ディスパッチャやストアにLLM呼び出しを入れる
- `unwrap()`（テスト以外）
- Web UI、リモート実行、予算管理の実装（別プロジェクト）
- タスクの状態を会話やワーカーのメモリに持たせる（真実は SQLite。ワーカーはステートレス）
- ワーカーの自己申告だけで完了にする（完了はレビュアーか決定的な検査が決める）
- events の書き換え・削除（追記専用）

## 完了報告の書き方
受け入れ条件ごとに「条件」「実行したコマンド」「出力の要点（exit code、テスト数、差分ゼロなど）」を箇条書きで本文に書く。
評価器はトランスクリプトしか見ないので、ファイルに書いただけでは完了と判定されない。