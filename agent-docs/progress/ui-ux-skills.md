---
tasks: [01M3XTAH6KYYXTWRTNC2D4MDTG]
---
# ui-ux 課向け外部 skill の vendoring（取得記録）

WorkUnit `vendor-skills`（工程: upstream skill 4 件の取得と出典・ライセンスの保持）の実行記録。4 つの外部 agent skill を
`git clone --depth 1`（https、`gh` は不使用）で取得し、`config/skills/<name>/` にディレクトリごと複製した。各ディレクトリの
`SOURCE.md` に出典・commit sha・ライセンスを記録済み（本ファイルは取得作業の要約）。

ui-ux 課への mount・routing skill との共存確認・worker への注入経路は別 WorkUnit（`skill-import` / `e2e-verify` /
`review-skills` / `adr`）の範囲。本 WorkUnit は取得・配置とライセンス保持のみを行う。

## 1. frontend-design（Anthropic）

- source: https://github.com/anthropics/skills/tree/main/skills/frontend-design
- commit: `8a1541c4a3ffa5a20a5a91de0dcf3f0bab1d1ef4`（2026-09-28 時点の main HEAD）
- fetched: 2026-10-02
- 配置先: `config/skills/frontend-design/`
- ファイル一覧: `SKILL.md`, `LICENSE.txt`, `SOURCE.md`（本取得で追加）
- ライセンス: Apache-2.0。`LICENSE.txt` は upstream のスキルディレクトリ内に既に同梱されていた（標準 Apache License 2.0
  本文、改変なし）。repo 根の LICENSE 代用は不要だった。
- frontmatter: 単一行 `name` / `description` / `license`（3 キー、複数行 YAML なし）。`name: frontend-design` は
  mount 名と一致、改名不要。
- バイナリ・symlink: なし。

## 2. shadcn（shadcn/ui 公式）

- source: https://github.com/shadcn-ui/ui/tree/main/skills/shadcn（docs: https://ui.shadcn.com/docs/skills）
- commit: `d75a96ab781f3d659be1ad287347d5887ce9f2fc`（2026-10-01 時点の main HEAD）
- fetched: 2026-10-02
- 配置先: `config/skills/shadcn/`
- ファイル一覧: `SKILL.md`, `customization.md`, `cli.md`, `registry.md`, `mcp.md`, `agents/openai.yml`,
  `rules/base-vs-radix.md`, `rules/chat.md`, `rules/composition.md`, `rules/forms.md`, `rules/icons.md`,
  `rules/styling.md`, `assets/shadcn.png`, `assets/shadcn-small.png`, `evals/evals.json`, `LICENSE.upstream`（repo 根
  `LICENSE.md` の複製）, `SOURCE.md`（本取得で追加）
- ライセンス: MIT。スキルディレクトリ内に LICENSE は無く、repo 根の `LICENSE.md`（MIT, shadcn）を `LICENSE.upstream`
  として複製。
- frontmatter: 単一行 `name` / `description` / `user-invocable` / `allowed-tools`（4 キー、license キーなし、複数行
  YAML なし）。`name: shadcn` は mount 名と一致、改名不要。
- バイナリ: `assets/shadcn.png`（100x100 PNG）, `assets/shadcn-small.png`（16x16 PNG）。symlink なし。

## 3. web-design（pascalorg）

- source: https://github.com/pascalorg/skills/tree/main/web-design
- commit: `7be87e9292e12fe412d1eb8582fd95fdbd151325`（2026-09-11 時点の main HEAD）
- fetched: 2026-10-02
- 配置先: `config/skills/web-design/`
- ファイル一覧: `SKILL.md`, `LICENSE.upstream`（repo 根 `README.md` の複製）, `SOURCE.md`（本取得で追加）
- ライセンス: **人の判断待ちの可能性あり**。スキルディレクトリにも repo 根にも独立した LICENSE ファイルが存在しない。
  repo 根 `README.md` に `## License` 節があり本文は「MIT」とだけ記載。今回は MIT の明示的な宣言があり permissive な
  ライセンスであることから「見つからない」には当たらないと判断し、`SKILL.md` を含めて取得し、`README.md` をそのまま
  `LICENSE.upstream` として複製した（`config/skills/web-design/SOURCE.md` に判断の根拠を記載）。ただし正式な LICENSE
  ファイルが無い点は、より厳格な基準（独立 LICENSE ファイル必須）を人が求める場合は取り下げが必要になる、という留保
  付きの判断である。止めてはいないが、最終レビューで確認してほしい論点として記録する。
