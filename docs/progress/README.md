# docs/progress/ は agent-docs/progress/ へ移った

ADR-0128 により、進捗の記録は `agent-docs/progress/` に置く（task ごとのファイル、または段の WorkUnit ごとのファイル）。
ここ（`docs/progress/`）に新しいファイルを置かないこと。

移行期間中に走っている branch がここへ新設したファイルは、land の task が `agent-docs/progress/` へ移す。
