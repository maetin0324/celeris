---
tasks: [01M4Q53E9WQEKMAS448GZMWTFB]
---
# Eval harness fix

`Candidate` に harness ID を持たせ、Claude OAuth の 4 候補を `claude-code`、Codex OAuth の 3 候補を `codex` に固定しました。run の trial 作成は候補値を使い、dry-run JSON と Markdown にも harness を含めます。集計の paired comparison は同じ genre と harness の trial に限定します。

利用方法は従来どおり `celeris-routing-eval dry-run --manifest <file> --db <read-only-db> [--measured <usage.json>] [--out <dir>]` です。候補別の harness は JSON の `candidates[].harness_id` と Markdown の Harness 列で確認できます。

確認: `cargo test -p routing-eval` および `cargo test -p task-dispatch eval_trials`（実行結果は run の result.json に記録）。LLM を呼ぶ評価 run は起動していません。