- frontmatter: `name` / `description` / `metadata`（`metadata.author`, `metadata.version` の 2 階層ネスト。複数行
  YAML あり。license キーなし）。`name: web-design` は mount 名と一致、改名不要。
- バイナリ・symlink: なし。

## 4. ui-ux-quality-gate（atuizz、改名: ui-ux → ui-ux-quality-gate）

- source: https://github.com/atuizz/codex-ui-ux-skill/tree/main/ui-ux
- commit: `3c311f71f5aab40af3a10dadb2306578783979d0`（2026-07-06 時点の main HEAD）
- fetched: 2026-10-02
- 配置先: `config/skills/ui-ux-quality-gate/`（upstream のディレクトリ名・frontmatter `name` はいずれも `ui-ux`。
  Celeris の `ui-ux` 課 id と衝突するため `ui-ux-quality-gate` に改名。改名は `config/skills/ui-ux-quality-gate/SOURCE.md`
  に記載。改名した frontmatter `name` 以外の upstream 本文は無改変）
- ファイル一覧: `SKILL.md`, `references/tool-selection.md`, `references/design-reference-packs.md`,
  `references/workflow.md`, `references/anti-patterns.md`, `references/project-cognition.md`,
  `references/development-guardrails.md`, `references/ux-evaluation.md`, `scripts/init_frontend_quality.py`,
  `templates/PAGE_BRIEF.md`, `templates/DESIGN.md`, `templates/FRONTEND_CONTRACT.md`, `templates/FRONTEND_REVIEW.md`,
  `evals/trigger-evals.json`, `evals/evals.json`, `LICENSE.upstream`（repo 根 `LICENSE` の複製）, `SOURCE.md`（本取得
  で追加）
- ライセンス: MIT（copyright "ui-ux contributors"）。スキルディレクトリ内に LICENSE は無く、repo 根の `LICENSE` を
  `LICENSE.upstream` として複製。
- frontmatter: `name` / `description`（2 キーのみ、`description` は二重引用符で囲んだ 1 行の長文。license キーなし、
  複数行 YAML なし）。
- 位置づけ: この skill は主たる design generator ではなく **quality gate / reviewer**（loading/empty/error/mobile/
  accessibility 確認、AI 生成 UI のアンチパターン検出、visual QA）として扱う。review 段での配布は別 WorkUnit
  （`review-skills`）の範囲。
- バイナリ・symlink: なし。

## frontmatter 形式のまとめ（比較）

| skill | キー数 | 複数行 YAML | license キー |
|---|---|---|---|
| frontend-design | 3（name, description, license） | なし | あり（値は "Complete terms in LICENSE.txt"） |
| shadcn | 4（name, description, user-invocable, allowed-tools） | なし | なし |
| web-design | 3（name, description, metadata） | あり（metadata 配下に author/version） | なし |
| ui-ux-quality-gate | 2（name, description） | なし | なし |

4 skill とも frontmatter の形式がまちまち（license キーの有無、ネストの有無、allowed-tools のような実行専用キーの
有無）であり、取り込み側（別 WorkUnit `skill-import`）はこれらすべてを許容するパーサが必要になる。

## 検証

- `find config/skills -maxdepth 2 -type f | sort` で 4 ディレクトリそれぞれに `SKILL.md` と `SOURCE.md`、ライセンス
  系ファイル（`LICENSE.txt` または `LICENSE.upstream`）が揃っていることを確認済み。
- 本 WorkUnit の範囲外（`crates/` は未変更。`cargo test` / `cargo clippy` の対象に含まれる変更はなし）。

## 第三者審査の反映（WorkUnit `vet-skills`）

人による第三者 skill 審査で挙がった 3 点を反映した。

