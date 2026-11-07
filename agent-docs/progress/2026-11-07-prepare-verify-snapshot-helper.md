---
title: release prepare の verify が snapshot_db.py 欠落で落ちた件の原因と本番手順
tasks: [01M6S99ADJGBSD0CAJQDKSSMQ5]
status: done
updated: 2026-11-07
---

# release prepare の verify が snapshot_db.py 欠落で落ちた件

結論: コード修正は不要。人が本番で下の手順のどちらかを実行し、prepare を再実行する。本 run は本番を変更していない。

## 原因

- prepare.log の失敗は `python3: can't open file /local/celeris/state/current/scripts/snapshot_db.py`。
- ログ: `/local/celeris/state/releases/.deliveries/01M6S99ADJGBSD0CAJQDKSSMQ5/ed4301e8bc6bd6f26e124dd372119ddaa6814be8/prepare.log`
- verify.sh は current release の scripts を使う。current は snapshot_db.py を同梱する前の release 8024a00d18c2 なので、これが current である限り verify は落ちる。

## コード修正が不要な理由

- ブランチ側の release.sh は helper を同梱済み。commit 件名「phase 5: pin verify database snapshot during concurrent writes」で `scripts/selfdeploy/snapshot_db.py` と、同梱を固定する試験 `scripts/selfdeploy/tests/bundle_includes_verify_helpers.sh` が入っている。
- 新しい release が current になれば解消する。この WorkUnit の差分は進捗ファイルのみで、crates と scripts に差分は無い。

## 本番への移行（人が実行する手順）

次のどちらかを人が選ぶ。

A. current release の scripts へ helper を補う
1. `cp scripts/selfdeploy/snapshot_db.py /local/celeris/state/current/scripts/snapshot_db.py`（リポジトリの main 取り込み後の checkout で実行。current が symlink なら実体の release dir の scripts へ置く）
2. `ls -l /local/celeris/state/current/scripts/snapshot_db.py` で存在を確認する。

B. snapshot_db.py を同梱した release を先に promote する
1. helper 同梱後の commit から release を作り、通常の gate を通して promote する（promote.sh は `SD_ACTOR` が要る）。
2. `ls /local/celeris/state/current/scripts/snapshot_db.py` で同梱を確認する。

実行後の確認:
1. delivery の prepare を再実行する。
2. prepare.log に `can't open file ... snapshot_db.py` が出ず、verify が通る（exit 0）ことを確認する。

config の変更は無い。
