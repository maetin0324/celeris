---
title: hotfix — base の検査器と一緒に arch-map の検査器を出す（review の health_check_error）
tasks: [01M6TYVJSWEY3ZE50S02ETYDNM]
status: done
updated: 2026-11-10
---
# hotfix — base の検査器と一緒に arch-map の検査器を出す

完了日: 2026-11-10

## 経緯
G3（task 01M6TYVJSWEY3ZE50S02ETYDNM）が入れた検査器の起動の口の修正。operator が手で入れた（Celeris の task は無い）。

G3（健全性v2）の release を本番に昇格した直後、task 01M72DBBE7AFCNXYWSB5M0YZW9 の review の health が
`health_check_error: unsupported:arch-map.missing_row(checker_missing: /tmp/celeris-review-policy-…/tree/scripts/dev/check-architecture-map.py が無い)`
ほか arch-map.* の 6 rule で止まり、受信箱に認可の項目が出た。`with_base_checker` が一時 dir に出す file に、
rule arch-map.* が検査器の dir の外から import する `scripts/dev/check-architecture-map.py` が無かった。
WU の check の health（`check_item`）も同じ口を通るので、Celeris の repo のどの task も同じ理由で止まる。

## 変更
- `crates/task-core/src/required_checks/health.rs`: 定数 `ARCH_MAP_CHECKER` を足し、`CHECKER_SIDE_FILES` に入れた。
- `crates/task-dispatch/src/health_review/policy/tests.rs`: 試験 `health_review_base_checker_extracts_every_side_file`。
- ADR `agent-docs/adr/2026-11-08-health-enforcement-loop.md` の「状態の記録」に付記（2026-11-10 hotfix）。

commit: 「hotfix: base の検査器と一緒に arch-map の検査器を出す（review の health_check_error）」

## 証拠（提出時の HEAD で実行）
- `cargo fmt --all -- --check` → exit 0
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo nextest run -p task-dispatch -p task-core -E 'test(/health_review_|health_evidence_|base_checker/)'` → 131 tests run: 131 passed
- `cargo nextest run -p task-dispatch -E 'test(health_review_base_checker_extracts_every_side_file)'` → 1 passed
- 本物の検査器での再現と確認: main の `scripts/dev/repo-health/` と `source-size-report.py` だけを `git archive` で出した版は
  `repo_health.py --repo <repo> --base <G4 exc-expiry の merge> --head <その後の main>` が exit 2 で arch-map.* の 6 rule が
  unsupported、`check-architecture-map.py` も出した版は exit 0 で 6 rule が pass。
- `sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-adr-numbers.sh`・`sh scripts/dev/progress-index.sh --check` → ok
- `bash scripts/dev/check-health.sh` → exit 0（repo_health_gate: all checks passed）
- `bash scripts/dev/test-affected.sh` は走らせていない。task-core の変更で workspace のほぼ全部が対象になり、本番の止まりを
  早く直すため、全体の試験は release の gate（`cargo nextest run --workspace`）に任せた。

## 未解決
1. `checker_sha256`（D2.3）の式は arch-map の検査器の版を入れない。検査器の版の同一性に arch-map の検査器も入れる。
2. release の同梱（`scripts/selfdeploy/lib.sh` の `sd_bundle_repo_health`）は arch-map の検査器を含まない。

## 提案
- 検査器が自分の dir の外から読む file を、検査器の側（`repo_health.py`）が一覧で出し、Celeris の定数と試験で一致を確かめる。
