---
tasks: [01M5A6Y9RB8WRN5YY8XEXVCN5K]
status: done
completed: 2026-10-19
---
# main-pins: 固定 root 移行の確認と check-main-pins.py

## 実装
- `scripts/campaign/check-main-pins.py --root <artifacts>`（読むだけ。LLM・daemon・network・config/DB に触れない）。問題を `NG ...` で stderr に出し、1 件でもあれば exit 1。
- 固定 root の移行は既に `pinned/main/launcher.env` に反映済み（CASES_ROOT/GRADER_ROOT/GRADE_CASES_ROOT = `pinned/main/fixed/{cases,grader-private,grade-cases}`）。checker はこれを必須とし、`/wu/`・`/pin-values/` への参照を pinned/main 全体で拒否する。
- 検査内容: launcher.env（固定 root・実在 path・承認 release c9654bff5633・CAMPAIGN_BIN/CAP_ENV の pinned 内 path・秘密値なし）／final.json（primary=[A,B]、A/B とも Sonnet のみ、B routing.models・run_harnesses=[claude-code]・online_update false・evidence table なし、C は secondary のみ・Sonnet・no_evidence_table、per_arm×2 ≤ 全体で二重計上なし、入力・materials・kb の承認 hash、K2 commit）／daemon.toml（real harness は claude-code のみ、全 model Sonnet、cap-probe 宣言、Jev 参照なし、langmem/gc 無効、loopback・非本番 port、測定専用 path）／cap-env（A と worker が同一）／fixed（K2/K4 の cases・grade-cases・private、K2 initial.lock の commit）／公開側分離（公開 root に private.json・grader 名の path・private file の内容 hash 一致・canary 禁止語・private への symlink・root の重なりがない）／lock（`pinned/main/lock/{env-lock.json,lock.json}`。`--lock-dir` で変更可。生成は次 unit main-locks）／実証跡（cap-a-k2/k4・cap-b が ok・paid 0、実 K2 preflight 成功証跡 `pinned/main/evidence/k2-preflight.json`）。
- 固定 hash 定数はコード先頭に置いた（承認済みの値。変えるときは checker も同じ変更で更新する）。

## 証拠
- `python3 -m unittest discover -s scripts/campaign -p "test_check_main_pins.py"` → 21 件 OK（成功、lock/証跡欠落、失敗 preflight、path 欠落、旧 root、秘密値、hash 改変、非 Sonnet、harness/evidence 同等性、予算二重計上、Jev、cap-env 差、grader 混入 4 形態、root 重なり、CLI exit code）。
- 実 artifacts に対して実行: exit 1。指摘は 3 件だけ — `pinned/main/lock/env-lock.json`・`lock.json` の不在（main-locks の担当）と、実 K2 preflight 成功証跡の不在（未取得）。それ以外（設定・path・hash・公開側非混入）は合格。成功扱いはしていない。

## 未解決・提案
- 実 K2 preflight 成功証跡（`pinned/main/evidence/k2-preflight.json`）は未取得。lock 生成後に人が offline で preflight を実行して置く。lock の置き場所（`pinned/main/lock/`）は main-locks が違う場所に作るなら `--lock-dir` か定数を合わせる。
- 本 unit は Rust を変えていないため test-parallel / clippy は回していない。
