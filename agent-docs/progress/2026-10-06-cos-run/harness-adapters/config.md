# CoS harness config capability reasons

tasks: [01M47VN94QK6KHXQDNPCA0V7KK]

Implemented the config-facing view of the task-worker CoS harness capability table.
`Config::cos_harness_capability_warnings` reports non-fatal capability gaps for the
configured harness, and `ResolvedCosProvider` carries the same warnings. When provider
resolution is unavailable, its reason includes those capability notes. Harness and
provider selection remain unchanged; explicit harness/provider/source contradictions
continue to fail config validation.

Current capability table result:

- `claude-code`: no gaps (continuation, shell, filesystem, MCP, image input/tool).
- `codex`: no gaps (continuation, shell, filesystem, MCP, image input/tool).
- `opencode` (`acp`): shell, filesystem, MCP, and image input are unconfirmed and
  produce explicit reasons. Image support is reported missing only when neither native
  image input nor an image-reading tool is confirmed.

The Codex config has `exec_resume` and `experimental_resume` modes, both of which support
continuation. The current capability table therefore does not report a false resume gap.

Tests cover each harness, successful resolution with non-fatal gaps, and propagation of
gaps into an unavailable-provider reason. Targeted tests and workspace clippy are recorded
in the run result.
