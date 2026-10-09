# ledger-fixture 進捗

## 原因

integrate-ledger で task-worker 2 件と celeris browser doctor 1 件が失敗した。ADR と `scripts/browser-conformance.py` の生成規則は、credential の必須証拠を case ごとに runtime 付きで記録する設計だった。isolation/egress は launcher 実証、P4-B injection/auth 観測は daemon 実証である。一方 task-core の機密 capability 判定は isolation/egress の launcher 条件を全件へまとめて適用し、残る case は runtime を区別しなかった。task-worker fixture は全 case を launcher にし、celeris fixture は runtime 欄を省いて daemon 扱いにしていたため、生成器出力と一致しなかった。

## 修正

- `certify` は機密 capability の各必須 case を、isolation/egress は launcher、それ以外（現行 P4-B cases）は daemon の証拠で個別に認定する。runtime 不一致を拒否する回帰 assertion を追加した。
- task-worker と celeris の ledger fixture を同じ case 別 runtime 形式に変更した。
- ADR の台帳契約を case 別 runtime に明記した。生成器は既に各証拠行へ指定 runtime を出していたため変更不要。

## 検証

短い `TMPDIR=/tmp` を使って実行した（長い run TMPDIR では celeris の Unix socket path が `SUN_LEN` を超えるため）。

- `cargo test -p task-core browser_backend::tests::p4b_cases_count_only_with_measured_evidence` — pass
- `cargo test -p task-worker credential_use_is_released_only_by_p4b_evidence_in_the_ledger` — pass
- `cargo test -p task-worker released_ledger_still_refuses_unconformant_backend_and_unisolated_runtime` — pass
- `cargo test -p celeris browser_doctor_launcher_dns_credential_ping_and_versions_are_checked` — pass
- `python3 -m unittest scripts.tests.test_browser_conformance_credential` — 7 tests pass

- `cargo clippy --workspace -- -D warnings` — pass
- `cargo check --workspace --tests --keep-going` — pass
- `sh "$CELERIS_WU_SCOPE_PATHS"` — pass（変更は ADR・進捗・task-core/task-worker/celeris の fixture/test のみ）
