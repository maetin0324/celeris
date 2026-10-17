# WU の result.json 誤配置を再試行不可の失敗にする

---
tasks: [01M54Y3MRRA1PAHWC128PQCTP1]
---

## 原因

WU は `wu/<key>/repos/<repo>` を cwd、`wu/<key>/artifacts` を成果物ディレクトリとする。RunRequest は worktree の場合に絶対パスを渡すが、共通前置きには相対 `artifacts/` の案内が残り、cwd と成果物 root の混同を誘う。Claude/Codex は cwd 直下の `artifacts/result.json` だけを移動して採用していた。`../artifacts/result.json` は拾われず、Claude の結果不在が AdapterError::Other から InfraRequeue に分類される。

## 決定

- WU の成果物案内は共有 cwd の場合も絶対パスにする。worktree の冒頭案内に cwd、成果物ディレクトリ、result.json の絶対パスを明記し、親ディレクトリを推測する相対パスを禁止する。共通前置きの固定文も相対パスへの書込みを指示しない。
- 誤配置の移動・コピー・削除・自動採用は撤去する。期待ファイルがない場合のみ、成果物ディレクトリの親（WU では当該 WU root）と作業ツリー内の result.json を調べ、見つかったパスと期待パスを含む Terminal::Error { retryable: false } にする。
- runs（adapter 自身の終端記録）、.git、target、node_modules は探索しない。symlink は辿らない。JSON 本文は読まず、既存ファイルの所有 run を推定して採用しない。正しい場所に結果がある場合は従来の検証を優先する。
- Claude/Codex の結果復元より先に診断し、dispatcher の共通実行経路にも同じ診断を置く。他 adapter や mock adapter の返す結果不在も infra_requeue にしない。既存の WorkerFinished / WorkUnitTransitioned に失敗理由を保存し、同じ run の自動再試行と自動 replan を止める（計画を作り直しても報告先の誤りは直らない）。
- 結果ファイルが任意の対話（CoS chat / Execute conversation）は診断対象外。home を cwd とする対話で不要な探索をしない。誤配置がない単なる結果不在、供給側障害の分類は変更しない。

## 検証

外部 LLM を呼ばない stub adapter と dispatcher の mock adapter で、cwd 直下・親相対・正規結果優先・symlink 除外、および WU の再試行抑止と event の診断を確認する。
