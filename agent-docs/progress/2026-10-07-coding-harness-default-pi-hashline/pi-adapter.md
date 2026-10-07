---
title: Pi + Hashline の軽量 worker adapter（task-worker）
tasks: [01M49X6CW5KE5HY0EZRQR0S045]
status: done
updated: 2026-10-07
---
# Pi + Hashline の軽量 worker adapter（WorkUnit pi-adapter）

ADR `agent-docs/adr/2026-10-07-coding-harness-default-pi-hashline.md` の「後続葉 pi-adapter」。ACP ではなく新 adapter `pi`（`--mode json -p`、1 run = 1 process）。完了日 2026-10-07。

## 実装
- `crates/task-worker/src/pi.rs`（新規）: `PiAdapter`（`ID = "pi"`）・`PiConfig`。
  - 固定引数: `--mode json -p --no-extensions --no-skills --no-prompt-templates --no-themes --session-dir <run>/pi-sessions --provider <p> --model <p>/<id> --tools <allowlist>`、各 extension を `-e <path>`。prompt は stdin（長い prompt で argv 上限に当たらない）。
  - `PI_CODING_AGENT_DIR=<run>/pi-agent`（本物の `~/.pi/agent` を読まない）。`settings.json` で Pi 内 retry と install telemetry を切る（retry は Celeris が持つ）。
  - 検証（`PiConfig::validate`、config 読み込み時と run 開始時）: extensions（Hashline）と tools allowlist が必須、tool 名に subagent/planner を含めたら拒否、`args` は実行ファイル prefix だけ（`-` 始まり・`@` 始まりは拒否して固定引数の上書きを防ぐ）、model は `provider/id`。extension path が無ければ spawn 前に失敗（黙って Hashline 無しで走らない）。
  - プロンプト・作業場所・skills: claude-code と同じ `build_prompt` + `skills::deliver_agent_skills` + preamble、cwd は `req.cwd()`、result.json / delegate.json / run log（stdout.log・stderr.log・request・prompt）も同じ契約。
  - usage: assistant の `message_end` の `usage.{input,output,cacheRead,cacheWrite,cost.total}` を `Usage` に合算（retry の途中失敗 message も token は数える）。
  - 失敗分類: `stopReason` error/aborted の `errorMessage`、stderr を `classify_provider_failure` へ。`GoUsageLimitError` → Throttled。extension の読み込み失敗は retryable=false。`agent_end` が無い・非 0 exit は retryable Error。wall clock / idle timeout は process group を kill。
  - opencode-go pool: 選ばれた account dir（`XDG_DATA_HOME`）の `opencode/auth.json` から鍵を `OPENCODE_API_KEY` に渡す（auth.json は写さない）。openai_compatible / llm-proxy（`celeris/<tier>`）は run ごとに `models.json` を書き、鍵とルーティング文脈 header は env 参照で渡す。
- `crates/task-worker/src/opencode_account.rs`: `read_go_key` を crate 内公開。

## 範囲の訂正（attempt 2）
attempt 1 は `crates/celeris`（config の `extensions`/`tools` 欄・adapter 許可一覧・`build_adapters` の `pi`）と `crates/celerisctl`（試験の新欄）も変えたが、この葉の範囲 check（`crates/task-worker/` ほか）を外れたため差し戻した。
これらの config/daemon 配線は dispatch 葉（dispatcher/daemon への配線）の範囲。実装済みの差分は commit `a8b4f1e5` に残っているので、dispatch 葉は `git show a8b4f1e5 -- crates/celeris crates/celerisctl` を取り込めばよい（`pi_adapter_*` の config 試験 3 件と `rejects_duplicate_roles_unknown_role_adapter_and_zero_limits` の期待文字列の更新を含む）。

## 証拠
- `cargo nextest run -p task-worker` → 907 passed, 9 skipped（`pi_adapter_*` 14 件を含む: 起動引数・tool 集合・隔離・subagent/planner 拒否・usage・失敗分類・timeout・skills・result 読み取り・go 鍵）。
- attempt 2（celeris/celerisctl を差し戻した後）: `bash scripts/dev/test-parallel.sh` → exit 0、4198 passed / 0 failed / 13 skipped。
- `cargo fmt --all --check` → 差分なし。`cargo clippy --workspace --all-targets -- -D warnings` → exit 0。

## 未解決事項
- Hashline の実物は host に無い（ADR 未確認点 1）。tool 名は config で人が書く。実 Pi + Hashline での動作確認は未実施（外部ネットワーク・実 LLM を使わない方針）。
- session resume（`SUPPORTED_ADAPTERS` に `pi`）は未実装（ADR どおり範囲外）。
- `PiAdapter::ID` と `task_core::CodingHarness::Pi.adapter_id()` の一致試験は family 葉の統合後（integrate-core 以降）に足す。
- codex OAuth を Pi で使う経路は無い（設定で拒否）。

## 提案
- dispatch 葉で `a8b4f1e5` の celeris config/daemon 配線（`ProviderConfig.extensions/tools`、adapter 許可一覧、`build_adapters` の `pi`）を取り込む。
- dispatch 葉で `CodingHarness::Pi.adapter_id() == task_worker::PiAdapter::ID` を固定する試験を置く。
