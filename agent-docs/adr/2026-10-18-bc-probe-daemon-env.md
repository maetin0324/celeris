# ADR 2026-10-18: B/C の能力 probe は測定専用 daemon の通常の worker 起動経路で走らせる（CLI の環境で代用しない）

- 日付: 2026-10-18
- 状態: 採用・実装済み
- tasks: [01M57MB8PWVW50KD127M30HEFT]
- 関連: 2026-10-18-measure-env-launcher（D2 同一性照合・D3 daemon の PATH）、2026-10-17-campaign-lock、`crates/campaign/src/{cap_probe,preflight,env_check,main}.rs`

## 背景（再現確認: main 0f9c058642c4）
`WorkerDaemonEnv::from_process(expected)` と `Host::worker_daemon_env` の既定は、preflight / cap-probe を起動した CLI の環境
（`std::env`）から PATH と期待変数を写していた。測定専用 daemon が持つ worker 環境とは別物で、CLI にだけ toolchain・変数があれば
daemon に無くても通り、逆に daemon にあって CLI に無ければ落ちた。子 process も CLI の環境を継いでいた。celeris の config に
`[worker]` 節は無い（実装済みの接続先ではない）。worker の環境は daemon の環境 + adapter の `env` + run ごとの env（dispatcher が
足す）で、組み立てるのは daemon の dispatcher なので、CLI 側で再現した環境は証拠にならない。

## 決定
- D1. B/C の検査は、identity 照合（`env_check::verify`）に通った測定専用 daemon に、無 LLM の検査 task を通常の
  `POST /api/v1/tasks` で投入して走らせる（`cap_probe::run_in_daemon`）。検査 task は arm の worker が実際に使う harness
  （manifest の `routing.run_harnesses`。無ければ `harness`）ごとに 1 件、分野 `cap-probe-<harness>` で投入する。測定 config はその分野を
  `kind = "fake"` + `env_of = "<harness>"` の検査 harness に結ぶ。`env_of` は、実 harness `<harness>` の adapter 環境
  （`[adapters.<kind>].env` + secrets。Pi は宣言の env）を、実 adapter と同じ関数（`declared_env`）で組んで検査 worker に渡す。
  daemon の dispatcher が通常どおり worker process を起こすので、daemon の環境・その harness の adapter env・run ごとの env・
  権限・cwd の組立は daemon のもの。検査 task の実行内容（command）だけが無 LLM の検査になる。新しい API・store・スケジューラは
  作らない。claude_code 等の adapter env が toolchain/PATH を足す・壊す構成は、その harness の結果として現れる（`<harness>/<id>`）。
- D2. worker は `celeris-campaign cap-worker`（検査 harness の `command`/`args`）。stdin の `run` の task objective
  （`WorkerProbeSpec`: 必要能力の argv・mount の `test -d`・socket の `test -S`・期待 env）を読み、worker process の環境のまま
  `/bin/sh -c` で走らせ、能力の有無と識別（id・ok・detail）だけを `done` の summary に返す。環境変数は読み出さず（期待値との
  照合だけ）、値・stdout は返さず・記録しない。不足（PATH 要素・変数・mount・socket・必要能力）は該当 arm の `env_failure`
  （LLM 呼出し前）。A の sandbox probe と A/B/C の隔離は変えない。
- D3. 検査先 identity を報告に残す: `ProbeReport.daemon` と preflight `Report.probe_targets`（case・arm・pid・instance_id・
  release・`probe_task_id`）。秘密を含まない。identity 照合に通らない（別 pid・config 不一致・測定 daemon の指定なし）ときは
  検査せず、B/C の `capability` は miss（env_failure）。照合は preflight 中 1 回の `GET /api/v1/config` を env-check と共有する。
- D4. `Host::probe_daemon_worker(api_url, token_env, spec)`。`cap-probe --arm b|c` は `--manifest --measure-config --daemon-pid
  --api-url --token-env` を要し、同じ照合に通らなければ検査しない。検査 task は測定 daemon の task を増やすので、B/C の probe は
  preflight の最後（他の検査の後）に走る。
- D5. 測定 daemon の config に、run_harnesses の各 `<h>` について harness `cap-probe-<h>`（`kind = "fake"`, `command`/`args` =
  cap-worker, `env_of = "<h>"`）と、分野 `cap-probe-<h>` → その harness の `[harness_selection]` 規則を足す（cap-runbook §1）。
  無ければ検査 task は走らず、待ち時間の後に `<h>/worker-run` の `env_failure`（モデル失敗ではない）。`env_of` は
  `kind = "fake"` だけに書け、宣言済みの harness を指す（config 検証）。
- D6. 検査 task が通らない文脈: 試行の task-template（文脈）・permission_mode・harness の command 自体。B と C は同じ
  run_harnesses なので同じ環境を検査し、arm 差は報告ラベルだけ（manifest が B/C の run_harnesses の一致を要求する）。

## 却下
- daemon の config から `[worker]` を読む: 存在しない。
- `/proc/<pid>/environ` から PATH・名指し変数を読み、CLI 内の `FakeAdapter` → `run_subprocess` に `env -i` で渡す: dispatcher が
  足す env・adapter env・権限・cwd を通らず、daemon の worker 起動の証拠にならない（誤判定もあり得る）。
- `/proc/<pid>/environ` 全体を取得して記録・照合する: 秘密を含み得る。

## 試験
- `crates/celeris/tests/cap_probe_real_daemon.rs`: 実 `celeris` binary を `env_clear` した一時 daemon（一時 DB・loopback・
  検査 harness `env_of`）として起動し。`probe_runs_under_each_real_harness_adapter_env`: claude-code の adapter env が PATH に
  toolchain を足せばその harness だけ通り（codex は env_failure）、adapter env が toolchain を落とせば daemon の process 環境が正しくても
  その harness は env_failure。、実 HTTP の検査 task が実 dispatcher → 実 `cap-worker` process を通る。CLI（試験 process）だけに toolchain・変数が
  あり daemon に無ければ該当 arm の `env_failure`、daemon にだけある toolchain・変数は通る。socket・mount の存在は worker 側から見る。
  identity（`GET /config` の pid が起動した daemon の pid）と `probe_task_id` を記録し、秘密・HOME・変数値は報告にも task の event にも出ない。
- `crates/campaign/tests/{launcher,trial_state_cli_e2e}.rs`: daemon 役の dispatcher 代役が実 `cap-worker` を daemon の環境で起こす。

## 限界（未検証）
- 実の測定専用 daemon（launcher が起動した本番相当の process）での再実走は人の手順（cap-runbook §1）。
- harness 自体（claude/codex の command・permission_mode・container 化）と試行の文脈は通らない（D6）。環境（daemon + adapter env + run env）の検査である。
- mount / socket の存在は worker process から見た `test -d/-S`。daemon が別 mount namespace で worker を起こす構成ではその見え方が検査対象になる（意図どおり）。
