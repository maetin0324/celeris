---
title: pilot の作業場所を git にし stub が成果を commit する
tasks: [01M6MJEG72MD982T211EKAM74G]
status: done
updated: 2026-11-05
---
# pilot の作業場所を git にし stub が成果を commit する

完了日: 2026-11-05

## 変更
- `driver.py`: `make_git_seed` が repo-seed/ を `git init -b main` と初回 commit で作る。案件を作った直後に `POST /projects/{id}/repos`（primary、default_branch main）で登録し、`auto_integrate` を true にする。非 git dir の seed と「該当なしで閉じる」前提のコメントは消した。
- `stub_harness.py`: `out/<id>.txt` を書いた後に `git add` と `git commit` をする。
- `test-git-seed.sh`: seed の git 化と stub の commit が main に載ることを一時 dir で確かめる（LLM・外部 network なし）。

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）
- `python3 -I -m py_compile driver.py stub_harness.py` → exit 0
- `sh scripts/dev/long-horizon-pilot/test-git-seed.sh` → exit 0（`test-git-seed: OK`）

## 未解決
- 実 daemon での通し（`test-long-horizon-pilot.sh`）は未実行。repo 登録の API 呼び出しが実 daemon で通るかは未確認。
- D2 の hidden、D3 の effect、root_row の取り込み判定は他の WorkUnit の担当で触れていない。
