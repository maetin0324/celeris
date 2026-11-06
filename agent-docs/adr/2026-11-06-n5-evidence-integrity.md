# ADR 2026-11-06: N5 証跡の条件の固定と再達成、GC 最大 ID の保護

状態: 採用。

## 決定
- 人・CoS の root の起票時に、承認済み Roadmap の版への参照を DB の INSERT trigger で同じ transaction に保存する。条件は既存の版の行を参照し、root ごとに複製しない。保存行は追記専用とし、古い task への推測による補完はしない。snapshot は起票時と現在の acceptance・checks の一致と root の command を照合する。記録が無い旧 root は Roadmap の根拠にならない。stale_outcome_ids は acceptance が一致して根拠になり得る Outcome に限り、無関係な ID で snapshot を膨らませない。controller の root は既存の起票 CAS と digest を使う。
- 達成の履歴を残し、新しい根拠で再達成したときは reached を追記する。controller の同じ版・同じ根拠は重複記録しない。人の状態操作と理由は従来どおり追記する。TS は Roadmap の現在の evidence を見て更新を提案し、Rust が再検証する。
- 状態の検証と記録の間の承認版・案件の root の event の変更は CAS で拒む。既存の root event mark を使い、他案件の event は競合にしない。権限と human_epoch の門は既存の API・効果の門を維持する。
- migration 0118 で 3 表の最大 ID の DELETE を拒む trigger を追加する。permit があっても拒む。既存の GC 候補選択・archive hash・監査は維持する。
- 本番変更は行わない。migration は通常の release で適用する。旧 root に条件の記録が無い場合は新しい root の実行が必要になる。

## 検証時の修正

既存の `actor::tests::terminate_then_reuse_keeps_half_written_state` は Tokio context の外で isolate を作り、V8 の delayed task が発生すると SIGABRT になった。試験も本番 actor と同じく Tokio runtime に enter してから isolate を作り、isolate の破棄まで context を保つ。製品の runtime の挙動は変えない。
