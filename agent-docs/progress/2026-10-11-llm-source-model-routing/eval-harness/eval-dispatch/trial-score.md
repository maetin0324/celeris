---
tasks: [01M4Q7C4X2QR3HYPTTQAG5GXQ9]
---

# trial-score

- `eval_trials` の完了側を実装: worker 完了後に scoring へ移し、timeout 付き hidden check を worktree で実行、`route_outcomes` に pass・exit・timeout・wall・usage・usd_list を記録して終端化し、worktree を削除する。
- daemon 再起動後の `scoring` は採点を再開し、run が消えた `running` は理由付き `error` にして worktree を削除する。
- 単体試験で合格・不合格 exit・短い timeout（sleep stub）・回収判断を確認する。
