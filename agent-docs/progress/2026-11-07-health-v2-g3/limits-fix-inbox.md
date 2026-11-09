---
title: 健全性v2 G3 limits fix-inbox — 受信箱の health_repeat の項目・API 仕様・ADR D3.4 から Blocked の源に効かない retry の案内を除く
tasks: [01M70C8NHEANARRKZD9RGKZ87D]
status: done
updated: 2026-11-09
completed: 2026-11-09
---
# 健全性v2 G3 limits fix-inbox — 受信箱の health_repeat の項目・API 仕様・ADR D3.4 から Blocked の源に効かない retry の案内を除く

WorkUnit `fix-inbox`（task 01M70C8NHEANARRKZD9RGKZ87D）。final review の気づき (a) の直し。基準は `$CELERIS_WU_BASE`。

結論: 完了。受信箱の `health_repeat` の項目の選択肢は `comment` だけになった（recommended も `comment`）。API 仕様の 2026-11-09 の改訂行と ADR D3.4 の行は、Blocked の task を再開できる口だけを書く。試験 `health_limits_inbox_options_resume_blocked_source` を足し、既存の `health_limits_inbox_` の 5 件と合わせて 6 件が合格した。config・API の欄は足していない。

## 実装前 brief
1. 根本原因: `Builder::health_repeat`（`crates/task-ops/src/human_inbox.rs`）が選択肢に `retry`（`POST /tasks/{id}/retry`）を出していた。retry は `task_ops::retry::plan_retry` が Failed・Cancelled しか受けないので、項目の源（往復で Blocked にした task）には 409 になる。同じ案内が項目の detail、doc comment、API 仕様の改訂行、ADR D3.4 の行にもあった。
2. 責務 owner: 選択肢と detail は `Builder::health_repeat`。説明は `inbox/health_repeat.rs` の module doc、`docs/api/v1/api.md`、ADR `2026-11-08-health-enforcement-loop.md` の D3.4。
3. 既存の再利用先: Blocked の task への comment は `task_ops::comment::plan_human_comment` が回答（`Trigger::Answer`。`Event::Answered` と同じ transaction）にする。受信箱の質問の回答 `POST /tasks/{id}/answer` は `task_ops::gate::answer_via` が Blocked の task に同じ `Trigger::Answer` を適用する（途中確認・計画の承認待ちは除く。health の止まりはどちらでもない）。health の Error で止めた task が comment で Ready に戻ることは既存の試験 `health_limits_review_blocked_resumes_on_comment` が確かめている。試験の土台は `inbox/health_repeat/tests.rs` の `insert`・`inbox`・`repeats`・`fps`。
4. 選択肢: (a) retry の選択肢を消す（採用）。(b) retry を Blocked も受けるように広げる案は、retry が新しい task を draft で作る別の意味の操作なので採らない。(c) 受信箱の項目に回答の選択肢を足す案は、項目が木の root の 1 件で源が複数あり得るため、受信箱では答えない（native なし）既存の決め（D4.3）と合わない。源ごとの質問の項目が既に回答を受ける。
5. 新しい状態・分岐・fallback・port: 無し。
6. 撤去対象: `health_repeat` の選択肢 `retry`、detail と doc comment と API 仕様と ADR D3.4 の retry による再開の案内。
7. 不変条件: `health_repeat` の項目の選択肢は Blocked の源に効く口（comment）だけ。試験 `health_limits_inbox_options_resume_blocked_source` が確かめる。