1. **web-design の hit-area 導入コマンドの提案化** — `SKILL.md`（1422 行付近）の hit-area ユーティリティの節に、
   worker がそのまま実行できる形の外部 registry 向け導入コマンド（第三者 CLI 経由で外部 URL から依存を取得する一文）
   があった。worker が人の承認なしに依存を追加しないよう、「人に提案し、承認後にのみ導入する」文へ書き換えた。
   元のコマンド文字列は本ファイルを含めどこにも引用していない（`config/skills/web-design/SOURCE.md` に
   `modified: ` 行として詳細を記録）。
2. **ui-ux-quality-gate の `scripts/` 除外** — upstream の `scripts/init_frontend_quality.py` は
   (1) 対象リポジトリの `AGENTS.md` へ無条件で追記する、(2) `--force` でテンプレートを無検査に上書きする、
   (3) `--project` のパスを検査しない、という 3 点のリスクがあり、worker に mount しない（ディレクトリごと削除）。
   `SKILL.md` の「Templates」節と `evals/evals.json` の該当箇所は、この配布に `scripts/` が無い前提の文へ
   書き換えた。詳細は `config/skills/ui-ux-quality-gate/SOURCE.md` の `modified / excluded: ` 行を参照。
3. **依存方針の明文化** — `config/skills/README.md` を新規作成し、vendored skill のコード例に出るライブラリ
   （`next-themes`, `motion`, `react-hook-form`, `zod`, `lucide-react`, `figma-squircle`, `ForesightJS`,
   `next/font/google` 等）を worker が無断で導入しないこと、自律的に追加してよい依存は `shadcn` のみであること、
   テスト・ビルドで外部ネットワークに出ないことを記載した。

この反映は `config/skills/` と `docs/progress/ui-ux-skills.md` のみの変更であり、`crates/` `web/` `gui/` は
変更していない（`cargo test` / `cargo clippy` の対象外）。

## org 種の更新と結合試験（work unit `e2e-verify`）

完了日 2026-10-02。`config/org.example.toml` の `ui-ux` に `skills_mounts` と依存方針の policy 行を追加し、
`ui-ux` 課の worker 作業場所に 4 skill の本文が実際に届くことを確かめる結合試験を追加した（routing 用の
`profile.skills` は不変）。

### 変更した path

- `config/org.example.toml`（`[[org]] id = "ui-ux"` の `[org.profile]` に `skills_mounts = ["frontend-design",
  "shadcn", "web-design", "ui-ux-quality-gate"]`（license: none の skill は無いので 4 件とも含む）と、policy に
  「skill のコード例に出るライブラリ（next-themes, motion, react-hook-form, zod, lucide-react,
  figma-squircle, ForesightJS, next/font/google 等）は導入しない。依存は決定済みの範囲（shadcn）のみ、それ以外は
  提案に留める。テストやビルドで外部ネットワークに出ない。」を1行追加）
- `crates/celeris/tests/ui_ux_skills_delivery.rs`（新規）— 3 件の結合試験:
  - `ui_ux_skills_mounts_resolve_from_org_example_toml`: `celeris.example.toml` + `org.example.toml` を
    `Config::load` で読み、`task_core::resolve_profile` で `ui-ux` の実効 profile を解決すると
    `skills_mounts` が 4 件（上と同じ順）、`skills`（routing 用）は従来どおり（親 `engineering` から継いだ
    `software` を含め 10 件）であること、`software-engineering` には `skills_mounts` が漏れないことを確認。
  - `ui_ux_skills_reach_claude_code_workspace_with_real_bodies`: `config/skills/` を一時 KB
    （`task_ops::knowledge::init` + `skills_import_dir`）に取り込み、`skills_get` / `skill_applies_to`
    （`SkillUse::Work`）/ `skill_description` で解決した 4 件の `SkillMount` を
    `task_worker::skills::deliver_claude_code` に渡すと、一時作業場所の `.claude/skills/<name>/SKILL.md` の
    本文が `config/skills/<name>/SKILL.md` と**一致**すること、`ui-ux-quality-gate/scripts/` が取り込み元にも
    作業場所にも存在しないことを確認。同じ `SkillMount` を `task_worker::skills::deliver_agents_md` に渡すと
    `AGENTS.md` の `<!-- celeris:skills:start/end -->` 節に 4 件の `### <name>` 見出しが入り、除外済みの
    `scripts/init_frontend_quality.py` への言及が無いことを確認。
  - `ui_ux_skills_review_run_only_delivers_the_quality_gate`: 同じ 4 件を `SkillUse::Review` で解決すると
    `ui-ux-quality-gate`（`celeris-use: work, review`）1 件だけが残り（実 `SKILL.md` の frontmatter を読んだ
    結果。合成データではない）、`deliver_claude_code` 後の `.claude/skills/` には他 3 件が存在しないことを確認。

