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
