# ADR 2026-10-27: task 実行系と通常運用の評価範囲を分ける

---
tasks: [01M5XXF7QDVASQFQBT1GZBXS6V]
---

状態: 採用（評価契約の文書化。計測実装・測定開始は含まない）

## 背景

main-4 A0/B0 の B0 は campaign 実行器が task API を直接呼び、通常の CoS を通らない。
`cos_not_called_basis` を使う task 実行系の比較から、CoS を含む通常運用の性能・自律性・総費用は結論できない。
また、外部 operator の回答・修復・再投入を共有開発として除外すると、実際に必要だった支援を隠してしまう。

利用者の決定（2026-10-27、task comment）に従い、外部 operator（Claude Code 等）は Celeris の評価対象の
構成要素に含めない。実用上の補助として扱い、その判断・操作・消費は「外部 AI による支援」として別に報告する。

## 決定

1. 既存の [A0/B0 契約](../../docs/ops/mlsys-main-4-ab-contract.md)には評価範囲の説明だけを追加する。
   pinned bundle、pilot の条件・承認・成功契約・消費範囲は変更せず、過去の結果を通常運用の測定に読み替えない。
2. 通常運用の評価を独立した revision `main-4-ops-r1` とする。対象は daemon・案件 CoS・統括 CoS・方針・model。
   daemon が管理する planner/worker/reviewer の実行も含む。外部 operator はシステム境界の外に置く。
   CoS への依頼から成果の判定・報告までを測り、task API 直接投入をこの経路の代用にしない。
3. 各試行の release・model・CoS/operator 方針版・入力・評価器と初期状態を固定する。
   自己改善・配備・方針変更等で条件が変われば変更前後の別条件とし、記録を上書きしない。
   混在試行は固定条件の集計から分け、失敗・中断と消費・支援の履歴を残す。
4. operator の仕事は測定前の準備/配備、観測/記録、結果に影響する支援に分ける。
   回答・修復・再投入・設定変更は試行固有の支援。共有作業や protocol として隠さない。
   人が operator の回答を転記しても、判断と実行の主体を分け、外部 AI 支援として残す。
5. 主体・論理要求の語彙は [human-ops /2 ADR](2026-10-27-human-ops-actor-roles-request-ref.md)に揃える。
   A3（主体監査）・A5（human-ops）・A7（CoS 消費）との接続で独自の actor_kind や別の revision 名を作らない。
   未取得・未実装・帰属不能は欠測として理由・件数を出す。呼ばなかった根拠が無い状態を 0 にしない。
6. 指標の出所・取得可能性・報告単位は [通常運用の評価契約](../../docs/ops/mlsys-main-4-ops-r1.md)を正本とする。
   既存の task-run 集計を通常運用の総計と呼ばず、CoS・外部 AI 支援・共有消費・人の操作を分けて提示する。

## 影響と移行

本変更は文書と ADR だけで、API・schema・集計コード・本番構成は変更しない。
A5〜A8 の実装状況と採用 release の証跡を測定開始前に照合する。新 revision の定義は実測定の開始許可ではない。
通常運用の新しい測定 ID・固定票・承認枠を用意し、既存 pinned bundle を上書きせず別に保存する。
[外部 operator 手順](../../docs/ops/external-operator.md)の認可と二重対応防止は維持する。
旧 A0/B0 を残すのは異なる評価対象の凍結済み契約を保全するためであり、旧実装の互換分岐を増やすものではない。