### 既存試験との関係（重複ではなく役割分担）

- `crates/task-ops/src/knowledge/skill_import_tests.rs::ui_ux_skills_config_skills_import_into_a_temp_kb` —
  `config/skills/` → KB 取り込みの単体（license/付属ファイル/バイナリ除外）。本 WorkUnit はこれを前提に
  **KB → worker 作業場所**までの配送を結合して確認する。
- `crates/task-dispatch/src/dispatcher/tests/ui_ux_skills.rs` — 合成 org node と `instant` adapter で
  dispatcher が `RunContext.skills` に正しい名前の集合を積むこと（work=4件・review=1件・他課への非漏洩）を
  確認。本 WorkUnit は**実 `org.example.toml`** から解決した profile と**実 `SKILL.md` の本文**が worker の
  ファイルシステム（`.claude/skills/` / `AGENTS.md`）に**実際に書かれる**ことまでを見る（LLM 呼び出しなし、
  決定的）。
- `crates/celeris/src/config/tests.rs::example_org_routes_ui_work_to_ui_ux_and_api_work_to_software_engineering`
  — routing（`profile.skills` に基づく担当決定）の既存回帰試験。`skills_mounts` の追加後も再実行して
  通過を確認済み（下記コマンド）。

### 証拠コマンドと結果

- `cargo build --workspace --bins` → exit 0
- `cargo test --workspace ui_ux_skills 2>&1 | grep -E 'running [0-9]+ test|test result:'` →
  celeris: `running 3 tests` / `test result: ok. 3 passed; 0 failed`、celerisctl: `running 1 test` /
  `1 passed`、task-dispatch: `running 4 tests` / `4 passed`、task-ops: `running 4 tests` / `4 passed`
  （計 12 passed、0 failed、`ui_ux_skills_mounts_resolve_from_org_example_toml` を含め flake なし）
- `cargo test -p celeris --test ui_ux_skills_delivery` → exit 0（`test result: ok. 3 passed; 0 failed`）
- `cargo fmt --all -- --check` → exit 0（差分なし）
- `cargo clippy --workspace -- -D warnings` → exit 0（警告なし）
- `cargo test --workspace` → exit 0（120 試験バイナリすべて `test result: ok`、合計 3236 passed / 0 failed。
  `example_org_routes_ui_work_to_ui_ux_and_api_work_to_software_engineering` を含む routing 試験・
  `the_two_example_files_load_together_through_org_include` も通過）
- いずれも渡された `CARGO_TARGET_DIR` / `RUSTC_WRAPPER` / `SCCACHE_*` のまま実行。外部ネットワークへの
  アクセスなし（`task_ops::knowledge::init` の `git init` はローカルのみ）。

### 出典・ライセンス（再掲）

4 skill の出典 commit sha とライセンスは本ファイル冒頭（vendor-skills / vet-skills 節）のとおり:
frontend-design（Apache-2.0, `8a1541c4a3ffa5a20a5a91de0dcf3f0bab1d1ef4`）、shadcn（MIT,
`d75a96ab781f3d659be1ad287347d5887ce9f2fc`）、web-design（MIT, `7be87e9292e12fe412d1eb8582fd95fdbd151325`）、
ui-ux-quality-gate（MIT, `3c311f71f5aab40af3a10dadb2306578783979d0`、改名元 `ui-ux`）。`license: none` で
除外した skill は無い（4 件とも `org.example.toml` の `skills_mounts` に含む）。

### 未解決事項

なし。この WorkUnit の範囲（org 種の更新・結合試験）は完了。
