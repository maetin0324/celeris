# ADR 2026-10-27: 外部 operator と CoS の実行担当を分ける

---
tasks: [01M5XXF7QQ6KB8MVV0RE96ZKJ8]
---

状態: 採用（運用契約と文書。依存する新経路の実装・本番適用は未完了）

## 背景

外部 operator の責務が session memory と scratchpad の巡回台本にしかなく、CoS と二重回答・再試行・起票・配備を行う危険がある。
起票時の棚卸しでは B1 の二重対応、10-26 の release.sh 重複起動、60 秒巡回・既読再通知なし・release ready 対象外の
`inbox_watch_6h.sh`、delivery の non-fast-forward が報告された。本変更では本番を調査・修復したとは扱わない。

## 決定

1. 外部 operator は配備・rollback・credentiald・本番 config・daemon/DB の復旧、および CoS が人へ回し、
   人が operator に委ねた項目だけを扱う。通常の受信箱一次対応・技術レビュー・独自の再起票を並走させない。
2. 起動は人の明示依頼、または C3 の通知・escalate を契機とする。通知は調査の契機であって本番変更の許可ではない。
   全受信箱の常時巡回を廃止する。有限の観測も対象・期限・終了条件を記録し、完了・他者の担当・権限不足・結果不明で停止する。
3. 判断は人、技術的な成果確認は reviewer と決定的 check、要約・取り次ぎは CoS、実行は指定された一人が担う。
   配備する版の選択と配備後の確認を省略しない。実行担当の宣言は運用上の約束であって DB の排他 lock ではない。
4. 人の指示の取り次ぎは B3、operator の回答は A1 の認証 principal を使う契約とする。
   未実装の間は人の認証済み Web 入力に戻す。operator が人の token や CoS credential を借りて人の回答を偽装しない。
   A1/B3 の route や payload は本 ADR では創作しない。
5. 変更直前に元の待ち・revision・既存 operation・delivery/release を読み直す。409 は正常な競合として再照合し、
   同じ回答を押し通さない。外部効果の pending / needs_remediation は結果不明として扱い、新 key で再実行しない。
   同一 release の build/promote は一人が担当し、既存 lock・実行 process・結果を確認する。別経路で lock を回避しない。
6. operator の起動票に実際の model ID、harness/版、session、方針の版、認可根拠・token の scope（値は除外）を記録する。
   sudo の既定許可は無し。人だけに限定した本番操作は人が実行する。本 ADR は権限拡張ではない。
7. push_error は配備失敗と分ける。non-fast-forward は remote の包含関係を確かめてから人が復旧し、force push しない。
   手動復旧後の過去の push_error を本番 DB 直接編集で消さない。再 review・build・promote を表示消去のために起動しない。

具体的な起動票、責任分担表、停止条件、確認手順、依存の実装状況は
[外部 operator 手順書](../../docs/ops/external-operator.md)を正本とする。

## 既存の契約との関係

- [ADR-0051](0051-supervised-delivery.md): reviewer による成果確認、push 失敗でも準備継続、一度だけ再試行を維持する。
- [CoS 全変更操作](2026-10-09-cos-operations-all-mutations.md)・[外部副作用](2026-10-09-cos-operations-external-effects.md):
  人の指示による `release.promote` と監査を維持。selfdeploy の「人だけが API を押す」という古い説明を修正する。
- [ADR-0095 D-d](0095-worker-runs-see-the-db-read-only.md#d-d-本番-host-の操作は人が実行する手順として書く)と
  [CoS production](../../config/skills/cos-operator/production.md): 直接の promote/rollback/systemctl/config 編集の禁止は維持。
- [固定 CoS スレッド](2026-10-22-cos-fixed-threads.md): 案件の固定スレッド（無ければ統括）に記録を集める。
  [受信箱 triage](2026-10-06-cos-inbox-triage.md)の source triple・通知重複抑止は外部 operator の claim にはならない。

## 依存と移行

A1（principal）、B3（指示取り次ぎ）、C3（通知・escalate）、D1・D2 は起票元の改善計画内の識別子であり ADR 番号ではない。
本 checkout ではこれらに対応する新しい仕様・実装の成立を確認できない。既存機能と混同せず、手順書の依存表を満たしてから
新経路を使う。D1・D2 の詳細契約も未確認であり、自己改善の成果確認・版決定・実行・事後確認の責任は本 ADR の分担を維持する。

旧巡回台本は追跡ファイルに存在しない。本番の外部 session はこの変更では停止・削除できないため、人が停止を確認し、
起動設定から外す。旧台本の継続利用を fallback として残さない。新 API、principal、ロック機構、DB migration は追加しない。
