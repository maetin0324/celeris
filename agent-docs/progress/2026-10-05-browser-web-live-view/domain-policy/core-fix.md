# core 段の統合失敗の修正（core-fix）

---
tasks: [01M470CJRXMPWS39S14PN7XPFP]
status: done
completed: 2026-10-05
---

## 直したもの
- e2e の `task_core::Task` リテラル 3 箇所（tests/e2e/tests/scenarios.rs ×2・worker_db_read_only.rs）に `requirements: Default::default()` を足した（E0063）。
- task-worker の credential 要求の判定を origin で行う。`host_in_domains`（host の文字列比較）を `origin_in_domains`（`task_core::browser::origin_covers`）に置き換えた。→ `browser::tests::credential_request_origin_outside_effective_domain_is_denied` が通る。
- `browser_policy::tests::prepared_file_is_hash_bound_nonempty_default_deny` は fixture を origin 形式に直した（grant・network_domains・期待値を `https://example.com`）。実効値は origin の正規形で、期待を緩めたものではない。
- 同じ原因で落ちた試験（実効 allowed_domains が origin 形式になったのに、実行側が host 形式を前提にしていた）:
  - `task-dispatch dispatcher::tests::browser_fallback::dispatch_browser_fallback_primary_fails_alternate_runs_in_fresh_session`: shim（browser_cli.py）の config 検証が host の正規表現だったため policy を拒否し、close が失敗していた。
  - `browser_control_gate_wire` の 5 件と `python_transport_security_regressions_are_part_of_workspace_gate`（scripts/tests/test_browser_cli.py）: fixture が host 形式だった。
- 対応: 照合を origin（scheme・host・port）に揃えた。
  - `browser_policy::url_origin_allowed`: action server の open と shared CDP の navigate で使う。
  - `browser_policy::origin_host_port`: egress の `host:port` と、agent-browser の `--allowed-domains` 用の host を作る。
  - shim の browser_cli.py・browser_action.py の照合（ORIGIN 正規表現と origin_allowed）。
  - fixture を origin 形式に直した。新しい試験 `url_and_egress_follow_allowed_origin_scheme_host_port` を足した。
- worker protocol の型は変えていないので、docs/protocol の再生成は不要。

## 検査
- `cargo check --workspace --tests`: exit 0
- `cargo test -p task-worker --lib -- prepared_file_is_hash_bound_nonempty_default_deny credential_request_origin_outside_effective_domain_is_denied`: 2 passed
- `bash scripts/dev/test-parallel.sh`: exit 0（nextest 3956 passed / 0 failed / 13 ignored、doctest exit 0）
- `cargo clippy --workspace --all-targets -- -D warnings`: exit 0。`cargo fmt --all --check`: exit 0
- 無関係の flaky は出なかった。

## 未解決・enforce 葉への申し送り
- 実効許可の照合は origin になったが、enforce 葉の仕事は残る。grant 縮小の即時適用と、broker・egress での grant 外拒否の試験（browser_allowed_domains_）がそれにあたる。
- egress は `host:port` の完全一致のままで、wildcard の origin（`*.example.com:443`）には当たらない（以前から）。
- launcher 経路（browser_launcher/backend.rs・protocol.rs の allowed_domains 検証）は、この葉では見ていない。
