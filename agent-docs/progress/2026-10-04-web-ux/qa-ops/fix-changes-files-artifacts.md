---
title: ops 画面群 visual QA — changes・files・成果物の修正
tasks: [01M44C029SCGEZHEK57WEK3QNB]
status: fixed
updated: 2026-10-04
---

# changes・files・成果物の修正

## critique と修正

| critique.md の指摘 | 対応 |
|---|---|
| changes の取り込み結果が英語の状態値だけ | `merged`、`conflict` などに日本語ラベルを付け、元の method / state は補助情報に残した。操作直後の結果にも同じラベルを使う。 |
| changes の危険度が一覧の下まで読まないと分からない | repo の先頭に変更ファイル数と追加・削除行数を表示した。取り込み欄でファイルと対象ブランチの確認を促す。 |
| files は新規指摘なし | 現行の長い path・空・取得失敗時の導線を維持した。画面固有のコード変更は不要。 |
| artifacts は案件を選ぶまで成果物を見られない | 案件一覧の `updated_at` が最も新しい案件を初期表示し、その案件内では成果物の記録時刻順に並べる。案件の明示選択と URL の `?project=` は従来どおり使える。 |

changes の入力欄に明示的な accessible name を付け、mobile-audit の未命名 control を解消した。成果物一覧の領域名は「成果物一覧: ID」とし、案件 select のラベルとの曖昧一致を避けた。

## 表示確認

build 後、`qa-ops-pre` と `qa-ops-post` に `/tasks/T1/changes`、`/tasks/T1/files`、`/artifacts` の 360 / 390 / 412 / 1440px を各 4 枚保存。360px の pre/post を比較し、changes の差分規模が先頭に現れ、成果物は初期表示で一覧に進むことを確認した。files は同じ構造で横溢れがない。
撮影 script は API 応答を待たないため、post の成果物 412 / 1440px は一覧の読み込み中に撮れている。成果物の表示内容は 360 / 390px と parity e2e で確認し、4 幅の target と横溢れは mobile-audit で確認した。

## 検証

- `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile`、`build`、`typecheck`、`lint`、`test`、`check:boundaries`、`check:parity`: 通過。lint の既存警告 5 件は今回の差分外。
- `corepack pnpm@12.6.0 -C web e2e e2e/parity/runs-files.spec.ts e2e/parity/task-detail.spec.ts`: 11 件通過。
- `corepack pnpm@12.6.0 -C web mobile-audit --only /tasks/T1/changes`、`--only /tasks/T1/files`、`--only /artifacts`: 各 4 幅通過。
- 全画面 mobile-audit は `/projects/P1` の未命名 textarea と `/tasks/T1` の run リンクなど、この WU 以外の指摘で失敗。changes の未命名 textarea は修正済み。

## 残課題

- API に範囲外ファイルの判定値がないため、changes では範囲外かどうかを自動判定できない。表示したファイル一覧と対象ブランチで確認する必要がある。判定値を追加するには API 型・gateway 等の変更が必要で、この WU の許可範囲外。
- 全案件を横断した「最新の成果物」取得経路がない。初期表示は最近**更新された案件**の成果物であり、全案件横断の新着順ではない。
- critique.md が挙げた changes / files / artifacts の状態別 screenshot 欠落と screenshot timing は撮影 tooling の範囲であり、この WU では変更していない。
