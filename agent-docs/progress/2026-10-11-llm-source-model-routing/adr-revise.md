# adr-revise: 2026-10-11 人の確認の反映と quota 見積り

- task: 01M4M8D4AQHBC2KG3H1BPJ7Z0J / unit: adr-revise
- status: done (2026-10-11)
- 対象: ADR D1/D2/D3/D4/D6、親進捗
- コード変更: なし

## 設計・呼び出し数

対象は Claude 4 model と Codex 3 model。通常 5 model は同じ層別 30 問を各 2 回（各 60 calls）、高価な fable-5-1 / gpt-6-astra は層別 12 問を 1 回（各 12 calls）。理論上の最大は 324 calls（Claude 192、Codex 132）。pilot と本番根拠にこの同じ小標本設計を使う。fable/astra の k=1 は試行内分散を評価できず、区間が広いので、優越が確定しない候補を同等集合に残す。高価 model が区間下限だけの順位で不当に外れないよう、区間重なり・paired差・quotaを合わせ、低確度も記録する。

これは Δ=0.15 を十分な検出力で識別する標本設計ではない。本番データでは約350判定/armが目安とされるため、ここで得る根拠は「小さな差の優越」ではなく、大きな差・明確な劣位の検出、および運用可能な同等集合を目的とする。根拠なしの model を勝者扱いしない。

## quota 根拠（survey と read-only 本番 DB）

調査報告 §2 は本番 `quota_estimated` の measured 行と token/time の分布を使っている。Claude の観測例は opus 0.87 USD/run・7d quota 0.14%/run、claude_max_lab の measured 群 0.27%/run。60 calls を単純乗算するとそれぞれ 8.4% と 16.2% で、後者は週 15% を超えるため呼び出し数を減らす必要がある。fable は 5h quota 約10%/run の観測がある。これは短時間窓の観測であり週 quota へ外挿せず、12 run を同じ窓に詰めない。model/account ごとの実測中央値・時間は評価直前の見積りに反映し、各 account の合計を週15%以下へ制限する。

Codex 本番 quota は2,127件すべて `unknown` と報告され、利用可能な 1-run quota 消費率は算出できない。gpt-6-astra の過去 token 量も他 Codex model より大きいが、token から契約 quota 率を推定しない。10-run dry-run で usage endpoint の前後差から account の基準率を実測し、週15%以内に収まる最大呼び出し数へ調整する。受信箱の実行直前見積り承認まで評価 API を呼ばない。この条件により、上記324は最大計画数であり、承認前の確定呼び出し数ではない。

## routing の既定

先行 `llm_route.rs` の harness→優先順 `route_defaults` を根拠のない期間の唯一の既定にする。人が指定する frontier/standard/cheap ごとの既定も harness×lane の同じ優先列に統合し、genre config に別の defaults を作らない。優先列の最初の適格候補を選び、候補が無い場合は fail closed。根拠表が有効な enforce 中は根拠表規則が優先し、変更希望は shadow に戻して既定を改訂する。

opencode-go は model 数・quota 不明のため今回の評価・根拠表対象外で、必要な harness では明示的 `route_defaults` でのみ動かす。ローカル qwen3.8-27b は評価対象外だが、acp/pi の唯一のローカル候補として route default に残す。別途評価する場合は別の人の決定が必要。

## 変更・確認

- ADR 改訂: D1 7-model 表、k/問題数、週 quota gate、k=1 の根拠基準、opencode-go/qwen の扱い。D2 route_defaults と lane 人既定の統合。D3/D4/D6・Consequences・未解決も追従。
- 引用元: task artifacts `routing-evaluation-survey.md` §1, §2、`survey/prod-data.md`、`survey/prod-data-cells.md`。本番 DB は調査時に mode=ro で読まれた成果を使用。
