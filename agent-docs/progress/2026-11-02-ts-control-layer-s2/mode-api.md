---
title: S2 mode-api — 領域の mode を読む・rust/compare に切り替える API と celerisctl
tasks: [01M6E6HEH7CPZX9KX4Y2SAZKTG]
status: blocked
updated: 2026-11-02
---
# S2 mode-api

## 結論
- 境界 ADR の付記 S2-5 のとおり、compare-store の store 関数の上に API・CoS の操作・CLI を作った。dispatcher にはまだ結線していない（dispatch-hook の unit）。
- `crates/task-api/src/script_domains.rs`:
  - `GET /api/v1/script/domains` → `ScriptDomainList`（領域ごとの `mode`・`generation_id`・`bundle_sha256`・`version`・`updated_at`・`updated_by`）。
  - `PUT /api/v1/script/domains/{domain}/mode` → `ScriptDomainModeResult`（`changed`・`event_id`・`domain`）。本文 `ScriptDomainModeBody`
    `{mode, expected_version, generation?, reason?}`（`deny_unknown_fields`。`generation` は `bundle_sha256`）。
  - 失敗の写し: 未知の領域 404 `script_domain_not_found`、`canary`・`ts` 422 `script_mode_not_enabled`、版の不一致 409
    `script_domain_version_conflict`（`expected_version`・`version` を extra に出す）、generation の不備 422 `script_generation_invalid`、
    JSON の誤り・未知欄・未知の mode 名 400。どの拒否でも表と `script_domain_events` は変わらない（store の CAS が 1 transaction）。
  - 同じ mode・同じ generation は 200・`changed: false`（store の `Unchanged`）。
- CoS operations: admin の `ALLOWED` に `("PUT", "/api/v1/script/domains/{domain}/mode", "script_domain.mode_set")` を足した。
  route と同じ `set_mode_effect` を `external_store`（書き込み 1 回の外部効果の手順）で呼ぶ。actor は `cos:<thread>`、
  本文に `reason` が無ければ操作の理由を記録する。`rust`→`compare` と `rust` への戻しは許可、`canary`・`ts` は同じ関数が 422 で拒み rejected の記録が残る（境界 ADR (d) の権限）。
- celerisctl: `celerisctl script domains [--json]`（表か JSON）と
  `celerisctl script mode <domain> rust|compare --expected-version <n> [--generation <bundle_sha256>] [--mode-reason …]`。
  変更は API 経由（DB を開かない）。CoS の run では `cos_mapped` が `/cos/operations` に包む。理由の option は全体の `--reason`（CoS の操作の理由）と衝突しないよう `--mode-reason` にした。
- 文書: `docs/api/v1/api.md` の一覧 #227・#228、CoS 操作表の行、§3.134。`docs/api/v1/api-v1.schema.json`（`UPDATE_SCHEMA=1`）と
  `web/api/generated/{schema.json,types.ts}`（`node web/scripts/gen-types.mjs`）を再生成した。
- 範囲外の 2 file（既存の試験が新しい変更 route に要求する）:
  - `agent-docs/adr/2026-10-09-cos-operations-all-mutations.md`: admin の表に行と付記（試験 `cos_ops_registry_matches_adr_domain_assignment_and_exclusions`）。
  - `config/skills/cos-operator/operations.md`: 登録表に行（試験 `cos_operator_skill_table_matches_allowed` と celerisctl の `cos_mapped_table_wraps_every_allowed_subcommand`）。

## 試験（名前に script_domain を含む）
- task-api `tests/script_domains.rs`:
  `script_domain_list_starts_rust_without_generation`・`script_domain_mode_rust_compare_rust_round_trip`・
  `script_domain_canary_and_ts_are_422_without_change`・`script_domain_stale_version_is_409_without_change`・
  `script_domain_rejects_unknown_domain_bad_generation_and_bad_body`・`script_domain_mode_through_cos_operations`。
- celerisctl `commands/script.rs`: `script_domain_mode_request_carries_mode_version_and_generation`・`script_domain_table_shows_mode_version_and_short_bundle`。
  `main.rs` の `cos_mapped_table_wraps_every_allowed_subcommand` に `script mode` の 2 行、unmapped に `script domains` を足した。

## commit（件名）
1. `phase s2: mode-api: 領域の mode を読む・切り替える API・CoS 操作・celerisctl script`
2. `phase s2: mode-api: celerisctl の cos_mapped 試験の表を source-size の上限に収める`
3. `phase s2: mode-api: 進捗の証拠`

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p task-api -E 'test(script_domain)'` — exit 0、6 passed。
- `bash scripts/dev/test-affected.sh` — exit 0。cargo 4922 passed・20 skipped、web tsc -b と unit 697 passed、source-size gate 0 failure。
  （初回は celerisctl `main.rs` の inline 試験 `cos_mapping_tests` が 320 行で source-size の上限 300 を超えたので、表の追加を 1 行に減らした。`--generation` 付きの本文は `script.rs` の試験で見る。）
- `cargo clippy --workspace -- -D warnings` — exit 0。
- `cargo fmt --all -- --check` — exit 0。
- 範囲 check — exit 1。範囲外は `agent-docs/adr/2026-10-09-cos-operations-all-mutations.md` と `config/skills/cos-operator/operations.md` の 2 file だけ（下の未解決 1）。

## 未解決
1. 範囲 check（`crates/task-api/`・`crates/celerisctl/`・`docs/`・`web/`・`agent-docs/progress/`・境界 ADR）は、上の範囲外の 2 file で落ちる。
   新しい変更 route を足すと、既存の registry 試験がこの 2 file の行を要るので避けられない。plan_issue で申告し、replan で範囲に足すことを求めた。
   attempt 1 の再 run で確かめた: 2 file を `$CELERIS_WU_BASE` の内容に戻して `cargo nextest run -p task-api -p celerisctl` で該当 3 試験を走らせると exit 100、3 件とも失敗（`cos_ops_registry_matches_adr_domain_assignment_and_exclusions`・`cos_operator_skill_table_matches_allowed`・`cos_mapped_table_wraps_every_allowed_subcommand`）。確認後に 2 file は HEAD の内容へ戻した。

## 本番への移行
- 本番 KB の cos-operator skill を差し替える（人の手順: `celerisctl skills import config/skills --name cos-operator --root <kb_root>`）。差し替えるまで CoS の skill の表にこの操作が出ないが、操作自体は registry で通る。
- config の欄・DB の migration は足していない（migration 0103 は compare-store）。

## 提案
- 新しい変更 route を足す unit の範囲 check には、最初から `agent-docs/adr/2026-10-09-cos-operations-all-mutations.md` と `config/skills/cos-operator/operations.md` を許可する。
