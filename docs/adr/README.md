# docs/adr/ は agent-docs/adr/ へ移った

ADR-0128 により、ADR は `agent-docs/adr/` に置く。番号付きの最後は ADR-0128 で、以後の新しい ADR は
`agent-docs/adr/YYYY-MM-DD-<slug>.md` の形で書く（番号を使わない）。
ここ（`docs/adr/`）に新しいファイルを置かないこと。

移行期間中に走っている branch がここへ新設したファイルは、land の task が `agent-docs/adr/` へ移す。
