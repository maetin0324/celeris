# PROGRESS — Phase K（知識ベース）

目次は `docs/PROGRESS.md`。設計は `docs/adr/0047-knowledge-base.md` の「Phase K-1 追記」と
`docs/adr/0044-task-management.md` の「D7 追記（Phase K-1）」、利用者向けの規則は `docs/knowledge.md` §2.1 と `docs/mcp.md` §5。

## Phase K-1: 知識の置き場の整理と配置ガード — 完了 2026-09-28（worktree、main 未 merge）

### 人の報告（2026-09-28）

> chatgpt-rdc（MCP client, knowledge:propose）で agent-platform の自己改善案件に celeris の大まかな目的や方針、
> 研究として成立させるための方針を投下させたところ、既存のパスではなく新たに案件 ID に紐づいた知識として登録して
> しまった。pegasus の Qwen の知識も environment 直下に置かれているなど、知識の置き場が混沌としている。直してほしい。

### 原因

1. 案件に slug が無く、`scope_dir("project:<x>")` は `<x>` を素通しで `projects/<x>` にしていた。ChatGPT が
   `scope: project:01M2WTS3DKNZBSZ2JMVB4CZMBW` を渡すと、accept で `projects/01M2WTS3DKNZBSZ2JMVB4CZMBW/celeris.md` ができた。
   ADR-0047 D1 自体が `project:<slug>` と `project:<id>` の両方を書いていた。
2. `record` / `knowledge_propose` の候補は取り込み先を持たず、accept が `scope` のディレクトリ ＋ 題名の slug で決めていた。
   `environment` は分類なしの `environment/<slug>.md`、日本語だけの題名は候補の id の名前（`user/20260922t155154z-note.md`）になった。
3. 同じ題名のページ（`user/profile.md` など init の雛形）があっても新しいページを作った。
4. 知識整理 run の候補（`apply_candidates`）は `path` の境界しか見ていなかった（`environment/` 直下の `create` がそのまま入った）。

### Part 1: KB のデータの整理（`~/.local/share/celeris/knowledge`。人のデータ、1 件 1 コミット）

開始時 `git status` は clean。タグ `pre-cleanup-20260928`（= `61e20bd`）を打ってから始めた。author は GUI の編集と同じ
`Celeris (human) <celeris@local>`。消した内容は無い（全部どこかに統合した）。

