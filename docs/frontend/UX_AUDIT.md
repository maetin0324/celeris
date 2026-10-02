# web/ 全画面 UX 監査
---
tasks: [01M3XTCNKMQBCHKSZ7Y1GF6ZM4]
---

## 画面ごとの監査

現行 web/ の画面台帳（`web/e2e/support/screens.ts`）を基準にする。入口は主に shell のナビゲーションと、一覧から詳細へのリンク。主要な次操作は表示名から整理したもので、fixture の現在の状態・操作結果を個別に検証する監査は後続作業で補う。

| fixture | primary user task / 次の操作 | 優先情報 | friction の確認点 |
|---|---|---|---|
| `/` | CoS に依頼を送る／会話を続ける | 会話と入力欄 | composer とキーボードの干渉、IME |
| `/inbox` | 要対応項目を処理する／対象を開く | 承認・質問・注意の区分 | 複数項目の操作結果と長い内容 |
| `/login` | サインインして戻り先へ進む | 認証欄と失敗理由 | 狭い高さでの入力・送信 |
| `/org` | 組織を探して選ぶ／構成を変更する | 組織階層と選択詳細 | 階層と詳細の移動・編集操作 |
| `/org/cos` | 担当者の Console を使う | 会話と宛先 | `/` と共通の composer 操作 |
| `/projects` | 案件を探す・作る／詳細へ進む | 案件名と状態 | 一覧と作成操作の優先度 |
| `/projects/P1` | 案件の作業を把握する／task・repo 等へ進む | 状態、計画、仕事の木 | 情報量と長い構造の走査 |
| `/projects/P1/docs` | 案件文書を読む・編集する | 文書本文と保存状態 | 閲覧と編集の切替 |
| `/projects/P1/docs/maintenance` | 文書保守を起動する／結果を確認する | 起動条件と処理結果 | 実行中・失敗後の次の手掛かり |
| `/board` | 案件の進捗を列で確認する／task を開く | 列と task 状態 | 6列の小画面での移動 |
| `/knowledge` | 知識を検索して読む／保存する | 検索結果と本文 | 検索条件・本文の切替 |
| `/knowledge/inbox` | 知識候補を採用・却下する | 候補本文と判断 | 判断の文脈と破壊的操作 |
| `/knowledge/skills` | skill を探す・作成・編集する | skill 名と本文 | 一覧・詳細・編集の状態識別 |
| `/reports` | 報告を確認して既読にする | 未読・重要度・発生元 | 展開行の長さと通知設定 |
| `/approvals` | 保留中の承認を判断する | 対象・根拠・判断結果 | 誤操作防止と結果の見落とし |
| `/artifacts` | 成果物を絞り込み開く | 名前・task・project | 長い名前と種別の判別 |
| `/tasks` | task を検索・絞り込み選ぶ | 状態・タイトル・担当 | 表の横幅と検索の状態保持 |
| `/tasks/new` | task と受け入れ条件を作る | 必須項目と条件 | 条件の追加・削除、入力エラー |
| `/tasks/T1` | 状況を把握し判断・操作する | 状態、次の判断、活動 | 多数の操作と tab の位置関係 |
| `/tasks/T1/files` | 作業ツリーとファイルを読む | path と本文 | 長い path・行の折返し |
| `/tasks/T1/changes` | 差分を確認し統合を判断する | 변경概要と差分 | 긴 줄・横方向差分の操作 |
| `/tasks/T1/runs/R1` | run の会話ログを追う | 発話・時刻・実行状態 | 長いログと実行中の追随 |
| `/plans/new` | 計画を作成する | 計画内容と条件 | task 作成との概念差・失敗表示 |
| `/daemon` | daemon の状態を確認し replay する | 接続状態と replay 結果 | stale と停止中の識別 |
| `/providers` | provider を管理・確認する | 接続状態と設定 | 削除・check の結果と確認 |
| `/accounts` | account・認証・secret を管理する | account 状態と認証手順 | 秘密値の扱いと操作密度 |
| `/clusters` | cluster の状態と設定を確認する | 可用性・接続情報 | disconnected と stale の区別 |
| `/releases` | release の状態を確認する | version と昇格状態 | 状態遷移と影響範囲の理解 |
| `/graph` | task の依存関係をたどる | 依存と選択中 task | 小画面での graph navigation |
| `/help` | 操作方法を探す | 検索可能な説明と関連先 | 情報の見つけやすさ |

