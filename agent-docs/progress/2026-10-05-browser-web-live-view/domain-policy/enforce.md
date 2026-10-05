# 実効許可 task ∩ grant の強制（broker・egress）と grant 縮小の即時適用

---
tasks: [01M470CJRXMPWS39S14PN7XPFP]
status: done
completed: 2026-10-05
---

## 変更

- task-core `browser.rs`: `BrowserTaskPolicy::within_task_origins` と `task_run_policy`。保存 policy の `network_domains` を `Task.requirements.browser.allowed_domains` と origin で交差する（scheme・host・port、wildcard は包含）。空なら `empty_browser_domains`。requirements の無い旧 task は保存 policy のまま。
- task-worker `browser_policy.rs`: `prepare_for_task`（狭めてから grant と `derive`）、起動前判定 `admit`、egress の許可 `PreparedBrowserPolicy::egress_allow`。`browser.rs` の準備と egress はこれを使う。
- task-dispatch `worker_task.rs`: `browser_run_policy` で狭めた policy を `RunContext.browser_policy` に入れ、run 起動前に `admit` を呼ぶ。交差が空・不正なら process を起動せず `browser policy rejected: <code>` で失敗。grant は `run_extras` が run ごとに org から読む（作成時の grant は保存しない）。
- ADR `agent-docs/adr/2026-10-05-browser-allowed-origins.md` に「強制と grant 縮小の伝え方」を追記（実行中の run には流さず次の run から。binding hash が変わるので縮小前の承認・待ちは次の判定で拒否）。
- worker protocol の型は変えていないので docs/protocol の再生成は不要。

## 試験（browser_allowed_domains_）

- task-core: `..._intersection_is_task_and_grant`、`..._scheme_and_port_mismatch_leave_empty_intersection`
- task-worker: `..._broker_denies_outside_task_and_grant`（grant 外・task 外・scheme・port）、`..._egress_allow_is_task_and_grant`、`browser_egress::tests::..._egress_denies_outside_task_and_grant`（CONNECT を DNS 前に拒否、交差内は通す）、`..._grant_shrink_applies_to_existing_task`、`..._empty_intersection_is_refused_before_run`
- task-dispatch: `..._grant_shrink_reaches_next_run_context`（org の grant を縮めると同じ task の次の run context で縮小後の交差、空なら拒否）
- いずれも userns・実 browser 不要。

## 検査

- `cargo test -p task-core --lib browser_allowed_domains_`: 9 passed（うち本葉 2）
- `cargo test -p task-worker --lib browser_`: 67 passed
- `cargo test -p task-dispatch --lib browser_allowed_domains_`: 1 passed
- `bash scripts/dev/test-parallel.sh`: exit 0（nextest 3964 passed / 0 failed / 13 ignored、doctest exit 0）
- `cargo clippy --workspace --all-targets -- -D warnings`: exit 0、`cargo fmt --all --check`: exit 0

## 未解決

- egress は `host:port` の完全一致のまま（wildcard origin は egress では当たらず fail-closed。以前から）。
- launcher 経路（browser_launcher）は prepared policy を受け取るだけで、本葉では別の検証を足していない。
- 保存 policy の無い browser task は従来どおり `browser_policy_required` で拒否する（requirements から policy を合成しない）。
