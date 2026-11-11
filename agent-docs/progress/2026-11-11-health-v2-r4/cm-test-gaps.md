---
title: "健全性v2 R4: 反例試験の不足の調査"
tasks: [01M74VAMWFCC4JGZY4ENA92HJJ]
status: completed
updated: 2026-11-11
---
# 健全性v2 R4: 反例試験の不足の調査

完了日: 2026-11-11。基準は `$CELERIS_WU_BASE`。

## 変更

- `phase 1: document dispatcher counterexample test gaps` — change-map.md の既存7関数の本文を保持し、末尾に lease・restart・同時完了・review/integration・権限/予算の5節を追記した。各節に既存試験の行と関数名、assertion の範囲、不変条件の根拠、足りない入力状態と操作順、期待結果、決定的な再現案を書いた。
- コード・既存試験・計測 script・他 WU の記録は変更しない。新しい設計の採用はなく、ADR は追加しない。

## 作業前 brief

1. 根本原因: 判断と副作用の間にある順序の違いを、単独の分岐試験と区別して記録する必要がある。
2. 責務 owner: 調査記録は cm-test-gaps。調査対象の処理は run-supervisor・execution-coordination・routing。
3. 再利用先: cm-functions の7関数の区間、既存の lease・ownerless・統合・予算・credential 試験。
4. 選択肢: 試験名だけの推測ではなく、本文の入力・操作順・assertion と照合した不足を記録する。
5. 新しい状態・分岐・fallback・port: 無し。再現用 hook は将来の案に留める。
6. 撤去対象: 無し。既存7関数の節を保持する。
7. 不変条件: SQLite 正本、lease 排他、古い完了の非干渉、review 候補の束縛、予算・認証待ちの停止。今回は文書構造・参照・変更範囲を検査する。

## 証拠（提出時の HEAD で実行）

- 成果物ディレクトリの `validate-docs.py` を `python3` で実行: 5節の完全一致、各節の試験行参照・不足・再現方法、7関数を含む元本文の完全保持、参照 path と試験関数の宣言行を検査。exit 0。
- `sh scripts/dev/progress-index.sh --check`: exit 0。
- `git diff --check`: exit 0。
- `cargo clippy --workspace -- -D warnings`: exit 0。vendor の deno_core に既存の deprecated 警告1件が出るが、検査は成功。
- `cargo fmt --all -- --check`: exit 0（整形差分なし）。
- `$CELERIS_WU_SCOPE_PATHS` があれば `sh "$CELERIS_WU_SCOPE_PATHS"`、無ければ `git diff --name-only "$CELERIS_WU_BASE"` で確認: 変更は change-map.md と本記録の2件のみ。
- Objective の「試験は書かず、走らせない」に従い、test-affected・nextest・doc test・web 試験は実行していない。既存試験の合格は主張しない。

## 未解決

不足として挙げた順序の動的な再現と修正要否は未確認。特に ownerless ADR は遅延した WorkerFinished の追記を許すため、終了 event の件数と終了処理の二重適用を区別した。

## 提案

後続の設計段階では、lease snapshot 後の更新、旧 reviewer の遅延完了、WU／run／task の永続化境界、review の候補変更、credential 解消後の予算再評価を優先して検証する。再現は注入時計・出来事待ち・試験専用 hook を使い、CPU 負荷を使わない。今回の調査はその実装や試験追加を先行しない。
