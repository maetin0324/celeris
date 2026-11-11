# クラスタ job の失敗・長い待ち行列での再開

- 日付: 2026-11-11
- 状態: Accepted
- 関連: [ADR-0090](0090-durable-wait-for-cluster-jobs.md) D1–D7

## 決定

wait の再開条件を全終了に加えて、失敗終了、Q/H の連続観測時間超過に広げる。
PBS の終了コード非ゼロ、Slurm の失敗終端状態または非ゼロ exit/signal を失敗とする。
全終了は従来どおり扱う。部分失敗は次の poll 結果で再開する。
Slurm の State は `State%64` で取得し、OUT_OF_MEMORY 等の長い状態名の切詰めを避ける。

`clusters.job_wait.queued_wake_secs` は Q/H の再評価までの秒数。既定は 3600 秒、正の整数。
最初に Q/H を観測した時刻から数え、R 等を観測すると連続時間をリセットする。
Q と H の移動ではリセットしない。観測不能では時計を進めたと判断しない。
同じ task・WorkUnit・cluster・scheduler・job id について、長い Q/H は一度だけ通知する。
失敗の起床は抑制しない。同じ ID を再実行した後の失敗も次の poll で調べ直せる。
待ち直し・daemon 再起動をまたぐ時刻と通知済み情報は既存 wait の status JSON と event に保存する。
同じ job を待ち直した際は過去の wait の情報を引き継ぐ。再投入による新しい job id は新しい対象になる。

新しい wait 状態や DB 列は作らない。`satisfied` は再開条件成立を表し、早期再開では
`ClusterJobWaitFinished.detail` を `job_failed` または `queue_stalled` とする。
ContinuationContext.cluster_jobs.state にその理由を渡し、全 job の現状を保持する。
PBS の Resource_List、comment、estimated.start_time は折返しを含めて前置きへ渡す。
既存 event の読み戻しでは追加 status 欄の省略を許す。

再開は既存の ClusterJobResume / needs_continuation を使う。待機の run を continuation 回数・
進捗なしの窓に算入しない D1、全成功時の再開、上限時の質問は変更しない。
Celeris は qdel/scancel しない。worker がログと qstat -x を確認し、測定条件と承認の範囲内で
再投入・資源指定の緩和・取消を判断する。測定条件でないホスト固定は禁止し、必要なら進捗に理由を書く。

## 本番への移行

`queued_wake_secs` を本番設定に足すのは、そのキーを知る release の昇格後に人が行う。
省略時も既定値で動く。BenchFS の task や本番 job はこの変更では操作しない。
