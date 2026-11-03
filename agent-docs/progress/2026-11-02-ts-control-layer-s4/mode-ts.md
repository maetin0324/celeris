---
title: S4 mode-ts — cos_digest だけ mode ts を受け、人の actor だけが切り替える
tasks: [01M6EVP9GT07K2G01EMW0E6HNC]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S4 mode-ts — cos_digest だけ mode ts を受け、人の actor だけが切り替える

完了日: 2026-11-02（unit mode-ts）。[境界 ADR](../../adr/2026-11-02-ts-control-layer-boundary.md) 付記 S4-14 の「`ts` を受ける範囲と権限」を実装した。

## 結論
- `PUT /script/domains/{domain}/mode` は `cos_digest` に限って `ts` を受ける。`lane_rules`・`route_priority` の `ts` と、どの領域の `canary` も今までどおり 422 `script_mode_not_enabled`。
- `→ts` は人の主体（actor `human:*`）だけ。外部 operator・MCP・CoS・`system:*` は 403 `script_mode_requires_human`。
- `→ts` の元は同じ generation の `compare` だけ。`rust` からの直行と generation の同時変更は 422 `script_mode_ts_requires_compare`。`ts` に generation が無ければ 422 `script_generation_invalid`。
- 戻し（`ts→compare`・`ts→rust`）は誰でも行える（operator・CoS・`system:*` を試験で確認）。
- CoS の `script_domain.mode_set` は route の分類を ALLOWED のまま、本文の `mode` が `ts` なら effect の前に 403（理由 `human_only`）で拒み、`cos_operations` に rejected を記録する。
- migration は要らない（0103 の CHECK は既に `ts` を許す）。Event の variant は足していない。
- dispatcher は変えていない。`compare_target` は `compare` 以外で何もしないので、pass-wire が入るまで `ts` は `rust` と同じに動く。

## 変えたもの
- `crates/task-core/src/script_compare.rs`: `ScriptMode::enabled_in_s2` を消し、`accepted_for(domain)`・`needs_generation()` と `actor_may_enable_ts(actor)` を置いた。`ScriptDomainError::ModeNotEnabled` に領域を持たせ、`TsRequiresHuman`・`TsRequiresCompare` を足した。
- `crates/task-core/src/store/script_domains.rs`: `script_domain_set_mode` に上の規則（主体は transaction の前、元の mode・generation は CAS の版の検査の後）。
- `crates/task-api/src/script_domains.rs`: 新しい code 2 つ（403・422）と検査の順（領域→mode→主体→generation→CAS）。
- `crates/task-api/src/cos/ops/admin.rs`: CoS の `ts` を `audit.reject` で 403 にする。
- `crates/celerisctl/src/commands/script.rs`: `ModeArg::Ts` を足した（可否は API が決める）。
- 文書: `docs/api/v1/api.md` §3.134.2 と CoS の表、`agent-docs/adr/2026-10-09-cos-operations-all-mutations.md`（表の行と付記）、`config/skills/cos-operator/operations.md` の行。
- 生成物: `docs/api/v1/api-v1.schema.json`・`web/api/generated/schema.json`（doc comment の変化だけ。`web/api/generated/types.ts` は差分なし）。

## 試験（S4-19 の名前との対応）
| S4-19 の名前 | 実装 |
|---|---|
| `script_domain_mode_ts_only_for_cos_digest` | `crates/task-api/tests/script_domains.rs` 同名 |
| `script_domain_mode_ts_requires_human_actor` | 同名（operator の 403、`system:script-startup` の store 拒否）。CoS の 403 は `script_domain_mode_through_cos_operations` の後半 |
| `script_domain_mode_ts_requires_compare_same_generation` | 同名 |
| `script_domain_mode_ts_rollback_by_anyone` | 同名（operator・CoS・`system:*`） |
| `script_domain_mode_canary_still_rejected` | 同名（旧 `script_domain_canary_and_ts_are_422_without_change` を置き換えた） |
| CoS registry の既存試験の追従 | `cos_ops_registry_matches_adr_domain_assignment_and_exclusions`・`cos_operator_skill_table_matches_allowed` は変えずに通る |
| （追加）store | `crates/task-core/src/store/script_domains_tests.rs` `script_domain_ts_for_cos_digest_needs_human_and_compare_same_generation`、`script_compare/tests.rs` の mode 表 |
| （追加）celerisctl | `script_domain_mode_request_carries_mode_version_and_generation` に `ts` |

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p task-api --test script_domains` → 10 passed
- `cargo nextest run -p task-api -E 'test(/cos_ops|cos_operator_skill|schema|script_domain/)'` → 54 passed
- `cargo nextest run -p task-core -E 'test(/script_domain|script_compare|script_mode|domain_names/)'` → 19 passed
- `cargo nextest run -p celerisctl -E 'test(/script|cos_mapped|skill/)'` → 5 passed
- `UPDATE_SCHEMA=1` で schema を再生成し、`pnpm run gen:types` で web の生成型を再生成した。
- `bash scripts/dev/test-affected.sh` → exit 0（7015 passed、21 skipped）。`cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。

## 未解決事項
- 起動時の復元（`crates/celeris/src/daemon/script_layer.rs`）は `compare` の行だけを `rust` へ戻す。S4-14 は `ts` の行も戻すと決めている。この unit の範囲（S4-19）に `crates/celeris` が無いので直していない。pass-wire か real-host の unit で `ts` も対象にする必要がある。今は dispatcher が `ts` を `rust` と同じに扱うので、挙動への影響は無い。

## 提案
なし。
