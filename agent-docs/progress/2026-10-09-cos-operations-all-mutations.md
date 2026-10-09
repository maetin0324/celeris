---
title: CoS が API の変更操作を全て /cos/operations 経由で監査付きで行う（ALLOWED の拡張）
tasks: [01M4F5KS8E1MXZDNJTRAVFJESW]
status: done
updated: 2026-10-09
completed: 2026-10-09
---

# CoS の全 API 変更操作の監査付き登録

ADR: [all-mutations](../adr/2026-10-09-cos-operations-all-mutations.md)（実装済み。末尾の「付記: close-out」）、
[external-effects](../adr/2026-10-09-cos-operations-external-effects.md)（実装済み）、
[cos-chat-home D3](../adr/2026-10-05-cos-chat-home.md) の差分節は解消。
葉ごとの進捗は [2026-10-09-cos-operations-all-mutations/](2026-10-09-cos-operations-all-mutations/) の `<wu-key>.md`。

## 結果

変更 route 142 行（router と gui-api の表の和集合）は全て ALLOWED（106）か EXCLUDED（36）のどちらか一方だけに入る。
`PENDING` は撤去した。EXCLUDED は人の決定（秘密・browser credential/attestation・`/console/instruct`・`/cos/*` の再帰）と
ADR-0079 の撤去済み入口だけ。

| WU | 結果 | commit・文書 |
|---|---|---|
| adr・registry | 範囲の ADR と領域別 registry の基礎 | c33137ce・42170005、[adr.md](2026-10-09-cos-operations-all-mutations/adr.md)・[registry.md](2026-10-09-cos-operations-all-mutations/registry.md) |
| main | D6 の必須例（decision revise/withdraw・task edit/reopen/retry/pause/resume・execution-plan PUT・LLM 割り当て・knowledge accept） | ADR の「付記: 実装」 |
| verify-head | clippy 修正後の HEAD の検査 | [verify-head.md](2026-10-09-cos-operations-all-mutations/verify-head.md) |
| ops-tasks-decisions | task gate・rereview・decompose・通知既読・execution-plan POST・tree/adopt（B）、外部効果の helper | [ops-tasks-decisions.md](2026-10-09-cos-operations-all-mutations/ops-tasks-decisions.md) |
| ops-projects-cron | 案件 14・cron 6・integrate/PR merge・KB page（C） | 2a1dbfd8〜b256143d、[ops-projects-cron.md](2026-10-09-cos-operations-all-mutations/ops-projects-cron.md) |
| ops-admin-config | admin の残り全部（promote・reload・notify/test・replay・accounts・org・providers 等） | 23636397〜09951ec4、[ops-admin-config.md](2026-10-09-cos-operations-all-mutations/ops-admin-config.md) |
| ops-surface | chat・console・org messages・artifacts promote・browser の 21 行 | 30100aa6〜4e7eef54、[ops-surface.md](2026-10-09-cos-operations-all-mutations/ops-surface.md) |
| ops-closeout | PENDING 撤去・試験の二択化・skill・ctl の表駆動試験・gui-api・ADR 3 本・全体検査 | この節 |

## ops-closeout（2026-10-09）

1. `Registry.pending`・各領域の `PENDING` 定数・`match_operation` の pending 分岐を撤去。`Registry.name` は `cfg(test)`。
   試験 `cos_ops_registry_classifies_every_mutation_as_allowed_or_excluded`（旧 `..._covers_every_mutation_exactly_once`）と
   `cos_ops_every_allowed_row_is_reachable_through_match_operation`（新規）。
2. skill: cos-operator SKILL.md・operations.md の「登録されていない」「PATCH は操作表に無い」を消し、除外の表（系列・path・理由）を置いた。
   cos-inbox-triage に standing_rule.create・release.promote を一次対応で使わない旨。SOURCE.md に記録。
3. celerisctl: 表駆動の試験 `cos_mapped_table_wraps_every_allowed_subcommand`（approve を含む 21 行。包んだ request が
   operations.md = ALLOWED の行に当たることも確かめる）。到達しない `gate.rs` の CoS 分岐と `send_cos_approve` を削除。
4. gui-api §3.129（gui 写しも `scripts/sync-gui-docs.sh` で同期）・ADR 3 本を更新。
5. この文書。

## 証拠（HEAD c089b601 の上、文書 commit 前）

- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0
- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` → exit 0（passed 4947、failed 0、ignored 14、doctest exit 0、tmp_leftovers 0）
- `cargo nextest run -p task-api --lib cos` → 5 passed（registry 試験・skill 表の一致）
- `cargo nextest run -p celerisctl cos_mapped` → 2 passed
- `sh scripts/dev/check-doc-links.sh`・`check-adr-numbers.sh`・`check-doc-layout.sh scripts/dev/docs-layout.tsv` → exit 0
- `git diff --check $(git merge-base HEAD main) HEAD` → 出力なし

## 未解決

- 本番反映（release → verify → promote、KB への skill 取り込み `celerisctl skills import config/skills --name cos-operator --name cos-inbox-triage --root <kb_root>`）は人が行う。
- provider・account の file 効果は SQLite commit 失敗時に file だけ残り得る。browser の DB write は C の 2 段の間に入る。
- run の既定 TMPDIR（93 文字）では test-parallel.sh の Unix socket 試験が SUN_LEN に当たるため、全体検査は `TMPDIR=/tmp` で流した。
- `python3 scripts/dev/check-architecture-map.py` は変更前から NG（architecture-map の 3 path。この task と無関係）。

## 提案

- browser store に呼び出し側 transaction を受ける `_tx` 版を作り、browser の設定系を A（1 transaction）にする。
- 外部効果の `needs_remediation` を人が確かめる画面に daemon channel の操作種別を出す。
- test-parallel.sh の内側 TMPDIR を短くし、Unix socket 試験が既定の run TMPDIR でも通るようにする。
