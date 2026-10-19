---
tasks: [01M58VFRBA6HG0SSF3CG5BKM91]
unit: daemon-decl
status: done-pending-operator-input
updated: 2026-10-19
---
# 測定専用 daemon config 雛形（cap-probe 宣言）と launcher env 例

## 追加（`agent-docs/progress/2026-10-16-mlsys-final/`。秘密なし・既存 secret 参照のみ）
- `measure-daemon.example.toml`: final.json の B/C `routing.run_harnesses = ["claude-code"]` の全 harness に `cap-probe-claude-code`（kind=fake・`env_of = "claude-code"`・command=celeris-campaign `cap-worker`）、`[llm_sources.cap-probe]`、`[route_defaults.cap-probe-claude-code]`、`harness_selection.rules` の先頭に分野 `cap-probe-claude-code` の規則。実 harness は `claude-code` のみ宣言（未宣言 harness は使わない）。probe は分野 `cap-probe-*` の task だけに結ばれ、実試行の候補に入らない。`<...>` は operator が実値にする（abs path・port）。knowledge/memory/cron/langmem/gc は config-check（measure_env::check_config）の条件を満たす形。
- `measure-launcher.example.env`: MANIFEST / DAEMON_CONFIG / NORMAL_CONFIG / CANARIES / TASK_TEMPLATE / A_ACCOUNT / CELERIS_BIN / RELEASE / OUT / STATE_DIR / CAP_ENV / 各 *_SRC / CAMPAIGN_BIN。実値で決まらないものは `<operator 入力待ち: …>` と明示（置換前は prepare が拒否）。
- `crates/campaign/tests/measure_daemon_example.rs`: 雛形に対する宣言検証の試験 3 件。

## 宣言検証の結果
- `cap_probe::declaration_problems_of_file(measure-daemon.example.toml, run_harnesses)` → 問題 0 件（試験 `template_declares_every_run_harness_probe`）。
- 雛形の harness は run_harnesses ∪ その cap-probe のみ（`template_has_no_undeclared_probe_or_real_candidate_probe`）。宣言を欠く config は `no harness_selection rule` で拒否（`missing_probe_declaration_is_rejected`）。
- 実 daemon の `Config::load` でも通る（placeholder を一時 dir の実 path に置換した写しで確認。一時試験は削除）。当初 `harness_selection.execution_class_defaults` 欠落で Parse エラーだったので雛形に `{}` を追加した。
- 既存成果をそのまま利用: CAP_ENV toolchain 内容 pin・env-lock/3、Rust probe の `--target-dir` 固定、単独 cap-probe の B/C 宣言検証（変更なし）。

## operator 手順（実行しない。本番 config・release は触らない）
1. `measure-daemon.example.toml` を複製し `<...>` を実値に（db・workspace_root・KB root・token_file は本番と別の NFS 外 path、port は本番と別、command は `CAMPAIGN_BIN` と同じ実体）。`NORMAL_CONFIG` の `[[clusters]]` を複写。
2. `measure-launcher.example.env` を複製し operator 入力待ちを埋める（manifest の harness_version/ A_ACCOUNT / RELEASE は H1 昇格後）。
3. `docs/ops/measure-run-final.md` §5 の `prepare` → `start` → `preflight`（runbook-preflight unit が整合）。

## 未解決
- 実値（RELEASE・CELERIS_BIN・A_ACCOUNT・port・path）は operator 入力待ち。実 daemon 起動下の cap-probe 実走は未実施（有料/daemon 起動を伴うため）。
