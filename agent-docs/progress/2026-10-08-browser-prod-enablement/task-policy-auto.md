---
task: browser-prod-enablement
wu: task-policy-auto
status: done
completed: 2026-10-08
tasks: [01M4CDNAYX6J68WTX7SKF0DJ64]
---

# task-policy-auto: 起票・retry 時の最小 browser policy の自動付与と引き継ぎ（ADR 2026-10-08 D4）

## したこと
- `crates/task-core/src/store/browser_policy_auto.rs`（新規）:
  - `minimal_task_policy`（純関数）: `policy_id = "auto"`・`revision = 1`・`common_hosts`・
    `network_domains = requirements.browser.allowed_domains`・`allowed_actions = PHASE1`・`approval_actions = []`・
    `artifact_policy_id` なし。`browser_site_policies` のうち `exact_origin` が `network_domains` のどれかに覆われ
    （`origin_covers`）、かつ組織ノードのどれかの `profile.browser.credential_policy_ids` にある id を
    `credential_policy_ids` に入れ、1 つでもあれば `credential_use` を足す。`allowed_domains` が空なら付けない。
  - `attach_auto_browser_policy_tx`: `insert_tx` の最後で呼ぶ。task の作成はすべて `insert_tx` を通る
    （`POST /tasks`・CoS operations・delegate・followups・計画の子・retry）ので付与は 1 か所・同じ transaction。
    `ON CONFLICT DO NOTHING` で保存済みの policy（人の PUT）を上書きしない。
  - `inherit_browser_policy_tx`: `retry_task_impl` で元の task の保存 policy を新しい task に写す（`revision` だけ 1）。
    元に無ければ自動付与がそのまま残る。
- `browser_task_policy_set`（`PUT /tasks/{id}/browser/policy`）は、最後の遷移理由が `browser_prerequisite`
  （D2 の前提 gate）の `blocked` でも受ける。人待ちの `blocked` は従来どおり 409。
- 承認方針は不変（`approval_actions` は空。`click`/`download`/`credential_use` は `ALWAYS_APPROVED` で毎回承認、standing approval なし）。
  grant は広げない（実効 policy は run 時の `EffectiveBrowserPolicy::derive`）。

## 証拠
- `cargo test -p task-ops --test browser_policy_autoattach` → 5 passed
  （`_on_create`・`_without_grant_has_no_credential`・`_retry_inherits_original_policy`・
  `_does_not_overwrite_human_put`・`_put_accepted_while_prerequisite_blocked`）。task-core の単体 `browser_policy_autoattach_minimal_policy_rules` も pass。
- `bash scripts/dev/test-parallel.sh` → exit 0、4738 passed / 0 failed / 13 skipped（`tmp_leftovers: 0`）
- `cargo clippy --workspace -- -D warnings` → exit 0
- 【v2】統合後 HEAD で `UPDATE_SCHEMA=1 cargo test -p task-api schema`（3 passed）・`-p task-worker protocol`（1+15 passed）
  → `git status` 差分ゼロ。`docs/api/v1/api-v1.schema.json` と `web/api/generated/schema.json` は一致（再生成物の commit 不要）。

## ADR との差（付記の提案）
- ADR D4 の `Event::BrowserTaskPolicySet { policy_id, revision, source }` は**入れていない**。Event を足すと
  schema・web の `event-kinds.ts`／`invalidation-map.ts`・EVENT_TYPES の数の固定試験に及び、並行する ledger-release と
  衝突しやすいため。web-task-policy 葉（web の範囲を持つ）か close-out で足すことを提案する。
- 「`exact_origin` が origin として一致」は `origin_covers`（wildcard の requirements も含む）で判定した。grant との照合は run 時に残る。

## 未解決
- 上の Event（出自の記録）。
- web の task 詳細での確認・編集は web-task-policy 葉。
