---
tasks: [01M4Q53E9WQEKMAS448GZMWTFB]
---
# eval-harness eval-recheck

Implemented `crates/routing-eval/src/recheck.rs` and the `celeris-routing-eval recheck` subcommand. It verifies that each frozen manifest problem actually discriminates: the recorded check command must **fail** at the source work unit's `base_commit` (a check that passes there scores nothing — noop) and **pass** at the recorded `head_commit`. Problems that do not satisfy this are excluded with a reason, and the record is written into the manifest's `recheck` key.

Execution: for each problem and each stage a detached `git worktree add` is created under `$TMPDIR` (`celeris-routing-eval-recheck-<pid>/wt/<n>-<stage>`, overridable with `--worktree-root`), the command runs as `sh -c <command>` with the worktree as cwd and its output on log files (no pipe to deadlock), and the worktree is removed afterwards. base and head deliberately get **separate** worktrees so files an earlier run left behind cannot make the later run pass. The whole `$TMPDIR` root is dropped at the end; the source repository is only read (`git worktree prune` / `add` / `remove`), and `git status --porcelain` of the source stays empty (asserted by a test). Each check has a time limit (`--timeout`, default 900 s): the child is killed and the problem excluded as `check_timed_out`. No LLM is called.

Record shape (`celeris.routing-eval-recheck/1`): `manifest_sha256`, `repo`, `checked_at` (RFC 3339, injectable with `--checked-at` for tests), `timeout_s`, per-problem rows (`command`, `base_commit`, `head_commit`, `adopted`, `reason`, `base_exit`, `head_exit`, short `detail`), and the totals `adopted` / `excluded` / `excluded_by_reason`. Writing the record adds only the `recheck` key: the frozen `problems` array and `sha256` are untouched, and the manifest still deserializes as `Manifest` afterwards (the key is outside the hash, so re-running `manifest` reproduces the same sha). `--out <file>` additionally writes a standalone copy.

Exclusion reasons: `no_command_check`, `no_base_commit`, `no_head_commit`, `commit_not_in_repo` (force push / shallow clone), `noop_check_passes_at_base`, `check_fails_at_head_commit`, `check_timed_out`, `check_could_not_run`, `worktree_failed`. A manifest with another schema and a `--repo` that is not a git repository are rejected up front.

CLI: `celeris-routing-eval recheck --manifest <file> --repo <path> [--out <file>] [--timeout <secs>] [--limit <n>] [--worktree-root <dir>] [--checked-at <ts>]`; `recheck --help` prints the usage, the discrimination rule and the reason list. Submodule contents are not populated in the worktrees (a check that needs them must be excluded by hand or the repo prepared beforehand).

Verification:
- `cargo test -p routing-eval recheck`: exit 0, 8 passed / 0 failed (1 filtered out — the `manifest` test). All tests build a temporary git repository with a base commit (no `notes/flag.txt`) and a head commit (with it), plus a temporary manifest; no production database, no network, no LLM.
  - adoption plus the six exclusion paths (noop, head failure, no command, no head commit, no base commit, unknown commit), `excluded_by_reason`, no leftover worktree registration, manifest `sha256` unchanged and still parseable, `--out` copy equal to the record;
  - `check_timed_out` for a `sleep 30` check with a 1 s limit;
  - base-run pollution cannot make the head run pass, and the source repository stays clean;
  - foreign schema and non-repository `--repo` are rejected;
  - CLI parsing (required `--manifest`/`--repo`, `--timeout`, `--limit`) and the end-to-end `run()` path that writes the record into the manifest using the default `$TMPDIR` worktree root.
- `cargo test -p routing-eval`: exit 0, 9 passed / 0 failed (including the pre-existing `manifest` test).
- `cargo clippy -p routing-eval --all-targets -- -D warnings`: exit 0. `cargo fmt -p routing-eval -- --check`: no diff.
- CLI smoke run against a temporary repository and a hand-written manifest (4 problems: good / noop / head-fails / missing head commit) printed `adopted=1 excluded=3 check_fails_at_head_commit=1 no_head_commit=1 noop_check_passes_at_base=1`, exit 0; the source repository kept a single worktree registration, an empty `git status --porcelain`, and no `celeris-routing-eval-recheck-*` directory was left in `$TMPDIR`.

Unresolved: `crates/routing-eval/Cargo.toml` gained `time = { version = "0.3", features = ["formatting"] }` for the RFC 3339 timestamp (and `Cargo.lock` moved with it); the workspace-wide `scripts/dev/test-parallel.sh` and `cargo clippy --workspace` gates are the integrate stage's job and were not run here — only the crate-level gates above. Downstream units (`aggregate`, `stats`) should filter on `recheck.problems[].adopted` rather than re-deriving adoption.
