# ADR 2026-10-18: B/C の能力 probe は測定専用 daemon の process の環境を検査する（CLI の環境で代用しない）

- 日付: 2026-10-18
- 状態: 採用・実装済み
- tasks: [01M57MB8PWVW50KD127M30HEFT]
- 関連: 2026-10-18-measure-env-launcher（D2 同一性照合・D3 daemon の PATH）、2026-10-17-campaign-lock、`crates/campaign/src/{cap_probe,preflight,env_check,main}.rs`

## 背景（再現確認: main 0f9c058642c4）
`WorkerDaemonEnv::from_process(expected)` と `Host::worker_daemon_env` の既定は、preflight / cap-probe を起動した CLI の環境
（`std::env`）から PATH と期待変数を写していた。測定専用 daemon が持つ worker 環境とは別物で、CLI にだけ toolchain・変数があれば
daemon に無くても通り、逆に daemon にあって CLI に無ければ落ちた。また子 process は CLI の環境を継いでいた
（`FakeAdapter` は `Command::envs` を重ねるだけ）。celeris の config に `[worker]` 節は無い（実装済みの接続先ではない）。

## 決定
- D1. B/C の worker 環境は、同一性照合（`env_check::verify`）に通った測定専用 daemon の process（`identity.pid` = launcher 所有の
  `--daemon-pid`）の `/proc/<pid>/environ` から作る（`WorkerDaemonEnv::from_daemon_pid`）。取り出すのは PATH・cap env の
  `worker.env` が名指す変数・非秘密の許可リスト（`HOME LANG LC_ALL TERM USER LOGNAME`）だけ。環境全体は保持も記録もしない
  （他の変数は読んだ buffer ごと捨てる）。`from_process` は削除した。
- D2. 検査は worker の実起動関数（`FakeAdapter` → `run_subprocess`）に通し、各検査を `env -i <D1 の環境> /bin/sh -c …` で走らせる。
  CLI の環境は子に残らない。daemon の PATH・変数・ro_mounts（`test -d`）・socket（`test -S`）・必要能力の欠落は、該当 arm の
  `env_failure`（LLM 呼出し前）。A の sandbox probe と A/B/C の隔離は変えない。
- D3. 検査先 identity を報告に残す: `ProbeReport.daemon` と preflight `Report.probe_targets`（case・arm・pid・starttime・instance_id・
  release）。秘密を含まない。identity 照合に通らない（別 pid・config 不一致・測定 daemon の指定なし）ときは環境を読まず、
  B/C の `capability` は miss（env_failure）。照合は preflight 中 1 回の `GET /api/v1/config` を env-check と共有する。
- D4. `Host::worker_daemon_env(expected, pid) -> Result<…>`。既定は D1。新しい API・設定欄は作らない。`cap-probe --arm b|c` は
  `--manifest --measure-config --daemon-pid --api-url --token-env` を要し、同じ照合に通らなければ検査しない。

## 却下
- daemon の config から `[worker]` を読む: 存在しない。
- 測定 daemon に fake harness の task を投入して検査する: 測定 config に harness の追加が要り（config 変更）、台帳・task を増やす。
  `/proc/<pid>/environ` の PATH は D3（launcher ADR）で既に読む前提の経路。
- `/proc/<pid>/environ` 全体を取得して記録・照合する: 秘密を含み得る。名指した変数だけ。

## 限界（未検証）
- worker は daemon の環境に加え、dispatcher が run ごとに足す env（`CARGO_TARGET_DIR`・`TMPDIR` 等）と adapter 設定の env を持つ。
  それらは取らない（toolchain の有無を左右しない前提）。daemon config の `[adapters.*.env]` で toolchain 変数を足している場合は
  cap env の `worker.env` にも書く運用とし、食い違いは `worker-env` として落ちる。
- mount / socket の存在は検査 process（daemon と同じ利用者・同じ host）から見た `test -d/-S`。daemon が別 mount namespace なら見え方が違う。