| # | コミット | 中身 |
|---|---|---|
| 1 | `c3eaeb0` | `projects/01M2WTS3DKNZBSZ2JMVB4CZMBW/celeris.md` → `projects/agent-platform/design.md`（案件の概要ページが他に無かったので D1 の `design.md`）。`scope: "project:agent-platform"`。sources / created / updated はそのまま、本文は不変（リンクは無かった）。空の ID ディレクトリは消えた |
| 2 | `66391e8` | `projects/agent-platform/` の scope 表記を `project:agent-platform` に統一（`project:01M2…` の 3 件 = coding-harness-routing-foundation / knowledge-gc-state-management / model-org-routing-phase1-design、`projects/agent-platform` の 1 件 = celeris-db-skills-post-patch-api-v1-org-org-toml）。17 ページすべて同じ表記 |
| 3 | `3760b05` | `projects/README.md` から `scope: user` を外す（置き場の説明なので scope を持たない。§2.1） |
| 4 | `3a3b5f2` | `environment/pegasus-qwen-vllm-ib-10-110-0-150-18000-bnode150.md` を `environment/clusters/pegasus-lm-stack.md` の節「確認の詳細（2026-09-24、読み取りのみ）」として統合し削除。tags は和（`tunnel`・`llm-proxy` を追加）、sources は元から同じ `task:01M3A0A0…`、`updated` は新しい方の 2026-09-28 |
| 5 | `aa959e8` | `user/20260922t155154z…155214z-note.md` の 4 件の本文で `user/{profile,expertise,preferences,goals}.md` の雛形（init の空欄）を置き換え、tags は和、sources = `human` + `mcp:chatgpt-rdc`、created = ノートの 2026-09-22、updated = 2026-09-28、confidence = high（ノートの値）。本文はノートと 1 バイトも違わない（`diff` で確認）。ノートは削除 |
| 6 | `d6b50f9` | `environment/taskd/remote-exec.md`（taskd は Celeris の旧名）を `environment/celeris/remote-exec.md` の節「二重 sh -c の引用符罠と SSH 設定の明示指定」として統合し削除。tags / sources は和（sources 6 件）、created は古い方の 2026-09-20 |
| 7 | `d563bda` | `environment/taskd/worktree-sync-tracked-files.md` → `environment/celeris/`。`environment/taskd/` は無くなった |
| 8 | `35ea7a7` | 見つけた参照の修正: `environment/clusters/pegasus.md` と `experience/2026/09/pegasus-remote-tilde-expansion.md` の `environment/taskd/remote-exec.md` → `environment/celeris/remote-exec.md` |
| 9 | `023ab1c` | 他に見つけた不整合: `projects/benchfs/primary-sources.md` の sources の生 URL 10 件を文書の書式 `url:<…>` に揃えた |
| 10 | `1c719e1` | 他に見つけた不整合: `projects/benchfs/framing-candidates.md` に front matter が無かった（出典が無いページ）。本文の題名・置き場の scope・本文冒頭に書かれた出典（`task:01M35X86XTK84F97QW0CN5PGMR`）で付けた。本文は不変 |
| 11 | `5d79c71` | KB の `README.md` に environment の分類・案件 slug・置き場の規則を書き足した |

調べて**問題が無かった**もの: ULID のパスは (1) だけ。scope の根の直下のファイルは (4) と各 README だけ。scope ごとの題名の重複は
整理後 0 件（`index.json` の題名で確認）。scope ラベルと置き場の食い違いは整理後 0 件（`projects/README.md` を除く。下の「未解決」）。
`environment/servers/`・`environment/tools/` は空のディレクトリ（init が作ったもの。git には現れない）。

置き場の前後（`git ls-tree -r --name-only pre-cleanup-20260928` と `git ls-files` の差）:

```
- environment/pegasus-qwen-vllm-ib-10-110-0-150-18000-bnode150.md   → environment/clusters/pegasus-lm-stack.md に統合
- environment/taskd/remote-exec.md                                  → environment/celeris/remote-exec.md に統合
- environment/taskd/worktree-sync-tracked-files.md                  → environment/celeris/worktree-sync-tracked-files.md
- projects/01M2WTS3DKNZBSZ2JMVB4CZMBW/celeris.md                    → projects/agent-platform/design.md
- user/20260922t155154z-note.md                                     → user/profile.md に統合
- user/20260922t155159z-note.md                                     → user/expertise.md に統合
- user/20260922t155209z-note.md                                     → user/preferences.md に統合
- user/20260922t155214z-note.md                                     → user/goals.md に統合
(64 files → 58 files)
```

整理後の `environment/`: `celeris/`（aider-harness-setup, remote-exec, task-creation-quirks, worktree-sync-tracked-files）、
`clusters/`（fern03, pegasus, pegasus-cargo-setup, pegasus-lm-stack, sirius）、`hosts/`（home-dev）。直下にページは無い。

証跡（本番の `~/.local/celeris/current/bin/celerisctl`、`--root ~/.local/share/celeris/knowledge`）:

- `celerisctl knowledge reindex` → `54 pages (2026-09-28T12:23:11Z)`。**`index.json` は `.gitignore` 済みの派生物**（ADR-0047 P-61-a）
  なのでコミットはしていない（依頼の「index.json をコミット」は KB の規約と食い違うため行わなかった）
