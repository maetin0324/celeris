---
tasks: [01M4G2HVJRSRJVWEM9G32TP99R]
---

# ledger-fix: integrate-ledger verification

## 統合失敗の調査

`integration-checks/integrate-ledger/*.log` は作業ツリーにも run の artifacts にも見つからず、元の integrate-ledger 失敗検査を特定できなかった。

現在の `scripts/dev/test-parallel.sh` の EXIT trap は、logdir を `chmod -R u+w` してから削除し、削除失敗を警告に留める実装である。既知の 0o500 cleanup 問題は現 HEAD では再現せず、修正不要だった。

統合後 HEAD で launcher credential 試験を再実行したところ、run の `TMPDIR` が長いため AF_UNIX の `SUN_LEN` 制限で 8 件が bind 時に失敗した。短い `TMPDIR=/tmp` で同じ試験群を再実行すると全件通過した。これは試験環境の socket path 長制限であり、コードの失敗ではない。長い TMPDIR における全体 test-parallel 実行の再現・修正は本 WorkUnit の範囲外として残る。

## 変更

コード変更なし。ログの欠落と再現結果を記録した。

## 検証

- `cargo clippy --workspace -- -D warnings`: 成功。
- `cargo check --workspace --tests --keep-going`: 成功。
- `TMPDIR=/tmp cargo test -p task-worker launcher_credential_ --lib`: 24 passed。
- `TMPDIR=/tmp cargo test -p task-worker browser_ledger_launcher_ --lib`: 3 passed。
- `TMPDIR=/tmp cargo test -p celeris-credentiald launcher_credential_`: prod_admission 5 passed（他 filter 対象なし）。
- `sh "$CELERIS_WU_SCOPE_PATHS"`: 成功、範囲外変更なし。

## 未解決

元の integrate-ledger のログが利用できないため、元検査の失敗原因を断定できない。長い run TMPDIR で launcher socket 試験が bind できないことは確認したが、これが統合時の失敗だったというログ上の証拠はない。
