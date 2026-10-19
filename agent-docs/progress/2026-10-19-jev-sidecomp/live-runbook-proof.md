---
title: "独立読取照合の運用手順・欠測扱い・最終検査を整合"
tasks: [01M59W8DFMVT3M8WRM4XP8ZEWE]
unit: live-runbook-proof
status: done
updated: 2026-10-19
completed: 2026-10-19
---
# live-runbook-proof

`docs/ops/jev-c1-live-check.md` を統合済みの検証器（独立読取元 `CELERIS_JEV_SOURCE_DB`・各 trial の凍結候補/pin 照合）に合わせて更新。front matter の tasks を本 task に直した。

- 旧記述（provenance の source/sha256/ULID だけで合格、手製 2 file を信頼する手順）を削除。provenance/routing_events は照合される側で、原記録と全件一致しなければ不合格と明記。
- 運用者設定の読取元・ro 権限・ID 照合（run_id 結合）・投影変換・信頼境界、終了コード 0/1/2/3（未設定・取得不能・schema/欄不足・campaign 不在は非 0）を記載。呼出し行は維持。
- fixture（builder.py の test double）と実証の区別、結果保存、各段件数・欠測・抑止理由・全役割 Jev retry 費用の確認手順を記載。
- review 再現 2 件の回帰試験名を記載（`test_live_source_contract.py`、`test_per_run_conditions.py`）。

## 実 export 検証の有無
**実 daemon の DB・実 export での検証は未実施（未検証）。** 実データが手元に無く、手組み正例では実検証済みと書いていない。本番変更・有料実行なし。

## 証拠
- `python3 -m unittest discover -s scripts/dev/tests/jev_live_evidence` → 87 件 OK（直前 unit。本 unit はコード変更なし）
- `sh scripts/dev/check-doc-links.sh docs/ops/jev-c1-live-check.md` → ok（exit 0）／`check-doc-layout.sh` → ok
- `bash scripts/dev/test-parallel.sh` → exit 0（passed 5840 / failed 0 / ignored 15）
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0

## 未解決・提案
- `provenance.json` の tasks と `routing_events.jsonl` を原記録から作る生成器が無い（手作業。値は照合されるので偽造では通らない）。`celerisctl campaign export` に出させるのが望ましい。
- 凍結候補は script 内定数（manifest 本文との自動照合は未）。Jev の model_snapshot が記録に出ない問題（inventory 不足 1）は残る。
