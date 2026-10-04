source_url: https://github.com/shadcn-ui/ui/tree/main/skills/shadcn
commit: d75a96ab781f3d659be1ad287347d5887ce9f2fc
fetched: 2026-10-02
license: MIT
license_file: LICENSE.upstream

## Notes

- Vendored via `git clone --depth 1 https://github.com/shadcn-ui/ui.git`, then copied the `skills/shadcn/` subdirectory verbatim.
- No LICENSE file exists inside the skill directory upstream, so the repo-root `LICENSE.md` (MIT, copyright shadcn) was copied here unmodified as `LICENSE.upstream`, per the vendoring convention for this task.
- Docs: https://ui.shadcn.com/docs/skills
- Binary files copied verbatim from upstream (not symlinks, regular files):
  - `assets/shadcn.png` (PNG, 100x100)
  - `assets/shadcn-small.png` (PNG, 16x16)
- No symlinks present.
- SKILL.md `name: shadcn` already matches the mount name used here (`config/skills/shadcn/`); no rename was needed.
- Upstream body (SKILL.md and all accompanying files: customization.md, cli.md, registry.md, mcp.md, agents/openai.yml, rules/*.md, evals/evals.json) is unmodified.
