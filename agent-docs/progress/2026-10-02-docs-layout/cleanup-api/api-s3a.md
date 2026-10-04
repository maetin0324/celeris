---
title: docs 再構成 — gui-api.md §3.1〜§3.62 を handler・型と照らして直す（cleanup-api / api-s3a）
tasks: [01M3YNFVFZG5AFFM0RS05X22ZT]
status: done
updated: 2026-10-02
---
# docs 再構成 — gui-api.md §3.1〜§3.62 を handler・型と照らして直す（cleanup-api / api-s3a）

完了日: 2026-10-02。編集は `docs/api/v1/gui-api.md` の `### 3.1` から `### 3.63`（retry）の見出しの手前までだけ。
`## 3.` の見出し・記法の行と `### 3.63` の見出し行、§2 の表は触っていない。crates/・web/・gui/・scripts/・CLAUDE.md・.claude/・
docs-layout.tsv・*.schema.json は読んだだけ。

## 処理した文書

| 処理(削除/統合/移動/修正) | 旧パス | 新パス or - | 理由 | 最後の commit |
|---|---|---|---|---|
| 修正 | `docs/api/v1/gui-api.md`（§3.1〜§3.62） | - | 各節を task-api の handler・types.rs・task-ops と照合し、食い違い・撤去済みの挙動・経緯を直した | `d9524c0f` |

## 直した点（要点。根拠は crates/ のファイル:行）

- 見出しの `METHOD /path` はすべて実在の route（§3.8 の `{stdout|stderr|result|request|prompt}` も 5 本とも実在）。見出し番号は変えていない。
- 認可: `POST /tasks`・approve/reject/answer/cancel・`POST /replay`・`PATCH /projects/{id}` は `require_admin` を呼ぶ管理系と明記（handlers/tasks.rs:184、handlers/task_actions.rs、handlers/system.rs:110、handlers/projects.rs:234）。
- 撤去済み（410 `removed_by_adr_0079`）に書き換えた: §3.14 `POST /plans`、§3.49 途中目標の POST/PATCH、§3.61 `POST /projects/{id}/plan`、§3.62 前の `#### 3.63 POST /milestones/{id}/decide`（milestones.rs:53、project_plan.rs:53、handlers/system.rs:88）。旧の本文・応答例・エラーは削除。
- §3.1: schema_version の migration 列挙をやめて `task_core::SCHEMA_VERSION`（現在 37）に。`db.filesystem`/`db.device` を追加。verify モードの dispatch 条件を直した。
- §3.2: スナップショットの参照先（3.23→3.20）、`attention[].type`、`browser_waits`/`decisions`/`counts` を追加。
- §3.3〜3.6: クエリの丸め・400 条件、`support` の語彙 8 種、`is_root_task`/`paused`/`actions`、POST /tasks の受ける欄と既定値の順、TaskDetail の欄、イベントの `types` 語彙（query.rs:153）。
- §3.8〜3.9: Remote の手元写し、403/400 の条件、Range、`forbidden` の意味。
- §3.19〜3.21: providers の `account_pool`/`account_id`/`tier_models`/`credential_refs`、daemon の欄、config の `plan_auto_accept`。
- §3.23〜3.41: clusters の `stats`/`work_dir`、省略される欄、409 `providers_admin_unavailable` の条件、check は 3 ターンまで、reload の反映表、accounts の `?adapter=` の範囲（3.31〜3.35）、secrets の `used_by` は起動時に作る、cluster connect の応答例・disconnect の挙動。
- §3.42〜3.55: org の継ぎ方・検証、projects の `remote.mode`、`include_frozen` と追加欄、PATCH の書ける欄・status 制約、reports の limit の丸め、対話の max_turns 10・対話用分野。
- §3.56〜3.62: approvals decide の 409 条件（計画の承認待ち）と追加欄、memory の 409 の順。
- Phase 番号・日付・実機の事故の記述は ADR 参照に縮めた（範囲内の「Phase」は境界の `### 3.63` 見出し行を除き 0 件）。

## 証拠コマンドと結果

- 見出しの route: `find crates/task-api/src -name '*.rs' -exec cat {} + | tr -d '\n' | grep -oE '\.route\( *"[^"]+"'` で route を取り、範囲内の `###`/`####` 見出しのパスを照合 → 不一致は §3.8 の `{a|b}` 表記 1 件のみ（展開した 5 本＋artifacts/{idx} はすべて実在）。
- 範囲外不変: `git diff -U0 docs/api/v1/gui-api.md` の hunk は旧 472〜1638 行の中だけ（§2 の表・`### 3.63` 見出し以降は差分なし）。
- 新規の相対リンクなし（差分行に `](` 0 件）。
- 照合は 4 つの subagent が chunk ごとに読み、主要な主張（POST /tasks の require_admin、milestones decide の 410、SCHEMA_VERSION 37、CONVERSATION_MAX_TURNS 10）を手で再確認した。
- cargo test / clippy は docs のみの変更のため未実行。

## 未解決

- `#### 3.63 POST /milestones/{id}/decide` が §3.61 と §3.62 の間にあり、`### 3.63 POST /tasks/{id}/retry` と番号が重なる。兄弟 WU（api-s3b）と境界の見出しで衝突しないよう番号は変えていない。
- §3.46 の worktree の細部（ブランチ名・置き場所）、§3.50 の報告の生成元、§3.23 の自動接続・probe 間隔、3.32/3.33 の時間制限は dispatcher/celeris 側を読み切っていない。
- 「DESIGN §5.10」の参照（DESIGN.md は正本でない）を残した。
- `PUT /clusters/{id}/settings`（§2 #120）に §3 の節が無い。

## 提案

- 撤去済みの `#### 3.63`（milestones decide）を §3.125.8 の撤去一覧へ寄せて節ごと消し、番号の重複をなくす（integrate-body で）。
- `PUT /clusters/{id}/settings` の節を §3.23 の後に足す。
