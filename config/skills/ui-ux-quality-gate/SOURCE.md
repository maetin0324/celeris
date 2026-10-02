source_url: https://github.com/atuizz/codex-ui-ux-skill/tree/main/ui-ux
commit: 3c311f71f5aab40af3a10dadb2306578783979d0
fetched: 2026-10-02
license: MIT
license_file: LICENSE.upstream

## Notes

- Vendored via `git clone --depth 1 https://github.com/atuizz/codex-ui-ux-skill.git`, then copied the `ui-ux/` subdirectory verbatim.
- No LICENSE file exists inside the skill directory upstream, so the repo-root `LICENSE` (MIT, copyright "ui-ux contributors") was copied here unmodified as `LICENSE.upstream`.
- **Renamed**: upstream skill directory and SKILL.md frontmatter `name:` were both `ui-ux`. Renamed to `ui-ux-quality-gate` (directory `config/skills/ui-ux-quality-gate/`, frontmatter `name: ui-ux-quality-gate`) to avoid colliding with Celeris's own `ui-ux` department id. This is the only intentional change to upstream content; all other text in SKILL.md and accompanying files is unmodified.
- Positioning: this skill is a quality gate / reviewer (loading/empty/error/mobile/accessibility checks, AI-generated-UI anti-patterns, visual QA), not a primary design generator — see the task objective and `docs/progress/ui-ux-skills.md` for how it is meant to be wired into review runs rather than generation runs.
- No binary files or symlinks present in this skill directory.
- Upstream accompanying files copied verbatim: `references/*.md`, `templates/*.md`, `evals/*.json`.
- modified / excluded: 第三者審査（docs/progress/ui-ux-skills.md 参照）を受け、upstream の `scripts/init_frontend_quality.py`
  はこの配布に同梱しない（worker にも mount しない）。理由: この script は (1) 対象リポジトリの `AGENTS.md` に無条件で
  追記する、(2) `--force` でテンプレートを無検査に上書きする、(3) `--project` のパスを検査せずファイルシステムに書き込む、
  という 3 点を持ち、承認なしに worker が実行するにはリスクが高いと判断した。`SKILL.md` の「Templates」節と
  `evals/evals.json` の該当する expectation 文言は、この配布に `scripts/` が無いことを前提にした文に書き換えた
  （テンプレート自体は手動でコピーする運用に変更。upstream の `scripts/` が必要な場合は、人が内容を確認したうえで
  個別に取得する判断とする）。
