---
tasks: [01M4651ZZP8FJKGG6W2WPFNKBH]
unit: routellm-wrapper
status: done
completed: 2026-10-05
---

# Phase 5 routellm-wrapper: RouteLLM /estimate v1 sidecar

## 変更

- `scripts/model-routing/routellm_sidecar.py` に loopback のみで listen する標準ライブラリ HTTP server を追加した。`READY port=<n>`、`GET /healthz`、`POST /estimate`、SIGTERM による port 閉鎖を実装した。
- 実 classifier は RouteLLM `BERTRouter.calculate_strong_win_rate` のみを呼ぶ。`bert` 以外は起動時に拒否する。weights はローカルディレクトリを要求し、Hugging Face の offline 環境変数を import 前に設定する。生成 API は呼ばない。偽 classifier では RouteLLM と ML 依存を import しない。
- RouteLLM の pair win-rate は Celeris の品質指数に未校正なので、全候補の `index` と `confidence` は `null` とし、strong 候補の `reasons` に `raw_pair_win_rate=<score>` を記録する。pair 外候補は `outside_configured_pair`、prompt 不在は `prompt_required`、pair 不足は `pair_incomplete` とした。raw score の表現は v1 DTO に専用欄が無いため暫定であり、下流 report で抽出する。
- `requirements.lock` に RouteLLM full SHA と torch、transformers、litellm の直接 pin を記録した。推移依存の解決と CPU/GPU wheel 選択は人の手順に残す。
- `real-sidecar-check.sh` に opt-in の `start-stop` と `shadow --dataset <dir> --max-requests <n> --out <dir>` を追加した。明示設定とローカル weights が無いと「not run」を表示し exit 2。実 weights の試験はこの run では実施しない。

## 証拠

- `python3 -m unittest discover -s scripts/model-routing -p 'test_*.py' -v` → 3 件成功。共有 fixture を読み、偽起動、healthz、estimate、SIGTERM、port 閉鎖を確認。
- `env -u CELERIS_ROUTELLM_REAL -u CELERIS_ROUTELLM_WEIGHTS_DIR sh scripts/model-routing/real-sidecar-check.sh start-stop; result=$?; test "$result" -eq 2` → exit 0（実スクリプトの exit 2 を確認）。
- `git diff --check` → exit 0。
- `git diff --name-only "${CELERIS_WU_BASE:-HEAD}" -- crates` → 出力なし。

## 未解決事項

- `routellm-weights-use` の人の決定待ち。実 weights の取得・起動・shadow 実測は行っていない。偽 classifier の合格は実測の代替にならない。
- `requirements.lock` は直接依存の pin であり、全推移依存・wheel hash の lock は実行環境が確定してから作成する。
