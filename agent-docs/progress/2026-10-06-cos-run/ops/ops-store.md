# CoS operations store

0050 の `cos_operations` を使い、CoS operation の適用、冪等再送、監査 event、chat card を単一の IMMEDIATE transaction にまとめた。適用失敗時はその transaction を破棄し、別 transaction に rejected 行と理由付き監査 event を記録する。監査 context は actor=cos、thread_id、run_id、operation_id、reason、policy_version を含む。task 対象の event はその task の stream に、その他は operation ID の audit stream に追記する。

`cos_operation_apply` は domain 側が transaction を受ける closure を渡す入口。API 側は run credential から AuditContext を作り、登録済み経路の操作だけを closure に接続する。event の型追加に伴う公開 schema の再生成は後段の schema WorkUnit が担当する。

検証: `cargo test -p task-core cos_chat_ops_store_ --lib`、`cargo clippy -p task-core --all-targets -- -D warnings`。
