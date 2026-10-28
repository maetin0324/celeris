# ADR: sweep の未統合判定を配送の状態に束縛する

- 日付: 2026-10-28
- 状態: 実装済み
- tasks: [01M60JC1N9V08GHGDSEHP27TRC]
- 関連: [取り残し検出](2026-10-11-stranded-done-branches.md)、[single-flight](2026-10-12-stranded-sweep-single-flight.md)

## 問題

配送が main を進める直前に sweep が祖先でないと判定すると、その後に古い unintegrated を保存できる。
配送中の branch も取り残しとして知らせてしまう。

## 決定

sweep は Git の祖先判定前に delivery の行を読む。保存は専用の compare-and-swap とし、
同一の SQLite IMMEDIATE transaction 内で delivery の行全体と既存 integration 行が読んだ時点から
変わっていないことを確かめ、状態と event を保存する。変更されていたら今回は保存せず次の sweep で判定する。
Git を実行中に DB のロックを保持しない。

対象 repo の delivery が Reviewing / MergeQueued / Merging / Preparing のときは unintegrated を保存しない。
祖先になった場合の integrated は保存できるので、配送の prepare 中でも既存の知らせを解消できる。
Ready / Blocked および delivery の無い task は従来どおり Git で判定する。
Held / Discarded の人の処分は維持し、判定中の処分や他の統合結果も上書きしない。

祖先判定は現在の default branch ref に対して行い、未統合の結果はキャッシュしない。
既存の unintegrated は次の sweep で Integrated と IntegrationMerged に更新し、受信箱の既存同期で解消する。
状態の種類・event・DB schema は追加しない。古い無条件 upsert の sweep 経路は置き換える。

## 検証

試験専用フックで delivery の読み取り後と祖先判定後に処理を挟む。
配送開始、Merging 中、Git merge 後、配送完了後、古い検出の解消を順序固定で検証する。
待ち時間・負荷による再現は使わない。
