---
wu: core-callsite-fix
completed: 2026-10-19
---
# core-callsite-fix

E0063（CampaignIdentity に variant / variant_org が無い literal）を 4 件修正。decision_log_wire.rs・tests/trial_context.rs・tests/lane_assist.rs（2 件）に `variant: None, variant_org: None` を補い、既存の意味（variant 無しの試行）を維持。core 型・migration 0088・祖先 resolve は未変更。

証拠: `cargo test -p task-dispatch --no-run` 成功、対象試験・clippy・fmt は下記コマンドで確認。
