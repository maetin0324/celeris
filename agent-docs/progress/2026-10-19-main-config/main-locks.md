---
tasks: [01M5A76P7RHE7TGBE14XGE6YJR]
status: blocked
---
# main-locks: lock 生成は承認済み catalog 実証跡待ち

## 実施
- `check-main-pins.py`: lock の既定置き場を `pinned/main/`（`env-lock.json` / `lock.json`）に変更。内容検証を追加 — A binary の path/sha256（承認 pin と実 file）、worker program の hash、B/C arm の Sonnet-only・claude-code・online_update false・evidence table なし・C は no_evidence_table（Jev 無効・C 呼出し 0）、cap_env と pinned/cap-env.json の一致、release、lock.json の campaign/materials/kb/measurement_id pin・封印形式・K2 commit・config の Sonnet only・Jev は C のみ、両 lock の campaign_sha256 一致。
- `test_check_main_pins.py`: lock 改変・hash 不一致・binary 経路不一致の fixture 試験を追加（29 件 OK）。
- 実 root への checker は exit 1、指摘は 3 件: env-lock.json 不在・lock.json 不在・k2-preflight.json 不在。

## 生成できなかった理由
- offline-lock の入力 `--models-evidence`（`GET /api/v1/llm/models` 本文）と承認 sha256 が実 root・repo のどこにも無い。rehearsal の catalog から偽造しない。
- 実行 argv は run の artifacts `operator-lock-argv.md` に記載。実行後 `--verify` と checker で再照合する。
- よって受け入れ条件 1・2（実 root の lock 存在・再照合）は未達。

## 提案
- 人が実証跡を保存し sha256 を承認 → 上記 argv を実行 → checker の残りが k2-preflight.json のみになることを確認。
