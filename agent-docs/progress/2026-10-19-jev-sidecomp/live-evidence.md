---
title: "実 campaign export 検証器 jev-live-evidence.py（B1/C1）"
tasks: [01M59W8DFMVT3M8WRM4XP8ZEWE]
unit: live-evidence
status: done
updated: 2026-10-19
completed: 2026-10-19
---
# live-evidence

- `scripts/dev/jev-live-evidence.py --evidence-dir <dir> --require-live`（標準ライブラリのみ、外部通信なし）。終了コード 0 合格 / 1 不合格 / 2 live 証拠なし（dir 無し・空・必須 file なし）。
- 入力 dir: `records.jsonl` / `summary.json`（`celerisctl campaign export` の出力そのもの）、`provenance.json`（source=`celerisctl campaign export`・campaign_id・manifest_hash(64hex)・generated_at・export_command・files_sha256・tasks 木）、`routing_events.jsonl`（`routing_decided` の写し。`{task_id, run_id, event, allowed?, max_lane?, fixed_lane?}`）。provenance と routing_events は現行 export が出さないため、人が export 時に添える（手順は runbook unit が docs/ops/jev-c1-live-check.md に反映）。
- 検証: 出自（手組み fixture・改変・campaign/hash 不一致・非 ULID task_id を不合格）、別 campaign/別 hash/arm A/B1 と C1 の混入、routing_resolved の pin（mode・evidence_mode none・source=task）が root/子/孫/retry で一致、C1 の lane_assist・明示 lane 抑止・Sonnet+Opus 候補集合（単一モデル pin 拒否）、exec_model、jev_smoke::judge 相当（jev_ok・費用・応答版・許可内・帰属）、`real_api_verified` は証拠にしない、全役割・retry の費用を summary と record で突き合わせ、jev_stages と同じ段定義の件数・抑止理由・欠測を JSON で出力。
- 前 unit の契約: side manifest（B/C のみ、根拠表 none、候補に sonnet と opus、mode 以外同一）に合わせた。manifest_hash は side manifest の hash を provenance で指す。
- 試験: `python3 -m unittest discover -s scripts/dev/tests/jev_live_evidence`（28 件 OK）。正例 = 実 export 形式（root/子/孫/retry + B1）、負例 = fixture 単独・改変・fake 成功・欠測(費用/版/exec model)・pin 不一致・混入・単一モデル・明示 lane・根拠表・費用不一致・被覆不足・証拠なし。
- 未確認: 実 daemon の live export は未入手（既定 dir は存在せず exit 2）。
- 提案: `celerisctl campaign export` に provenance.json と routing_events.jsonl の出力を足せば手作業が要らなくなる（本 unit では本体を変えない）。
