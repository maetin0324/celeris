# ADR-0112: P4-B trusted injection の適合を実測証拠つきで記録し、機密能力を記録からだけ解放する

---
tasks: [01M3RK6XG30KD3QC0Z67XBB5G5]
---

- 日付: 2026-09-30
- 状態: Accepted
- 関連: [ADR-0106 conformance dispatch](0106-browser-phase4-conformance-dispatch.md)、[ADR-0109](0109-browser-p4b-injection-ipc-cdp-sink.md)、[ADR-0110](0110-browser-p4b-h3-shared-cdp-trusted-selector.md)、[ADR-0111](0111-browser-p4b-redisplay-guard-wiring.md)

## 背景

ADR-0106 の ledger（`ConformanceResult.passed`）は fixture 件名の集合だけで、`injection_attack_suite`・`auth_section_observation_stop` を書き込めばどんな根拠でも `CredentialInjection`（= `CredentialUse` の起動前必須能力）と `IdentityRestore` が解放された。P4-B の実攻撃行列（A0〜A17、A8 は ADR-0111 で合格）と本番 H3 e2e が通った今、その結果そのものを記録に残し、記録からしか解放が起きないようにする。

## 決定

1. `ConformanceResult` に `evidence: [{case, test, outcome}]`（`outcome` = `passed|failed|not_run`）を足す。P4-B の 2 件は `passed` に載るだけでは通ったと数えず、`required_evidence(case)` が挙げる試験名が全て `passed` で並び、同じ件に `failed`/`not_run` が 1 件も無いときだけ通る（`ConformanceResult::case_passed`）。
   - `injection_attack_suite`: `browser_injection_attacks::real_browser_injection_attack_matrix#<印>`（A0, A1, A2, A3, A3b, A4, A5, A6, A7, A7a, A8, A8n, A9, A9a, A10〜A17 の 22 件）
   - `auth_section_observation_stop`: `browser_h3_injection::production_h3_injects_once_without_exposure`、`browser_h3_injection::injected_leak_is_caught_by_the_same_scanner`
2. 記録は `scripts/browser-conformance.py --p4b-evidence <測定済み ledger> --p4b-backend <id> --output-dir <dir>` が実試験を走らせて作る。1 件でも通らなければ証拠は `failed`/`not_run` のまま残り、2 件は `passed` から外す（fail closed）。静的な適合登録・コード内の既定適合は足さない。旧形式（証拠なし）の ledger では機密能力は拒否のまま。
3. 解放後も、(a) 記録の無い backend、(b) 隔離 runtime（bwrap・sandboxd・egress・resolver）が揃わない worker では拒否する。後者は `isolated_runtime_ready` として routing とは別に判定し、ledger が代わりになることはない。
4. ADR-0110 の本番 admission（broker は `Attested` のみ受け入れ、同一 UID の host は `SameUid` で拒否）は変えない。記録による解放は routing の起動前判定を満たせるようにするだけで、この host の本番経路では実注入は拒否のまま。P4-A の `isolation_suite`・`egress_negative_suite` の証拠化は本 ADR の範囲外（件名集合のまま）。

## 帰結

- 本番の ledger は runner の P4-B 実行を経ない限り機密能力を名乗れない。試験名を変える場合は `task_core::browser_backend::P4B_ATTACK_MARKS`/`P4B_H3_TESTS` と runner の同名定数を揃える。
- 本番昇格はしない。