- `knowledge search "研究として成立"` → `projects/agent-platform/design.md … project:agent-platform`
- `knowledge search "10.110.0.150"` → `environment/clusters/pegasus-lm-stack.md`
- `knowledge search "人のプロフィール"` → `user/profile.md`（tags `user, profile, tsukuba, hpcs, research`）
- `knowledge search "worktree sync" --scope environment/celeris` → `environment/celeris/remote-exec.md` と `…/worktree-sync-tracked-files.md`
- `knowledge get projects/agent-platform/design.md` → 本文と `scope: "project:agent-platform"`

DB の参照（読み取り専用 `file:/var/lib/celeris/celeris.sqlite3?mode=ro`。全表の TEXT 列を LIKE で走査）: KB のパスを**構造として**
持つ列は無い（`knowledge_runs.summary_json` は件数と破棄の理由だけ）。動かしたパスの文字列は `tasks.objective/json`・`events.json`・
`reports.body` に**過去の記録**として出てくるだけ（知識整理 run の依頼文に埋めた索引、run のログ）。未終端のタスクで参照しているものは
0 件。`notifications.body` の `projects/01M2WTS3…` は GUI の案件 URL（`/projects/<id>`）で KB ではない。DB は変えていない。

### Part 2: 仕組み

設計（ADR-0047 Phase K-1 追記、ADR-0044 D7 追記）:

- **案件の slug**: `Project.slug`（`projects.slug`、**migration 0029**、一意の部分インデックス）。作るときに題名の slug → primary
  リポジトリ名 → `project-<id 末尾 8>`、重なれば `<slug>-<id 末尾 8>`（`task_core::knowledge::derive_project_slug`）。
  migration 0029 が既存の行を作った順に埋める（Rust の backfill。0012 と同じ形）。`PATCH /projects/{id} {slug}`（422 / 409
  `project_slug_in_use`）。案件の自動マウント（dispatcher）は `Project::kb_slug()` を使う。
- **置き場のガード** `task_core::knowledge::place`（新しいファイル `crates/task-core/src/knowledge/layout.rs`。純粋関数）:
  `record`（CLI）・MCP `knowledge_propose`・`apply_candidates`（知識整理 run）が全部通す。規則は `docs/knowledge.md` §2.1。
- **`op: append`**（`CandidateOp::Append`）: 既存のページへの候補は上書きせず末尾に節として足す。雛形のままのページは置き換える。
- **accept**: 取り込み先に ULID の段があれば止める。`append` の accept は 1 コミット（`knowledge: <path>（候補 <id> を追記で取り込む）`）。
- **MCP**: `knowledge_propose` に `path` / `op`（`append` | `merge`）を足し、出力に `target` / `op` / `redirect`。置き場の違反は
  `-32002 rejected` と正しい書き方。`tools/list` の説明に規則と**その時点の**分類・`project:<slug>` = 題名（id）の一覧を載せる。
  `knowledge_list` / `knowledge_search` の `scope` の案件 ID も slug に直す。
- 知識整理 run の依頼文（`maintenance_objective`）に置き場の規則を 1 項足した。`celerisctl knowledge record` は取り込み先と op を出す。
- GUI: `append` の色とヒント（`gui/app/lib/knowledge.ts`）。型は `gen:types` で `Project.slug` / `ProjectPatchBody.slug`。

変更したファイル: `crates/task-core/{migrations/0029_project_slug.sql,src/knowledge.rs,src/knowledge/layout.rs,src/org.rs,src/store.rs}`、
`crates/task-ops/src/knowledge.rs`（＋ `Project` の literal に `slug: None` を足しただけのファイル多数）、`crates/task-api/src/{handlers.rs,types.rs,problem.rs,knowledge.rs}`、
`crates/task-dispatch/src/dispatcher.rs`、`crates/celeris/src/knowledge_maint.rs`、`crates/celeris-mcp/src/{tools/knowledge.rs,rpc.rs}`、
`crates/celerisctl/src/commands/knowledge.rs`、テスト（`crates/celeris-mcp/tests/mcp_integration.rs`、`crates/task-api/tests/knowledge.rs`）、
`docs/api/v1/api-v1.schema.json`、`gui/app/celeris/types.ts`、`gui/app/lib/knowledge.ts`、`gui/test/unit/knowledge.test.ts`、
`docs/{knowledge.md,mcp.md,celeris-api-v1.md}`、ADR-0044 / ADR-0047 の追記、この文書。

