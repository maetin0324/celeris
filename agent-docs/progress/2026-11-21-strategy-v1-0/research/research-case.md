# 研究ケース: BenchFS ISC27 task C の Sirius の node 停止とアプリ同居（2026-11-16）

## 選んだもの

BenchFS 案件の task 01M7C56WK4E86BTZNDKAX7SP7C（ISC27 C、status Done）が残した `c-results.md` の 11.15〜11.21 節（run 26〜31、PBS job 50098〜50198）。
同じ観測は知識ベースの 3 頁にある: `projects/benchfs/sirius-benchfs-daemon-core-triad-node-k-24-2026.md`（仮説）、
`projects/benchfs/sirius-benchfs-daemon-core-triad-thread-sched-id.md`（30 cell の時点の結論「止まらなかった」）、
`projects/benchfs/sirius-daemon-core-yield-triad-thread-sched-idle.md`（3 反復の後の訂正「1/56 残った」）。

## 選んだ理由

- 仮説 → 介入 → 早すぎた結論 → 反証 → 仮説の修正、の順が時刻つきの記録で残っている。子 8 が試す「仮説の反証の後に実験設計を合理的に修正できるか」に、そのまま当てられる。
  - 11.15・11.16: 同じ優先度で daemon の core を共有すると k=24 で host が止まる（8/24 host×cell）。start-up relief では 4/8 で止まった。
  - 11.17: daemon-core yield（triad の thread を SCHED_IDLE）で 0/240。ここで知識ベースは「止まらなかった」と書いた。
  - 11.18・11.19: 反復でも 0。
  - 11.20: 50183 の BenchFS strict の k=24 rep 3 で a19 が停止。11.21 で測り直し、合計は 1/488（BenchFS 1/56）。仮説は「yield で大きく減るが 0 にはならない。相の始まりに 1〜2 % の HW 由来の停止」に変わった。
- 早すぎた結論（30 cell・0/240 で「止まらない」と一般化）は、rubric の unsupported claims を採点する実例になる。事前の情報（k=23 でも 1/192 の停止、49898）が slice 11.17 の時点で既にあり、反復を待つべきだったと判定できる。
- 測定は終わっている。新しい実験は要らない。

検討して選ばなかったもの: BenchFS strict fsync の不具合（KB の project:benchfs の fsync の頁。修正の前後の比較で、仮説の改めの過程の記録が薄い）、
Pluvio の 2 つの調査（文献・Web の調査で、反証できる実験の記録が無い）。

## データの所在（read-only）

| もの | 所在 | 固定 |
|---|---|---|
| 結果の本文 | `/local/celeris/data/workspaces/01M7C56WK4E86BTZNDKAX7SP7C/artifacts/c-results.md`（329,632 byte） | sha256 `2830ce8242fd4d3ba538422ef5dc6c847e467eaebc3d25983109149423d264ea` |
| cell ごとの記録 | 同じ dir の `c-manifest.json`（7,349,041 byte、`application.daemon_core_yield`） | sha256 `169869f424518a7f863dbb9f395248c6b1b6e355b004788a24603602dff0e83f` |
| 写し | この task の artifacts の `research-case-src/`（c-results.md と KB 3 頁） | 同じ sha256 |

データは agent-platform の repo に写さない。ISC27 の論文は未発表で、測定は機微な研究結果として扱う（[design-brief.md](../design-brief.md) の機密境界。外部の LLM に既定で送らない）。

## 利用の許可

- 測定は人の依頼（BenchFS 案件 thread の seq 431、2026-11-14）で task C が行った。この案件の持ち主と、Celeris の評価の持ち主は同じ人である。
- 内部の評価（Celeris の中の CoS・Reviewer・人の採点）で読むだけに使う。外部の LLM（ChatGPT・Muse）に本文を送ること、論文の外で公開することは、人の明示の許可が要る。
- 人がこの用途に明示の許可を出したかは**未確認**。子 8 の pilot の前に、子 8 が人の確認（check の type human か質問）を置く。

## 再現手順

```sh
python3 -I agent-docs/progress/2026-11-21-strategy-v1-0/research/research_case.py verify
# 写しを使うとき:
python3 -I agent-docs/progress/2026-11-21-strategy-v1-0/research/research_case.py verify --src <artifacts>/research-case-src/c-results.md
```

sha256 の一致、11.15〜11.21 の 7 時点の slice（各時点はその節までの本文）に、11.20 で初めて現れる語（`1 / 488`・`16 件目`・`a19 が 2026-11-16 21:28`）が無いこと、
否定対照（11.17 の slice に 16 節の総括を足す）で漏れが検出されることを確かめ、すべて期待どおりなら exit 0（提出時の HEAD で実行: exit 0、7 slice、否定対照 1 件を検出）。

## 時点ごとの採点の参照（子 0 の案。人が上書きしてよい）

| 時点 | その時点で言えること | 根拠の無い主張の例 | 合理的な次の手 |
|---|---|---|---|
| 11.16 の後 | 同じ優先度の共有で止まる（8/24）。relief は効かない（4/8） | 「HW の不具合が原因と確定」 | 優先度を下げる介入（yield）を k=24 に当て、反復で率を測る |
| 11.17 の後 | yield で 0/240。共有の 8/24 より大きく減った | 「yield なら止まらない」（k=23 で 1/192 が既にある） | 反復を続け、背景の率（1〜2 %）と区別できる数まで測る |
| 11.20 の後 | yield でも 1 件止まった。0 ではない | 「yield は効かない」（8/24 → 1/488 の減りを無視） | 止まった cell を測り直す。結論を「大きく減るが残る」に改め、KB を訂正する |

## HPC の実行

この子は HPC クラスタで実験を走らせていない（Sirius・Pegasus に接続していない）。読んだのはローカルの file と知識ベースだけ。
