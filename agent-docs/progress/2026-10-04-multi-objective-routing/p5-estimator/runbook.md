---
tasks: [01M4651ZZP8FJKGG6W2WPFNKBH]
unit: runbook
status: done
completed: 2026-10-05
---

# Phase 5 runbook: RouteLLM sidecar の手順と手順検査

## 変更

- `docs/ops/model-routing-migration.md` に §10「Phase 5: RouteLLM sidecar」を追加した（ADR §10 Phase 5 の 4 項目）。
  - 10.1 版と出所: RouteLLM full SHA `0b64fdafe049e596a3f5657c219329f24af24198`、wrapper revision `d5225fc41210…`、weights/tokenizer の repository・revision・checksum（revision と checksum は承認後に人が記入する `未記入` 欄。checksum の算出コマンド付き）。
  - 10.2 code/weights/依存の license と notice（weights は未確認と明記）。
  - 10.3 `routellm-weights-use` の承認記録欄。状態 **pending**、記録項目（承認日・承認者・根拠・revision・checksum）。
  - 10.4 Python/torch/transformers/litellm の pin（lock と一致）と専用 venv の構築方法。
  - 10.5 CPU/GPU 要件（検証済み構成なし。未検証構成は保証しない。実測の記録方法）。
  - 10.6 loopback 起動・READY・/healthz・実 /estimate・SIGTERM・PID/port 確認・停止後の heuristic 継続。
  - 10.7 opt-in の config（既存の `[model_routing.estimator.sidecar]` の欄のみ）・allowlist・send_prompt/prompt_allowlist・日次上限・reload・既定 off への戻し方。本番操作は人の手順。
- `scripts/model-routing/requirements.lock` に `# python==3.11`（venv の interpreter pin。pip 対象外のコメント行）を追加。
- `scripts/model-routing/check-runbook.sh`（POSIX sh、offline）を追加。表示名 `routing_routellm_runbook_pins_dependencies_and_license`。§10 の必須見出し、lock と手順の pin の双方向一致、wrapper revision の存在、license 表、CPU/GPU の非保証文、手順のコマンド、wrapper の flag/marker と config 欄の実在、参照 path/相対 link の解決、承認状態（approved|pending 以外は exit 1、approved なら記入欄の充足と SHA 形式）を確かめる。`--require-approved` は pending で exit 1。

## 証拠

- `sh scripts/model-routing/check-runbook.sh` → exit 0（`ok (routellm 0b64fdaf…, routellm-weights-use=pending)`）。`dash` でも exit 0。
- `sh scripts/model-routing/check-runbook.sh --require-approved` → exit 1（`routellm-weights-use is pending`）。
- 負の確認（一時コピー上）: 状態 `maybe` → exit 1、手順の `torch==2.4.0` → exit 1（lock 不一致 2 件）、存在しない script 参照 → exit 1、approved で記入欄未記入 → FAILED (11)。
- `python3 -m unittest discover -s scripts/model-routing -p 'test_*.py'` → OK。
- `git diff --name-only "$CELERIS_WU_BASE" -- crates` → 出力なし。

## 未解決事項

- `routellm-weights-use` は人の決定待ち（pending）。weights の取得・実起動・CPU/GPU 実測はしていない。
- `celerisctl routing evaluate --policy estimator` と reload で proxy へ届く範囲は並行の cli / daemon-wire 葉の成果に依存する。統合後に §10.7 の記述と実装を突き合わせること。
- lock は直接依存の pin のみ。推移依存の freeze は構築した環境で行う手順にした。

## 再走（2026-10-05, attempt 1 修正）
- 前回の check `git diff --quiet $CELERIS_WU_BASE -- crates/ && sh scripts/dev/check-doc-links.sh` が exit 1: check-runbook.sh の case パターンに未作成の report の素のパス（docs/reports/…）を書いていたため、生きた参照として壊れたリンク扱いになった。
- 修正: パターンを `*/model-routing-routellm-shadow.md` に変えた（挙動は同じ）。
- 証拠: `sh scripts/model-routing/check-runbook.sh` → exit 0（pending）、`--require-approved` → exit 1、上の check → `check-doc-links: ok` exit 0。
