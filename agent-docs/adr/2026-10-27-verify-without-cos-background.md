# verify では CoS の背景処理を登録解除する

- 日付: 2026-10-27
- 状態: 採用
- tasks: [01M5XXF7Q8133JDMR051R4MHJX]
- 関連: [ADR-0041](0041-self-improvement-loop-hardening.md)、[CoS triage](2026-10-06-cos-inbox-triage.md)

## 根拠

human-ops の配送で build と試験は成功したが、staging smoke が reviewing のまま60秒で失敗した。
ログの dispatch 区間は約21秒・38秒。対象外 ready task の除外は既に実装済みだった。
同区間の `tick_cos_chat_launch` は verify でも登録され、タスクの eligible filter を通らず
snapshot の受信箱を再照合する。`cos.enabled=false` でも intake・fallback・root done 処理は残る。
この経路は verify の「煙試験だけ、背景ジョブなし」という契約にも反する。

## 決定

verify の起動時、最初の tick より前に CoS launch の登録を外す。
通知ループの起動時に行う CoS route cutover も verify では行わない（初期カーソル・通知の移行を防ぐ）。
通常運用の `cos.enabled=false` の fallback は維持する。reload は未登録の launch を作らない。
タイムアウトや合格条件は変更しない。CoS 関連表が変化せず、fake smoke が done になることを
隔離 DB の既存 verify 試験で検査する。実 snapshot 上の所要時間は次回配送で確認する。
