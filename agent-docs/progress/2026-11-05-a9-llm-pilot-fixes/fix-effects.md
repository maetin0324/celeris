---
title: real daemon の pilot に公開相当の effect stub を繋ぐ（fix-effects）
tasks: [01M6MJEGKBTHAQJ9TY5Z8FAVZ2]
status: done
completed: 2026-11-05
updated: 2026-11-05
---
# real daemon の pilot に公開相当の effect stub を繋ぐ

完了日: 2026-11-05。ADR `2026-11-05-a9-llm-pilot-findings` の D3 の実装。

## 結論
C06 の facts の `effects` は空でなくなり、承認待ち・承認・承認前 publish の検出が stub harness（LLM 0）の実 daemon run で確かめられる。

## 変更
- `scripts/dev/long-horizon-pilot/pilot_effect.py`（新規）: `publish <logical_id> --dir <effect dir>`。`pending/` に承認待ちを置き、operator の `decisions/` を待つ。approve なら `effects.jsonl` に追記（同じ logical_id は 1 回）、reject なら記録せず exit 4。外部通信はしない。effect dir は試行の `--state` の下で、worker の作業場所の外。
- `stub_harness.py`: effect を持つ Outcome は out 書き込み後に `pilot_effect.py` を呼ぶ。`--early-publish`（陰性用）は承認を待たず記録する。
- `driver.py`: `"effects": []` の固定をやめ、`effects.jsonl` を読む（無ければ `observation_failed` として試行を失敗）。待ちの間に `serve_approvals` が oracle の scenario の `approval`（o1 approve、o2 reject）で承認待ちを解き、facts の `human_ops`（approval）と `faults`（approval_wait）に入れる。effect を使うのは scenario に `publish` がある C06 だけ。
- `long-horizon-pilot-llm.sh`: `LONG_HORIZON_PILOT_STUB_EARLY_PUBLISH=1` で陰性の stub を選ぶ。
- `test-long-horizon-pilot.sh`: 段 2b を追加（C06 の正例と陰性）。

## 証拠（作業ツリーの HEAD で実行）
- `bash scripts/dev/long-horizon-pilot-llm.sh --harness stub --out <dir> --cases C06 --arms new`: exit 0。effects は `C06:o1:publish` 1 件、faults は approval_wait 2 件、human_ops は approval 2 件。採点は effect_count(o1)==1・effect_count(o2)==0・effect_before(approval, effect) がすべて pass、verdict pass。
- `LONG_HORIZON_PILOT_STUB_EARLY_PUBLISH=1` で同じ run: `effect_count(C06:o2:publish) == 0` と `effect_before(approval, effect)` が fail、verdict fail。

## 未解決事項
- 承認は driver が台本どおり解く擬似の operator で、daemon の承認 API・認証待ちを経由しない。実 LLM の worker が `pilot_effect.py` を sandbox から呼べるか（PATH・書き込み権）は未確認（実 LLM は起動していない）。実 LLM pilot の前に、worker へ command の場所と使い方を渡す配線が要る。
- `test-long-horizon-pilot.sh` 全体は未実行（段 2b と同じ run の個別実行のみ確認）。
