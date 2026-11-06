# ADR 2026-11-06: N4 受信箱の読み取りを取り込み pass 内で共有する

- 状態: 採用。
- 前提: [P4-C](2026-11-05-phase4-p4c-scoped-inbox.md)。

## 決定
- event 取り込みの最大 8 page は Task の一覧を 1 回だけ読む。page ごとの触れた Task・operation attribution・cursor の transaction は維持する。
- 取り込み開始時に event head を固定してから Task 一覧を読む。head より後の event は次の pass に残し、並行して追加された root の event を古い一覧で消費しない。
- Task の一覧は呼び出し内の値として渡し、tick を越えて保持しない。digest・fallback には持ち越さず、変更後は読み直す。
- 受信箱 1 回の組み立てでは blocked Task の event 行を 1 Task ずつ読み、質問と attention の両方を組み立ててから捨てる。全 Task の event 履歴をメモリに保持せず、読み直しも避ける。
- snapshot は SQLite の正本の代わりではない。lease・dispatch・状態遷移は従来どおり DB を読み直す。TS の契約と migration は変えない。

## 検証
同じ release profile・fixture・計測コードで before/after の 0・600・2,000・10,000 件を off/shadow で測る。
root の run 数・blocked 件数・残 lease に加えて一時 DB の quick_check と foreign_key_check を確認する。
目標に届かない場合も実測値を残す。

## 補足
計測試験の settle 上限は before/after とも 1,000 tick に揃えた。退避は 1 tick に 20 件とは限らず、
既存の 400 tick が不足することは実測では確認していない。実際の回数は raw の `settle_ticks` に残す。
製品の件数上限と cursor の進め方は変えない。

## 質問の退避を主キーで読む
退避候補が `question` の場合は、その Task と root だけを主キーで取得し、既存の質問判定と Builder で項目を作る。
全件の受信箱から同じ質問を選んだものとの同値を試験する。認可へ移った質問・閉じた質問は返さない。
他の種類は従来の範囲つきの組み立てを使い、質問のための `store.list(None)` を取り除く。