## 変更（commit の件名）
- `G3 limits fix-inbox: health_repeat の選択肢を comment だけにし、API 仕様と ADR D3.4 の再開の口を直す` — 次の 5 file と、この記録。
  - `crates/task-ops/src/human_inbox.rs`: `Builder::health_repeat` の選択肢から `retry` を消した。detail は「源の task に comment で直し方を伝えると再開し」にした。doc comment に、選択肢は Blocked の源に効く口だけで、retry は Failed・Cancelled しか受けないことを書いた。
  - `crates/task-ops/src/inbox/health_repeat.rs`: module doc と `still_blocked` の doc の再開の口を comment（回答 `Trigger::Answer`）にした。
  - `crates/task-ops/src/inbox/health_repeat/tests.rs`: `health_limits_inbox_source_leaving_blocked_clears` の doc の再開の口を comment にした。試験 `health_limits_inbox_options_resume_blocked_source` を足した。
  - `docs/api/v1/api.md`: 2026-11-09 の改訂行を「人は源の task に comment して再開する」にした。
  - `agent-docs/adr/2026-11-08-health-enforcement-loop.md`: D3.4 の行を「task への comment か受信箱の質問への回答（`POST /tasks/{id}/answer`）で再開する（どちらも `Trigger::Answer`）。`POST /tasks/{id}/retry` は Failed・Cancelled の task しか受けない（`task_ops::retry::plan_retry`）ので、Blocked の task の再開には使えない」にした。末尾の付記は変えていない（fix-dedup の範囲）。

## 試験が守る不変条件と、既存の試験で検出できない理由
- 試験 `health_limits_inbox_options_resume_blocked_source`（`crates/task-ops/src/inbox/health_repeat/tests.rs`）: Blocked の源を 1 件持つ `health_repeat` の項目について、選択肢の key が `["comment"]` だけで、recommended が `comment`、選択肢の effect に `/retry` が無く、detail に retry が無いことを確かめる。続けて、出さない口の `plan_retry` が Blocked の源を `InvalidState` で拒むこと、出した口の comment が源を `Blocked → Ready`（理由 `Trigger::Answer`）にして項目が消えることを確かめる。
- 既存の `health_limits_inbox_` の 5 件は、項目の数・源・消え方・native の無いことだけを見て、選択肢の key も、選択肢の口が源の今の状態で受け付けられるかも見ない。このため Blocked の源に効かない選択肢が混ざっても落ちない。
- 直す前の `human_inbox.rs`（`git show HEAD:…` で一時的に戻した）でこの試験を走らせると、`left: ["comment", "retry"]`・`right: ["comment"]` で落ちることを確かめた（確認の後に直した版へ戻した）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p task-ops --no-tests=fail -E 'test(/health_limits_inbox_/)'` → exit 0（6 passed）
- `! grep -rnE 'comment ?(するか|か|・) ?retry' docs/api/v1/api.md agent-docs/adr/2026-11-08-health-enforcement-loop.md crates/task-ops/src/human_inbox.rs crates/task-ops/src/inbox/health_repeat.rs crates/task-ops/src/inbox/health_repeat && grep -qE 'health_repeat.*comment して再開' docs/api/v1/api.md` → exit 0
- 範囲の検査（`sh "$CELERIS_WU_SCOPE_PATHS"` の path が Objective の 7 path の中）→ 範囲外 0 件
- `bash scripts/dev/test-affected.sh` → exit 0。対象は celeris・celeris-campaign・celeris-mcp・celerisctl・llm-proxy・task-api・task-dispatch・task-ops・task-worker と web（docs/api/ と task-ops の変更による）。nextest 6551 passed・23 skipped、web の型検査（`tsc -b`）と unit 104 file・711 件が合格、check-health の source-size を含む全検査が pass（`repo_health_gate: all checks passed`）。web/node_modules は先に `pnpm install --offline --frozen-lockfile` で入れた
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- 上の 3 つはコードの commit の後に実行した。その後の commit はこの記録の証拠の追記だけ

## 未解決
- `agent-docs/progress/2026-11-07-health-v2-g3/limits-escalation.md` の変更の記録は、当時の選択肢（comment と retry）をそのまま書いている。この unit の範囲の外で、過去の作業の記録なので変えていない。今の動きはこの記録と ADR D3.4 が正しい。

## 提案
- 無し。
