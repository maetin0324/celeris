# A2 の保存境界と独立外部 process

## 決定

正式 dagbench harness の A2 は外部 process を Supervisor で所有し、IpcExt で制御する。
controller は backend の次の呼出しで保存状態を読み、選択した job の Waiting が確認できた時だけ park する。
親は別の store 接続（Celeris）または保存済み state（baseline）で再確認してから所有 Child を SIGKILL する。
外部 process は observer の論理時計進行で完了し、sync ack の後に controller を同じ state で再起動する。
これは実時間の HPC 測定ではない。タスク3の collector schema は使用しない。

保存前の旧条件は `a0-restart-before-wait-save` として分離する。過去 raw は変更しない。
A2 は専用 raw schema と process-events を持ち、共通 graded metrics は生存証明の検証後に計算する。
タスク4の grader pin、attempt、validated completion はそのまま使う。

## 検証で判明した費用境界

最初のwaitで停止すると、baselineの別nodeがSubmitted保存後・実投入前になる。
そのnodeのpre作業は完了済みだが、従来は外部wait開始までworker busy区間が閉じず、停止中の990秒も費用へ入った。
両armでpreの作業完了を投入より前にexecution_finishedとして記録する。既存rawは再解釈・上書きしない。