テスト（足したもの）:

- `task_core::knowledge::layout::tests`（9 件）: ULID と slug の区別、案件 ID → slug と知らない案件の拒否（オフラインは slug だけ通す）、
  `project:<ULID>` → `projects/<slug>/`、environment 直下・知らない分類・分類なしの拒否とタグからの分類、scope と置き場の一致、
  ULID の段の拒否、同じ題名と user 正準ページへの向け直し、slug の導出、MCP の説明の文面
- `task_core::store::tests::migration_29_backfills_unique_project_slugs`: 本番の 4 案件と同じ形（Pluvio 2 件、2 件目の primary は benchfs）で
  `pluvio` / `pluvio-jp572bat` / `agent-platform` / `benchfs`、題名もリポジトリ名も ASCII にならない案件は `project-<末尾>`、`project_set_slug` の
  重複 409 相当・綴り違い・変更、新しい案件の slug
- `task_ops::knowledge::tests::{apply_candidates_guard_the_placement, append_accept_keeps_the_existing_body, projects_readme_has_no_project_scope}`
- MCP 結合テスト `knowledge_propose_resolves_project_ids_rejects_misplaced_pages_and_merges_same_titles`（本物の HTTP）: `tools/list` の説明に
  `project:agent-platform` が載る、`project:<ULID>` の propose が `projects/agent-platform/…` に入り accept しても ID のディレクトリができない、
  `environment/pegasus-qwen.md` は `-32002` と `environment/{…}` の案内、知らない案件は slug の一覧つきで拒否、同じ題名は `append` +
  `redirect.kind = same_title`、`人のプロフィール` は `user/profile.md` への `append` で、雛形が置き換わる
- 既存テストの更新: 既にあるページへの `record` は 409 ではなく `append` になった（task-ops と task-api の inbox のテスト）

### 証拠（ゲート）

ゲート（`CARGO_TARGET_DIR=/var/lib/celeris/scratch/targets/agent-kb-cleanup/target CARGO_INCREMENTAL=0`）:

- `cargo fmt --all -- --check` → exit 0
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0
- `cargo test --workspace --no-fail-fast` → exit 0（87 本の test バイナリ、passed 2615 / failed 0 / ignored 7）
- `UPDATE_SCHEMA=1 cargo test -p task-api --lib schema` で `docs/api/v1/api-v1.schema.json` を作り直した（`Project.slug`・`ProjectPatchBody.slug`・候補の `op` の説明）
- `corepack pnpm@11.27.0 -C gui gen:types` → exit 0（`gui/app/celeris/types.ts` を更新。もう一度走らせて差分 0）、`typecheck` → exit 0、
  `lint` → exit 0（275 files、info 2 件は既存）、`test` → exit 0（74 files、1119 tests）

実物の KB の**コピー**（scratchpad）に対する、この worktree の `celerisctl`（DB を開かない経路）:

- `knowledge reindex` → `54 pages`。`projects/README.md` の scope は `None`（旧バイナリは `project:README.md`）
- `record --title "Qwen forward" --scope environment` → exit 1 `refused: scope \`environment\` needs a category: pass \`path = environment/<category>/<name>.md\` (categories: celeris, clusters, hosts, servers, tools)`
- `record --scope project:01M2WTS3DKNZBSZ2JMVB4CZMBW` → exit 1 `refused: project "01M2…" is not a project slug …`
- `record --title "人のプロフィール" --scope user` → `recorded _inbox/…-note.md → user/profile.md へ append`
- `record --title "Qwen forward" --scope environment --tags pegasus` → `→ environment/clusters/qwen-forward.md`