台帳は31 route だが `/org/secretary` と `/org/$id` は同じ fixture `/org/cos` を使うため、撮影対象は30 unique fixture。`/tasks/T1/changes` の差分表示には長い行、`/projects/P1/docs/maintenance` は処理結果、`/inbox` と `/approvals` は誤操作防止を重点確認する。

## component hierarchy

全画面は共通 shell（ナビゲーション、状態通知、main 領域）を土台に、route 固有の見出し・操作・データ表示を置く。詳細画面では一覧からの入口、現在位置、次に可能な操作が追える必要がある。画面固有の component 境界と再利用実態は後続監査でコードに照合する。

## responsive の振る舞い

基準幅は 360/390/412/1440 px。狭い幅では一覧の情報優先度を保ち、表・ログ・差分・依存図の横スクロールを局所化し、操作対象は十分なタップ領域を持たせる。画面全体の横溢れ、固定入力欄とソフトキーボードの重なり、長文による操作の押し下げを before 画像で確認する。

## 状態の現状

この撮影は fixture の初期表示を記録するもので、状態遷移の試験ではない。loading、empty、error、stale、disconnected、permission-denied と破壊的操作の確認状況は、before 画像だけでは判定できず未監査として扱う。各画面でデータが無い状態と取得失敗を区別し、変更操作は対象・結果・取り消し可否を確認できることが必要。

## cosmetic と IA/component 層の切り分け

余白、文字階層、色、境界、折返しなど情報構造を変えない問題は cosmetic として調整できる。主要操作の発見性、情報の優先順、一覧と詳細の移動、複数操作の混在、状態の誤認は IA または component 境界から見直す。管理画面を共通カードの寄せ集めにせず、設定対象と危険度に沿ってまとまりを作る。

## 4 群の割り当て

| 群 | fixture |
|---|---|
| foundation | `/`, `/login`, `/help` |
| task・run 系 | `/tasks`, `/tasks/new`, `/tasks/T1`, `/tasks/T1/files`, `/tasks/T1/changes`, `/tasks/T1/runs/R1`, `/plans/new`, `/graph` |
| inbox・project 系 | `/inbox`, `/projects`, `/projects/P1`, `/projects/P1/docs`, `/projects/P1/docs/maintenance`, `/board`, `/knowledge`, `/knowledge/inbox`, `/knowledge/skills`, `/reports`, `/approvals`, `/artifacts` |
| 管理系 | `/org`, `/org/cos`, `/daemon`, `/providers`, `/accounts`, `/clusters`, `/releases` |

## quality gate critique

品質ゲートでは、Celeris 固有の高密度な ops workbench として、現在の作業・判断・復旧に必要な情報が先に読めるかを確かめる。generic AI dashboard の統計ヒーロー、同じ形の card wall、用途のない過剰余白、unstyled admin の単調なフォーム列になっていないかを全画面で見る。状態を色だけに頼らず示し、キーボード focus、狭幅での操作、エラーからの復帰、破壊的操作の確認を画面ごとに検証する。撮影画像は初期状態の視覚資料であり、これらの操作性・アクセシビリティ gate の合格証明ではない。

## before screenshot

- 保存先: `artifacts/before`（WU artifacts 絶対 path: `/var/lib/celeris/workspaces/01M3YEN6B6AWRGRVGTPKGQ32YP/wu/before-shots/artifacts/before`）。
- 撮影コマンド: `corepack pnpm@12.6.0 -C web screenshots -- --out /var/lib/celeris/workspaces/01M3YEN6B6AWRGRVGTPKGQ32YP/wu/before-shots/artifacts/before`
- PNG: **120 枚**（31 route / 30 unique fixture × 360/390/412/1440）。幅は各 viewport の CSS px。
- 命名規則: fixture の英数字以外を `_` に置換し、末尾に `-<width>.png` を付ける。root `/` は `_` なので `_-360.png` 等。同一 fixture の route は同じファイル名になる。
