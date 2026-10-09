# CoS operation 共通契約（plan v5）

---
tasks: [01M4F5KS8E1MXZDNJTRAVFJESW]
---

`/cos/operations` の domain dispatch は API handler と同じ共有 operation function を呼び、`OperationAudit` を渡す。
A は領域更新、operation row、監査 event/card を一つの SQLite transaction で commit する。B は domain 側の全更新を caller-owned transaction に移し、同様に単一 commit する。

C は外部副作用の前に `begin_external` で pending operation と監査 event を永続化する。外部副作用は一度だけ実行し、結果が得られたとき `finish_external` が pending から applied へ確定する。途中再送は外部副作用を再試行しない。結果不明の古い pending は起動時に needs_remediation にし、人の確認を待つ。しきい値判定には注入時計を用いる。DB migration は追加しない。

分類: execution-plan POST と tree/adopt は B。task integrate、PR merge、knowledge page PUT は C。C の GitHub 呼び出しは fake を使って exactly-once invocation と crash/retry 挙動を試験する。