本物の KB と本番の DB・daemon には、Part 1 のコミット以外は触れていない（コピーは消した。本物の KB の `git status` は clean）。

### 本番への反映で気をつけること（**migration 0029 を含む**）

- **schema が 28 → 29 に上がる**。selfdeploy の検査 5（N-1 互換: 旧バイナリを新しい DB で起こす）は旧バイナリが `SchemaTooNew` で
  起動できず `live_ok = false` になる（Phase F4b の 0028 と同じ）。昇格後に旧リリースへ戻すには `--restore-db` が要る。
- 人の手の作業は**要らない**: migration 0029 が本番の 4 案件に slug を付ける（`pluvio`、`pluvio-jp572bat`、`agent-platform`、`benchfs`。
  前の 2 つは今の案件マウント `projects/<題名の slug>` と同じ）。slug を変えたいときだけ
  `curl -X PATCH -H "Authorization: Bearer …" -d '{"slug":"…"}' http://127.0.0.1:7710/api/v1/projects/<id>`（KB のディレクトリは人が動かす）。
- 昇格したら、ChatGPT 側の skill（KB の `skills/chatgpt-celeris-rdc/SKILL.md`）の「Writing knowledge」に置き場の規則を足すとよい
  （`path` / `op` は新しい引数なので、**昇格より前に足すと今の本番は `deny_unknown_fields` で拒否する**。そのため今回は KB に入れていない）。
  足す文面:

  ```
  Placement (checked by Celeris; a misplaced proposal is rejected with the reason, so fix it and call again):

  * `scope` is `user`, `environment`, `experience` or `project:<slug>` — use the project **slug** (e.g. `project:agent-platform`), never the project id. The current slugs are listed in the `knowledge_propose` tool description.
  * optional `path`: `user/<name>.md`, `environment/<category>/<name>.md` (never directly under `environment/`), `projects/<slug>/<name>.md`, `experience/YYYY/MM/<name>.md`. Give an ASCII file name when the title is Japanese only.
  * Facts about the person go into `user/profile.md`, `expertise.md`, `preferences.md` or `goals.md`.
  * If a page with the same title exists, the proposal is appended to it. To replace a page with an integrated version, read it with `knowledge_get` first and pass `op: "merge"`.
  ```

### 未解決・申し送り

- 今の本番（旧バイナリ）の `reindex` は `projects/README.md` の scope を `project:README.md` と出す（索引の既定の bug。この Phase で直した。
  昇格後の `reindex` で消える）。
- 人の直接の編集（GUI の `PUT /knowledge/page`・エディタ）にはガードをかけていない（KB は人の物）。GUI で候補の取り込み先を書き換える
  場合も ULID の段だけを止める。
- `environment` の同じ題名の判定は分類（ディレクトリ）の中だけ（`clusters/` と `tools/` に同じ題名があっても別物とみなす）。
- 既定の文書リポジトリのディレクトリ名（`~/workspace/<slug>/`、`task_ops::docs::project_slug`）は `Project.slug` に揃えていない
  （既に作られた文書リポジトリの場所を動かさないため）。
- 知識 GC（`knowledge_gc.rs`）は案件の一覧なしのガード（`apply_candidates_with_policy`）を通す。GC は既存ページの update/merge/retire だけなので
  実害は無いが、案件 ID のラベルは slug に直らない。

**昇格**: release `0633d1c91b96`（main 0633d1c = K-1 3635e18 + PROGRESS.md の重複解消）。release.sh のゲート: cargo test 239 s / clippy 18 s / build 43 s、GUI の 3 step は skip（gui/ の差分あり→実行。exit 0）。verify ok=true、live_ok=false（schema 28→29 なので N-1 は想定どおり不可）。2026-09-28 13:26:44Z に **停止→起動** で昇格（backup 20260928-132621-pre-0633d1c91b96）。昇格後、`projects.slug` は pluvio / pluvio-jp572bat / agent-platform / benchfs に backfill 済み。
