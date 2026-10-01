# ADR-0103: Phase 4 の runtime・specialist 採用と旧秘密返却 IPC の廃止

---
tasks: [01M3Q49ZTST3XQ9DGF6AGNR0XG]
---

- 日付: 2026-09-29
- 状態: Accepted（方式選択と拒否境界。P4-A/B/C の完成を意味しない）
- 関連: [ADR-0102](0102-browser-phase4-isolation-injection-routing.md)、[ADR-0101](0101-browser-phase3-identity-contract.md)

## 決定

1. 人の回答 `p4-a-runtime-uid` に従い、bubblewrap と subuid/subgid による専用 host UID を採用する。host 側 trusted controller と filtering proxy を browser・worker から分離する。UID 範囲と helper の運用設定は管理者の責務であり、このタスクは本番設定を変更しない。内部 origin は追加しない。
2. 人の回答 `p4-c-h7-backend` に従い、固定 agent-browser 0.38.1 と既存 harness を専用 browser-specialist として組み合わせ、共通 controller の同一 task fixture で既存 ACP / 明示 Claude と比較する。選択を適合証拠の代わりにしない。機密能力は P4-A/B 実試験まで拒否する。
3. 旧 `resolve.sock` の秘密返却を廃止する。従来は SO_PEERCRED の UID が broker と同じで、binding/lease が正しければ worker も秘密を受け取れた。worker の起動前 routing だけではこの IPC の独立した呼出しを防げない。既存 endpoint は固定コード `trusted_injection_required` を返し、control に許可された PID でも秘密を返さない。拒否は request 本文の構文解析・provider 呼出し・lease 消費より前に行う。切断を正常に完了するため本文は既存の 64 KiB / 5 秒上限で読み捨てる。
4. 旧 plugin `bridge` も固定失敗応答にする。署名済み承認・有効な lease・継承 FD を持っていても旧 protocol に秘密を返さない。将来の controller は別の injection-only IPC を使用し、SO_PEERCRED による役割検証、稼働中の隔離 session、CDP 対象、auth_section を照合して receipt だけを返す。旧 endpoint を設定で再有効化する抜け道は設けない。
5. `CredentialProvider` 契約、broker 内部の lease/use/revoke、H3 と Phase 3 の auth_section は維持する。broker 内部の秘密解決の単体試験は存続する。旧 IPC 成功試験は明示的に拒否試験へ更新し、trusted injection 成功の証拠として扱わない。

## 実装と受入の区切り

P4-A は実 runtime 起動・事実採取・filtering proxy・orphan 回収・稼働中 session への identity 復元までを一単位とする。P4-B は P4-A の controller を使う実 CDP sink・injection-only IPC・実攻撃試験を一単位とする。P4-C は実 fixture runner・specialist・既存 loop・fallback・同一 task 評価を一単位とし、公開能力の評価は P4-A/B と独立して進められる。

方式選択の回答待ちは解消した。実装不足や実機試験未実施を人の再承認待ちに置き換えない。既存の判定関数や fake substrate の成功は実 runtime の証拠に数えない。
