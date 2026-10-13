# worker-route

tasks: [01M4SRWPFY0E50F6QF0GQV877X]

- Removed tier alias checks from OpenCode, Aider, and Pi routing-context transport. OpenCode and Aider use a configured destination URL together with a routing context reference; Pi adds its header only when both a base URL and routing context reference are present. Pi always parses model names as `provider/model`.
- Replaced Pi and tool-policy fixtures with concrete model IDs (`qwen/qwen3.8-27b`, `opencode-go/grok-4.6`). Added focused OpenCode/Aider checks for header transport and missing destination/reference cases, plus a Pi check that omits the header without a proxy base URL.
- Acceptance grep found no `celeris/frontier|standard|cheap` in the four owned files. Focused Pi (17), routing-context transport (2), and tool-policy (10) tests passed; `cargo clippy -p task-worker -- -D warnings` passed. `cargo test -p task-worker` was intentionally not run in this leaf; the `worker-suite` unit owns it.
- The requested `/local/celeris/data/scratch/cr-worker-route` TMPDIR was read-only here. Focused tests passed with the run-scoped writable TMPDIR under `.../runs/01M4STGZNV15GMAQZBY5PHNZ51/tmp/worker-route`.
