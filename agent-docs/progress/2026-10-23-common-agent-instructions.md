---
title: 全 harness が共通の必須規則に届くよう指示の正本と配布経路を揃える
tasks: [01M5KWPP3YPZMZ71PZP6HAGNFD]
status: done
updated: 2026-10-23
completed: 2026-10-23
---

# 共通 agent 指示の正本と配布

基準 SHA: 13d64acd1780（main、着手時）。検証対象は本 task の branch の最終 commit。

## 着手時の実測（fresh worktree）

- Claude Code: tracked の `CLAUDE.md` が worktree に出る。直接読まれ、届く（変更不要）。
- Codex: `AGENTS.md` は `.gitignore` の `/AGENTS.md` で追跡されず、fresh worktree には無い。旧 `deliver_agents_md` は skills 一覧の節だけを書くので、
  `CLAUDE.md` の必須規則は届かなかった（実測: 旧コードは `CLAUDE.md` を一切読まない）。
- Pi・ACP: skills は `.agents/skills` と前置きにだけ載り、規則を届ける経路が無かった。
- `CLAUDE.md` の「AGENTS.md は symlink」は誤り（symlink を作る経路が無い）。「Web UI／リモート実行／予算管理の実装禁止」は実装済み機能と衝突。
  `workspace.rs` の「LocalWorkspace のみ／RemoteWorkspace 骨組み」は `SshWorkspace`（ADR-0018）と食い違い、`RemoteWorkspace` は `unimplemented!` の死に体だった。

## 変更

- `skills::deliver_required_rules`（新規）: 作業場所直下の `CLAUDE.md` を `AGENTS.md` の `celeris:rules` 節へ写す。区切り外のユーザー内容を保つ・冪等。
  `codex.rs`・`pi.rs`・`acp.rs` の run 開始時に呼ぶ。`CLAUDE.md` が無い repo は何もしない。参照先なし（壊れた symlink）・空・書けない・読み戻して節が無い、は
  `Err` で run が落ちる（従来の skills 一覧の失敗は警告のまま）。
- `CLAUDE.md`: AGENTS の説明を正本＋配布経路に直し、実装済み機能と衝突する禁止項目を削り、歴史的 ADR と現行規則の区別を明記。
- `workspace.rs`: 説明を実装に合わせ、使われていない `RemoteWorkspace`・`WorkspaceError::RemoteUnsupported` を削除（破壊的変更方針）。
- `docs/architecture-map.md`: 「共通 agent 指示と配布」節（入口・不変条件・関連試験）。`agent-docs/README.md` に正本の一文。

## 検証（コマンドと結果）

- `cargo test -p task-worker --lib skills`: 44 passed（`required_rules_*` 4 件と、実 repo の CLAUDE.md が git 追跡され AGENTS.md は追跡されず、
  fresh な作業場所へ各経路で同じ規則が出る `the_repo_claude_md_is_tracked_and_reaches_every_harness_path`）。
- `python3 scripts/dev/check-architecture-map.py`: OK（429 パス）。`sh scripts/dev/check-doc-links.sh`: ok。
- `bash scripts/dev/test-affected.sh`: exit 0。`cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- 全 workspace／e2e／browser／userns 試験は走らせていない（ADR 2026-10-20-full-tests-at-release-only）。

## 残件・提案

- 実 Codex／Pi／opencode の CLI が `AGENTS.md` を実際に読むことの実機確認は未実施（LLM 呼び出しを伴うため）。配布結果（ファイル内容）までを試験で確認した。
- 他の ACP agent が `AGENTS.md` を読まない場合は、前置きへ規則を足す案がある（未実施）。
