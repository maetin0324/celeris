# 日次の知識候補抽出: 案件の置き場を入力で示し、外れた候補を直して置く

日付: 2026-10-28

## 背景

2026-10-28 の遡り抽出（10-03〜）で、少なくとも 17 件の候補が置き場の検査（`kb::place`）で
「project "asmr-archive" is neither a known project slug nor a project id」等として除外された。
抽出の LLM は入力に案件の slug を持たず、題名から slug を推測していた。除外された候補の元 task も
`knowledge_runs` は done になるため、再抽出されなかった。別に、langmem の実行が
`'ExtractionState' object has no attribute 'tool_call_id'` で失敗した元 task が 4 件あった。

## 決定（[日次抽出 ADR](2026-10-27-daily-knowledge-extraction.md) の付記）

D1. 抽出の入力（task の objective）に、有効な案件 slug の一覧 `projects` と、報告ごとの
`project`（元 task の root task の案件の slug。root に無ければ元 task 自身の案件。無ければ null）を入れる。
指示文は「`project:<slug>`・`projects/<slug>/` には `projects` の slug だけを使い、元 task の
`project` を既定にする。案件の無い報告は `experience` などの案件の無い置き場に置く」と書く。

D2. 置き場の検査で「未知の案件」（`PlacementError::UnknownProject`）だけで外れた候補は捨てない。
daemon が決定的に 1 回だけ置き場を直し、もう一度検査に通す。LLM の再実行はしない
（daemon に LLM 呼び出しを入れない。再抽出は費用がかかり、同じ推測を繰り返し得る）。

- 既定の案件: 候補が出典に挙げた抽出対象の元 task（`task:<id>` の先頭から順に）の `project`。
  あれば scope を `project:<slug>`、パスを `projects/<slug>/<元の projects/<x>/ より後ろ>` にする
  （`projects/` の外のパスならファイル名だけを使う）。
- 既定の案件が無い: scope を `experience`、パスを `experience/YYYY/MM/<ファイル名>`（今日の UTC 年月）にする。
  `shared` のような新しい置き場は作らない（案件の無い学びの既存の置き場は `experience/`）。
- `retire` は既存のページを指すので、パスは変えず scope だけを空にする（置き場はパスが決める）。
- 直したあとも検査に落ちた候補は従来どおり除外し、理由を残す。

D3. 直した件は `KnowledgeRunSummary.corrected`（`<元のパス> → <新しいパス>（<理由>）`）に残し、
日次要約の「報告からの候補抽出」に件数と一覧を出す。直した件は人の判断待ちにはしない。
除外の件数と理由（`discarded`・`discarded_reasons`）を日次要約と判断待ちに出すのは維持する。

D4. langmem の失敗は trustcall 0.0.39 の不具合による。抽出の tool call が schema の検査に落ち、
修正（patch）の LLM 応答が tool を呼ばないと、graph の `sync` 節点が `ExtractionState`
（`tool_call_id` を持たない）のまま `patch` 節点へ戻り、AttributeError で落ちる。
runner（`langmem_run.py`）はこの例外のときだけ、`max_attempts=1`（patch に入らない）で 1 回だけ
やり直す。検査を通った tool call は残り、通らない tool call は trustcall が捨てる。
他の例外は従来どおり失敗（retryable）にする。

## 本番への移行

本番操作は人が行う。除外・失敗した元 task の再抽出は `celerisctl knowledge rerun <元task-id>` と
遡り job の再発火で行う。対象の出し方は
[進捗の本番への移行](../progress/2026-10-28-knowledge-extraction-placement-repair.md#本番への移行) に書く。
