---
task: browser-prod-enablement
wu: daemon-path
status: done
completed: 2026-10-08
tasks: [01M4CH1MPV1KB69SYKXS79E0SP]
---

# daemon-path: daemon が release dir の適合台帳を configure_conformance で worker に渡す（D1.4）

ADR [2026-10-08-browser-prod-enablement](../../../adr/2026-10-08-browser-prod-enablement.md) D1.4 の path 解決。

## したこと
- `crates/celeris/src/daemon/browser_ledger_path.rs`（新規）:
  - 純関数 `conformance_path_for(env, exe, release)`: (1) `CELERIS_BROWSER_CONFORMANCE_FILE`（空は無い扱い）→
    (2) `--release <sha12>` のとき `current_exe()` の親の親（`<releases>/<sha12>`）`/browser/conformance.json` →
    (3) どちらも無ければ `None`（未配置）。
  - `configure_from_process(cli_release)`: 上を process の値で解き、`Some` なら
    `task_worker::browser_ledger::configure_conformance(path, release)` を呼ぶ。`None` は info ログだけで起動は続ける
    （gate が browser task だけを止める）。`std::env::set_var` は使わない。
- `daemon/run.rs`: 昇格 gate・hot mount 検査の後、`build_dispatcher` の前で `configure_from_process(opts.release)` を呼ぶ。
- release は `--release` の値だけを見る（`CELERIS_RELEASE` env は D1.4 の 2 の条件に入れない）。

## 証拠
- `cargo test -p celeris --lib browser_ledger_release_` → exit 0、3 passed（release dir・env 優先・None）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `bash scripts/dev/test-parallel.sh` → exit 0、4735 passed, 13 skipped。

## 未解決事項
- なし（celerisctl 側の `browser_ledger_release_` 試験と selfdeploy の台本は他の WU）。

## 提案
- systemd の unit が `--release` でなく `CELERIS_RELEASE` だけを渡す構成になったら、(2) の条件を
  `InstanceIdentity.release != "dev"` に広げる。
