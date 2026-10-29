---
task: 01M62V0FQMMYQ1S83GBXQP7A6J
unit: measure-config
status: done
completed: 2026-10-29
---
# measure-config: ops-r1 測定 daemon の config 生成 script と offline 試験

measure-probe の verdict が feasible なので進めた。daemon・task-*・web のコードは変えていない。基準は `$CELERIS_WU_BASE`。

## 作ったもの
- `scripts/campaign/ops-r1-measure-config.sh`（POSIX sh、dash で動く）: `--state --release-dir --models --out` で daemon.toml を書く。任意の option は `--port`・`--harness real|stub`（既定 real）・`--stub-command`・`--claude-bin`（既定 `claude`）・`--claude-accounts`・`--clusters-from`・`--skills-src`（既定 repo の `config/skills`）。
  - db・`[[api.tokens]]` の token・knowledge・workspace・scratch を `--state` の下に置く。`[api] listen` は 127.0.0.1 の空き port（`--port` で固定可）。
  - `[cos] enabled = true`、`[cos.triage] policy_skill = "cos-inbox-triage"`・`policy_version = "6"`。
  - CoS skill の配送: `config/skills` の SKILL.md を持つ各 dir（cos-operator・cos-inbox-triage・yomiyasu ほか）を `<state>/kb/skills/` へ写す。
  - route: `--models cos-opus` は CoS = claude-opus-5-5、`[route_defaults.claude_code]`（planner/worker/reviewer）= claude-sonnet-5-5。`all-sonnet` は全役割 claude-sonnet-5-5。
  - 並列 1: `max_concurrency = 1`・`[execution] max_parallel_work_units = 1`・`[execution.tree] max_parallel_child_tasks = 1`・（`--claude-accounts` 時）`[accounts] max_runs_per_account = 1`。試行: `max_requeues = 0`。
  - 試行専用の案件の body を `<state>/setup/project.json` に書く。案件と案件 CoS thread は daemon 起動後に API で作る（probe の結論。手順は docs/ops）。
  - 拒否（exit 2）: `--state`・`--out` が `~/.config/celeris`・`~/.local/celeris`・`/local/celeris/state` の下（symlink を解いてから判定。何も書く前に止まる）、未知の `--models`、使い方の誤り。release dir に `bin/celeris`・`manifest.json` が無ければ exit 1。
- `scripts/campaign/test_ops_r1_measure_config.py`（unittest、合成の release dir・一時 state・一時 HOME だけ）: 9 件。stub の全欄、real + all-sonnet + accounts + clusters の写し、空き port、拒否 4 例（~/.config/celeris の state・symlink 経由の ~/.local/celeris の out・/local/celeris/state の out・未知 models）、使い方・入力の誤り、同じ入力で byte 一致（token は保つ）。
- `docs/ops/ops-r1-measure-daemon.md`: 生成・起動確認・案件と CoS thread の作り方。

## 証拠（提出時の HEAD で実行）
- `python3 -I scripts/campaign/test_ops_r1_measure_config.py` → `Ran 9 tests ... OK`（exit 0）。
- 実 binary での読み込み確認（試験外、一時 state と env -i、run の TMPDIR の下）: `ops-r1-measure-config.sh --release-dir /local/celeris/state/releases/94f86928dc13 --models cos-opus --harness stub|real ...` で作った config を `/local/celeris/state/releases/94f86928dc13/bin/celeris --config <daemon.toml>` で起こし、両 mode とも `GET /api/v1/health` が `"schema_version":102`・`"cos_policy_version":"6"`・`"pricing_version":"2026-09-approx-1"` を返した（timeout 15 秒で停止、exit 124）。
  - この確認で、`--claude-accounts` 無しの `[accounts]` は `invalid config: [accounts] requires at least one of claude_dir / codex_dir / opencode_dir`（exit 2）になると分かった。script は `--claude-accounts` のときだけ `[accounts]` を書く。
- `bash scripts/dev/test-affected.sh` → exit 0（「試験対象の変更なし」。scripts/docs の検査 self-test は全部 OK）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` → `docs/ops/ops-r1-measure-daemon.md`・`scripts/campaign/ops-r1-measure-config.sh`・`scripts/campaign/test_ops_r1_measure_config.py`（と本ファイル）。

## 未解決事項
- 試行 1 回の上限: daemon config に task 全体の max_attempts 欄は無い（probe の結論）。config では `max_requeues = 0` だけを書いた。stub CoS は起票 body の `budget.max_retries = 0` で、実 LLM の CoS は org profile の `budget.max_attempts = 1` で固定する想定。org が空の DB では担当が決まらず profile の上限が効かないので、どちらで固定するかは measure-e2e と preflight で確かめる（未確認）。
- `--models` の key（`cos-opus`・`all-sonnet`）は script で決めた名前。決定 d-ops-r1-models はまだ option key 付きで起票されていない。決定を起こすときは同じ key を使う。
- `celeris-campaign config-check` との突き合わせは未実行。KB は skills を含むので、`--kb-src` に同じ中身の固定 KB を渡す必要がある。`--clusters-from` で通常運用の `[[clusters]]` を写す。
- 未確認: probe の「`budget.max_retries=0` を create_task_op が受けるか」は measure-e2e で確かめる。

## 提案
- 測定の固定票には daemon.toml の sha256 と、見出しの release sha12・bin/celeris sha256 を載せる（同じ入力で byte 一致するので再生成で照合できる）。
