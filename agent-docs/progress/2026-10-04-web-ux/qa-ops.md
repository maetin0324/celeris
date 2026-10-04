---
title: ops 画面群の visual QA と修正
tasks: [01M44C029SCGEZHEK57WEK3QNB]
status: done
updated: 2026-10-04
---

# ops 画面群の visual QA と修正

task 一覧・graph・作成・task 詳細・changes・files・run ログ・Console・成果物を、360 / 390 / 412 / 1440px で pre → 修正 → post の順に確認した。pre と post はそれぞれ 132 枚で、ファイル名の集合も一致する。状態別は long-text / long-id / empty / many / loading / error / stale / forbidden の代表画面を含む。記録は [pre critique](qa-ops/critique.md) と [task 一覧・graph](qa-ops/fix-list-graph-create.md)、[task 詳細](qa-ops/fix-task-detail.md)、[changes・files・成果物](qa-ops/fix-changes-files-artifacts.md)、[run ログ・Console](qa-ops/fix-runs-console.md) を参照。

## critique

運用者は task の状態と担当を見て、依存関係・run・変更の危険度を確かめてから次の操作を選ぶ。pre は task 木、run、review、変更ファイルを画面ごとに分けており、汎用ダッシュボードのカード壁にはなっていなかった。一方、一覧の英語 status、graph の `root` / `depth`、狭い run リンクが判断とスマホ操作を妨げた。作成の条件種類、changes の取り込み結果にも内部語が残り、成果物の初期画面は案件の選択が必要だった。これを [critique](qa-ops/critique.md) の #1〜#6 とした。

## 修正

| pre の指摘 | post の対応と再確認 |
|---|---|
| #1 一覧の status 絞り込みが英語 | 日本語の状態名に変更。`many-/tasks` の 390px pre/post で確認。絞り込み値と accessible name は維持し、parity e2e が通過。 |
| #2 graph の `root` / `depth` | 「起点の task id」「深さ」を主ラベルにし、API 名を補助表示へ。`empty-/graph` の 390px pre/post で確認。入力の accessible name と URL は維持。 |
| #3 task 詳細の短い run リンクが幅 44px 未満 | task 木を含む ShortId リンク 5 箇所に `min-w-11` を付与。対象画面の mobile-audit が 4 幅で通過。 |
| #4 作成フォームの条件種類が API 型名 | **未解消**。日本語の説明はあるが、select の選択肢は `command` 等のまま。`/tasks/new` の pre/post も同じ。 |
| #5 changes の取り込み結果が英語のみ | 日本語の結果ラベルを主にし、元の state を補助情報に保持。差分の規模を repo の先頭に追加。`/tasks/T1/changes` の 390px pre/post で確認。 |
| #6 成果物は案件選択まで空 | 最近更新された案件を初期選択し、その成果物を表示。`/artifacts` の 390px post で案件選択と案内を確認。撮影時は本文が loading 中のため、表示と絞り込みは parity e2e でも確認。 |

run ログ・Console は pre の #1〜#6 に新規指摘がなかったが、[修正記録](qa-ops/fix-runs-console.md) の R1〜R8 として取得失敗の再試行、終了状態と追跡の同期、loading 表示、長いログの「最新へ」、進捗の再試行を直した。stale では取得済みの内容を保持し、接続状態を shell で伝える。files は長い path と空・失敗の既存導線を維持した。

全画面監査の再実行に必要なため、mobile-audit の名前判定も修正した。label と関連付いた textarea / select を無名と判定しないための修正で、画面の表示や操作は変えていない。

## 検証

- `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile`、`build`、`typecheck`、`lint`、`test`、`check:boundaries`、`check:parity`、`check:secrets` は exit 0。lint は既存の警告 5 件。
- 指定の `e2e/a11y`、`e2e/states`、task / task detail / runs-files の parity e2e は 55 件通過。期待された失敗の表示 1 件を含み、h1・accessible name・URL の既存契約が保たれている。
- functional e2e 全体は 166 件通過、8 件 skip。初回は release 試験が worktree 内の pnpm store 参照先不足で失敗したため、既存の `/local/.pnpm-store` への一時リンクを置いて再実行した。配布物の offline install を含め exit 0。検査後にリンクを除去した。
- ops 対象 9 route の mobile-audit は 360 / 390 / 412 / 1440px で通過。全画面の初回監査は `/projects/P1` の textarea 2 箇所を「無名」と誤判定した。Playwright の `getByRole("textbox", { name })` では「依頼文」「仕事の目的」「段階」を取得でき、画面側の label は有効。監査スクリプトが `input.labels` だけを読んでいたため、textarea と select の関連付けられた label も読むよう修正した。全 31 経路 × 4 幅の再監査は exit 0。
- crates/、`web/api/generated/`、`web/server/`、DB schema、parity e2e に task 起点からの変更はない。

## 残課題

- #4 の作成フォームの条件種類は、見える選択肢に日本語を併記できる。操作を妨げる重大問題ではなく、次の画面改善で扱う。
- screenshot script は API 応答や 1 秒後の loading / error 表示を待たない。post でも一部は空の skeleton を撮っている。代表画面以外の `/tasks/new`・changes・files・artifacts は基本状態の 4 幅だけで、8 状態の全組合せは撮れていない。状態の挙動は functional e2e とコードで補った。
- 成果物の初期表示は全案件横断の最新順ではなく、最も最近更新された案件の中の成果物。changes の範囲外ファイルは API の判定値が無いため、表示されたファイルと対象ブランチを人が確認する。いずれも API 変更を伴う別件。
