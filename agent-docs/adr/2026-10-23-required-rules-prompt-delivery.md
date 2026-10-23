# 必須規則（CLAUDE.md）を Codex・Pi・ACP へ prompt の前置きで届ける

- 状態: 承認（実装済み）
- 日付: 2026-10-23
- task: 01M5KWPP3YPZMZ71PZP6HAGNFD

## 背景（実測、2026-10-23）

正本は作業場所直下の tracked な `CLAUDE.md`。各 harness が自分で読む file は次のとおり（host に入っている版の配布物を読んで確かめた。CLI は起動していない）。

| harness | 版 | 自分で読む作業場所の file | CLAUDE.md は届くか |
|---|---|---|---|
| Claude Code | 2.1.303 | `CLAUDE.md`（worker は `--setting-sources` 等で project 設定を外していない） | 届く |
| Codex | 0.162.1 | `AGENTS.override.md` / `AGENTS.md`（既定 `project_doc_fallback_filenames = []`） | 届かない |
| Pi | 0.84.2 | 各 dir で `AGENTS.override.md`・`AGENTS.md`・`AGENTS.MD`・`CLAUDE.md` の**最初の 1 つ**（`resource-loader.js` の `loadContextFileFromDir`） | `AGENTS.md` があると届かない |
| opencode（ACP） | 配布 binary | `AGENTS.md`・`CLAUDE.md`・`CONTEXT.md` の**最初の 1 つ**（`OPENCODE_DISABLE_CLAUDE_CODE` で CLAUDE.md を外せる） | `AGENTS.md` があると届かない |

前の試み（同 task attempt 1）は `CLAUDE.md` を `AGENTS.md` の区切り節へ写したが、(1) `CLAUDE.md`→`AGENTS.md` の symlink の repo では写すたびに節が入れ子に増え、(2) `AGENTS.md` を追跡する repo では追跡 file を書き換え、追跡しない・ignore しない repo では未追跡 file を残し、範囲 check や worker の commit に混ざった。

## 決定

- D1: Codex・Pi・ACP の run は、作業場所の `CLAUDE.md` の全文を見出し `## 必須規則（celeris: …）` 付きで prompt の先頭に置く（`crates/task-worker/src/required_rules.rs` の `preamble`）。作業ツリーには何も書かない。`AGENTS.md` に規則の節は作らない（前の試みの `deliver_required_rules` は消した）。
- D2: `CLAUDE.md` が無い作業場所は前置きなし。参照先の無い symlink・読めない・空は run を落とす（`AdapterError::Other("required rules delivery failed: …")`、harness は起動しない）。
- D3: Claude Code は今までどおり `CLAUDE.md` を直接読む（前置きは足さない）。
- D4: Pi・opencode で `AGENTS.md` が無いときは harness 自身も `CLAUDE.md` を読むので規則が 2 度見える。害は無く、harness の版ごとの探索規則に依存しないことを優先する。

## 試験

- `crates/task-worker/src/required_rules/tests.rs`: 前置きの形・欠落・壊れた symlink・空・読めない、fresh worktree 4 形（AGENTS.md なし／追跡された人の AGENTS.md／AGENTS.md→CLAUDE.md／CLAUDE.md→AGENTS.md）で作業ツリー不変。
- `codex/tests.rs`・`pi/tests.rs`・`acp/tests.rs` の `required_rules_reach_*_from_a_fresh_worktree`: `git worktree add` で切った作業場所から stub CLI の run を 2 回回し、stub が受け取った prompt（ACP は `session/prompt` の JSON）に CLAUDE.md 全文が 1 度だけあり、`git status --porcelain --ignored` が空。`*_run_fails_when_required_rules_are_unreadable`: 壊れた規則で run が Err になり stub が起動しない。
- `claude_code/tests.rs` の `required_rules_reach_claude_code_from_a_fresh_worktree`: claude は worktree を cwd に起動され、正本の CLAUDE.md がそこに見え、`--setting-sources`・`--bare` は付かず、前置きは足されない。

## 範囲外

- Codex の skills 一覧（`skills::deliver_agents_md` が `AGENTS.md` の `celeris:skills` 節を書く。ADR-0127）は従来どおり。skill がある Codex run だけが `AGENTS.md` を書く。
