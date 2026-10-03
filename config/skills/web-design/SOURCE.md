source_url: https://github.com/pascalorg/skills/tree/main/web-design
commit: 7be87e9292e12fe412d1eb8582fd95fdbd151325
fetched: 2026-10-02
license: MIT
license_file: LICENSE.upstream

## Notes

- Vendored via `git clone --depth 1 https://github.com/pascalorg/skills.git`, then copied the `web-design/` subdirectory verbatim (contains only `SKILL.md` upstream; no LICENSE file in the skill directory).
- **No dedicated LICENSE file exists anywhere in this repo** (checked skill directory and repo root). The repo root `README.md` carries an explicit `## License` section stating "MIT" (see copied `LICENSE.upstream`, which is the full unmodified root `README.md`).
- Judgment call: treated the README's explicit, unambiguous "License: MIT" statement as sufficient confirmation to vendor `SKILL.md` (MIT is permissive and redistribution-friendly), rather than withholding it under the "license not found" rule — a formal LICENSE file does not exist, but the license itself is clearly declared, not absent. Flagged in `agent-docs/progress/ui-ux-skills.md` for human review in case a stricter standard (formal LICENSE file required) is preferred, which would require pulling `web-design/SKILL.md` back out.
- No binary files or symlinks present in this skill directory.
- SKILL.md `name: web-design` already matches the mount name used here (`config/skills/web-design/`); no rename was needed.
- Upstream body (SKILL.md) is unmodified, except for one line noted below.
- modified: 第三者審査（agent-docs/progress/ui-ux-skills.md 参照）で、hit-area ユーティリティの節にあった外部 registry 向けの
  hit-area の導入コマンド（worker がそのまま実行できる形の一文）を、worker が実行せず人に提案するだけの文に書き換えた
  （SKILL.md 1422 行付近）。理由: 人の承認なしに外部 URL から依存を取得・導入するコマンドを skill 本文に残すと、worker
  が承認なしにそれを実行してしまう恐れがあるため。元のコマンド文字列は本ファイル・docs のどこにも引用していない。
