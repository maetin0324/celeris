# ops-r1 測定 daemon: config の生成と試行専用の案件・CoS thread の用意

---
tasks: [01M62V0FQMMYQ1S83GBXQP7A6J]
---

測定 daemon は本番昇格済み release の binary をそのまま使い、config だけで本番と分ける。設計の根拠は `agent-docs/progress/2026-10-29-k4-ops-r1-pilot/measure-probe.md`。本番 daemon・本番 DB・`~/.config/celeris` には触れない。

## 1. config を作る

```sh
sh scripts/campaign/ops-r1-measure-config.sh \
  --state /local/celeris/data/measure/ops-r1/state \
  --release-dir /local/celeris/state/releases/94f86928dc13 \
  --models cos-opus \
  --claude-accounts <claude の account dir> \
  --clusters-from ~/.config/celeris/config.toml \
  --out /local/celeris/data/measure/ops-r1/daemon.toml
```

- `--models`: `cos-opus`（CoS = claude-opus-5-5、planner/worker/reviewer = claude-sonnet-5-5）か `all-sonnet`（全役割 claude-sonnet-5-5）。それ以外は exit 2。
- `--state`・`--out` が `~/.config/celeris`・`~/.local/celeris`・`/local/celeris/state` の下なら exit 2（symlink は解いてから判定する）。
- `--port` を省くと 127.0.0.1 の空き port を選ぶ。`--port` を含めて同じ入力なら daemon.toml は byte 一致する。
- `--claude-accounts` を省くと `[accounts]` を書かない（release の daemon は claude_dir 無しの `[accounts]` を拒否する）。`max_runs_per_account = 1` は `--claude-accounts` と一緒に書く。
- `--clusters-from` は通常運用の `[[clusters]]` を写す（`celeris-campaign config-check` は通常運用と同じ clusters を要求する）。
- 無 LLM の通し（試験・preflight）は `--harness stub --stub-command <stub>`。harness id は `claude_code` のまま `kind = "fake"` になり、`[adapters.fake]` が stub を起こす。実測定との差は harness・source・adapter の 3 節だけ。

書かれるもの:
- daemon.toml: db・`[[api.tokens]]` の token・`[knowledge] root`（`default_mounts = ["kb:trial"]`、langmem・gc 無効）・workspace・scratch を `--state` の下、`[cos] enabled = true`・`[cos.triage] policy_version = "6"`・`[cos] route_candidates`・`[route_defaults.claude_code]`・`[harness_selection]`（全 task を claude_code へ。無いと CoS が起票した task が route されない）、`max_concurrency = 1`・`max_requeues = 0`・`[execution] max_parallel_work_units = 1`・`[execution.tree] max_parallel_child_tasks = 1`。
- `--state` の下: `token`（0600、既にあれば保つ）、`kb/skills/<name>/`（repo の `config/skills` の各 skill。毎回作り直す）、`ws/`、`scratch/`、`setup/project.json`（試行専用の案件の body）。

## 2. 起動と確認

起動は `scripts/campaign/measure-launcher.sh start`（`DAEMON_CONFIG` にこの daemon.toml）か、`<release>/bin/celeris --config <daemon.toml> --release <sha12>`。起動後に確かめる:

```sh
curl -s http://127.0.0.1:<port>/api/v1/health   # cos_policy_version が "6"、release が sha12
```

`celeris-campaign config-check` に `--kb-src` を渡す場合は `<state>/kb` と同じ中身の固定 KB を使う（KB は skills を含むので空ではない）。

## 3. 試行専用の案件と案件 CoS thread

seed（org.toml）は使わず、daemon 起動後に API で作る。`TOKEN=$(cat <state>/token)`:

```sh
curl -s -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d @<state>/setup/project.json http://127.0.0.1:<port>/api/v1/projects          # → project_id
curl -s -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"project_id":"<project_id>"}' http://127.0.0.1:<port>/api/v1/chat/threads  # → 案件 CoS thread
```

人の依頼は、生成 config の token（`operator:ops-r1`）で `POST /api/v1/chat/threads/<thread_id>/relays` に relay として入れる（`celeris-campaign ops-r1-intake --relay --source <出典>`、`docs/ops/ops-r1-intake.md`）。config に人の token を足さない。relay の message は author と `relay.relayed_by` が `operator:ops-r1`、`said_at` が人の承認の時刻になり、出典は intake の記録に残る（ADR `agent-docs/adr/2026-10-30-ops-r2-isolation-principal.md` D5）。

無 LLM の通しは `cargo nextest run -p celeris --test ops_r1_measure` が確かめる（stub harness で 依頼の message → CoS の起票 → worker run → CoS の報告 message、health の `cos_policy_version` = "6"）。この試験は今も試験の中だけで人の token を足して message を送る（ops-r1 の依頼受付の手順ではない。relay への書き換えは別 task、`agent-docs/progress/2026-10-30-ops-r2-isolation-principal/principal.md` の提案）。relay の受付は `cargo test -p celeris-campaign --test ops_r1_intake` が確かめる。

## 試行 1 回の上限

daemon config に task 全体の試行回数の欄は無い。`max_requeues = 0` で供給側失敗の無償の再投入を止める。試行回数は task の `budget.max_retries = 0`（起票 body）か、org profile の `budget.max_attempts = 1`（`PATCH /api/v1/org`、担当が決まった task に効く）で固定する。どちらで固定するかは measure-e2e と preflight で確かめる。
