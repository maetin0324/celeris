# config/skills/ — vendored external agent skills

This directory holds external agent skills vendored for Celeris departments (currently
`ui-ux`). Each `config/skills/<name>/` is a verbatim (or minimally modified, see below)
copy of an upstream skill, with `SOURCE.md` recording the source URL, the upstream commit
or fetch date, and the license, and `LICENSE.upstream` / `LICENSE.txt` holding the license
text. Any deliberate deviation from the upstream content is recorded in `SOURCE.md` with a
line starting `modified: ` (or `modified / excluded: `), explaining what changed and why.

## Dependency policy for workers using a vendored skill

These skills were written for a general audience and show code examples that assume a
fuller dependency set than this project installs. A worker (and especially the `ui-ux`
department's design generator) that loads one of these skills must follow this policy:

- **Do not install libraries shown only as code examples in a skill's body.** This
  includes (non-exhaustive — any library named only inside a skill's example code, not
  already a project dependency, falls under this rule): `next-themes`, `motion`,
  `react-hook-form`, `zod`, `lucide-react`, `figma-squircle`, `ForesightJS`,
  `next/font/google`, and the `hit-area` utility package referenced in `web-design/SKILL.md`.
- **The only dependency a worker may add on its own is `shadcn`** (the already-decided
  component tooling for this project). Anything else a skill suggests is a *proposal to the
  human*, not something to install unasked — name the package, what it would add, and wait
  for explicit approval before adding it as a dependency.
- **Tests and builds must not reach the external network.** Do not let a skill's example
  (e.g. fetching a component from a registry URL, running a CLI against an external host)
  turn into a network call during a test or build. Component installation that does need
  the network is a proposal for the human to run, not something a test/build step performs.

See each skill's own `SOURCE.md` for what, if anything, was changed from upstream, and
`docs/progress/ui-ux-skills.md` for the vendoring and review history.
