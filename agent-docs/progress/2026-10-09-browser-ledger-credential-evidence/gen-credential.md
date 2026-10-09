---
title: browser 適合台帳 credential evidence 生成器
tasks: [01M4F73EKB2FFGAKRY2041RZ49]
status: implementation
updated: 2026-10-09
---
# browser 適合台帳 credential evidence 生成器

## 実装

`scripts/browser-conformance.py` に ADR D1/D2 の P4-A credential evidence mode を追加した。指定 ledger の schema/source を検査し、task-worker の sandboxd/egress binary を先に build する。その後、ADR の isolation 19 件・egress 21 件を個別の cargo test 起動で実行し、`running 1 test`、対象試験の `ok`、成功 summary、skip 印なしを全て満たした場合だけ `passed` とする。各試験ログ、`credential-evidence.json`、原子的な `conformance.json` を output dir に作る。指定 backend のみ更新し、P4-B evidence と generated_for は維持する。未完了時は case を除いたまま exit 1。

子 cargo の環境では `CELERIS_USERNS_TESTS=1` と `CELERIS_LAUNCHER_TESTS=require` を設定し、`CELERIS_ISOLATION_TESTS` を除去する。長い TMPDIR は短い一時 directory へ置換する。cargo は `CELERIS_CONFORMANCE_CARGO` で差し替え可能。

`scripts/tests/test_browser_conformance_credential.py` に偽 cargo による全合格、1 件 failed/skip/missing、build failure、CLI validation、他 backend/provenance 保持、試験件数の試験を追加した。Rust 側の P4-A 定数は並行 `certify-credential` unit が追加するため、統合前は件数を確認し、Rust 定数が存在する段階では両方の宣言名を要求する。

## 検証

- 前回失敗は並行 `certify-credential` 未統合で `browser_backend.rs` に P4A 定数が無かったこと。今回、その段階でも生成器 suite を実行できるようにした。
- 指定 check を `TMPDIR=/tmp` で実行: 9 件成功。TMPDIR を短くした理由は、先行実行で既存 release test の Unix socket path が AF_UNIX 上限を超えたため。
- 実 cargo・userns・外部ネットワークは使用していない。

## 残作業

- Rust 側定数の統合後に、生成器と Rust の試験名リストの完全な一致を確認する。
- 範囲 check・commit 前レビュー。
