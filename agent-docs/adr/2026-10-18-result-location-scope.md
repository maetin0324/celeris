# result.json 回収の候補を run が書き得た場所に限る（ADR 2026-10-18-result-location-recovery の修正）

- 日付: 2026-10-18
- 状態: 実装済み
- tasks: [01M576C2STNWSMHANHY43H6NJN]

## 背景
回収（D1）は task workspace 全体を再帰で探したため、完了済み WU の正規の `wu/<key>/artifacts/result.json` まで候補に入った。
MLSys P1-3 の integration-repair-2 は max_turns で result.json を書かずに終わっただけなのに、14 件の他 WU の report を
「found」と列挙して `result.json misplaced`（retryable=false）→ `integration_repair_exhausted` で blocked になった。

## 決定
- D1: 探索から task root 直下の `wu/`（他 WU の正規の場所）を除く。残る探索範囲は artifacts の親（自分の WU dir / task root）と cwd 配下。
- D2: run 開始（2 秒 slack）より古い file は候補に数えない。
- D3: 候補が 0 件になったときは `None`（誤配置なし）を返し、通常の「result.json 不在」の経路（retryable な失敗・再試行の対象）に任せる。
  「misplaced」（retryable=false）は、この run が書いた候補が複数あって曖昧なときだけ。1 件なら従来どおり回収（recovery D1）。
- D4: recovery D2 の「古い 1 件は permanent error」は廃止（D2 により結果なし扱い）。

## 検証
`result_location` の単体試験: 他 WU の正規 result.json が並んでも誤判定しない、古い stray は結果なし、複数の新しい stray は misplaced、1 件は回収。
