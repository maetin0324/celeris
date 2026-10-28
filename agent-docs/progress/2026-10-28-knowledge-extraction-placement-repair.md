---
title: 日次の知識候補抽出で推測した案件 slug の候補を捨てずに直す
tasks: [01M60JC1S8ZZZ3A7Z98WNQE11M]
status: done
updated: 2026-10-28
---
# 日次の知識候補抽出で推測した案件 slug の候補を捨てずに直す

完了日: 2026-10-28

ADR: [2026-10-28-knowledge-extraction-placement-repair](../adr/2026-10-28-knowledge-extraction-placement-repair.md)

## 変更

- 抽出の入力（`celeris::knowledge_curation::extraction`）に、有効な案件 slug の一覧 `projects` と報告ごとの `project`（元 task の root task の案件、無ければ元 task 自身の案件、無ければ null）を入れた。指示文と langmem runner の指示・schema の説明も「`projects` の slug だけを使い、報告の `project` を既定にする」に直した（schema の説明にあった `project:<id>` は `project:<slug>` に直した）。
- `task_core::knowledge::repair_unknown_project` を追加した。置き場の検査が `UnknownProject` だけで落ちる候補について、元 task の案件があれば `projects/<slug>/…`・`project:<slug>`、無ければ `experience/YYYY/MM/<ファイル名>`・`experience` を返す。`retire` はパスを保って scope を空にする。
- 日次抽出の適用（`extraction::apply`）は、候補の出典の元 task の案件を既定にしてこの直しを 1 回だけ掛け、そのあと従来の `apply_candidates_in` に通す。直してもなお落ちる候補は従来どおり除外され、理由が残る。
- `KnowledgeRunSummary.corrected` を追加した。日次要約の「報告からの候補抽出」に「置き場の修正 N」と各件（元のパス → 新しいパス、理由）を出す。直した件は判断待ちにしない。除外の件数と理由は従来どおり要約と判断待ちに出る。
- langmem runner（`langmem_run.py`）: trustcall の `tool_call_id` の AttributeError のときだけ、`max_attempts=1` で 1 回だけやり直す（`invoke_extraction`）。

## langmem の `tool_call_id` の失敗の調べ

- 原因: trustcall 0.0.39（本番 venv `/local/celeris/state/tools/langmem/.venv`、langmem 0.0.30）の不具合。抽出の tool call が pydantic の検査に落ちると（例: `sources` が無い）、`handle_retries` が `ExtendedExtractState` で `patch` へ送る。patch の LLM 応答が tool を呼ばないと、`_tear_down` はメッセージを変えず、`sync` 節点の条件分岐 `validate_or_repatch` が graph の状態（`ExtractionState`、`tool_call_id` を持たない）のまま `patch` へ戻る。`_Patch.invoke` が `state.tool_call_id` を読んで落ちる（`trustcall/_base.py` の `sync`・`validate_or_repatch`・`_Patch.invoke`）。
- 再現: 本番 venv の python で、`GenericFakeChatModel`（`bind_tools` が自身を返す偽の model）に「`sources` の無い tool call」→「tool を呼ばない応答」を返させると、`create_memory_manager(...).invoke` が `AttributeError: 'ExtractionState' object has no attribute 'tool_call_id'` で落ちた。同じ入力で `config={"configurable": {"max_attempts": 1}}` を渡すと落ちずに終わった。
- 修正の確認: 同じ偽の model で、1 回目は上の失敗、2 回目は「検査に落ちる tool call ＋ 正しい tool call」を返させた。`invoke_extraction` はやり直して正しい候補 1 件（`experience/2026/10/a.md`）を返した。本物の LLM は呼んでいない。
- 決定的な試験: `task-worker` の `invoke_extraction_retries_the_trustcall_patch_state_bug_once_without_patch`（langmem 無しの偽の manager。標準ライブラリの python3 だけで動く）。

## 試験

- `task-core` `knowledge::layout::tests::repair_unknown_project_uses_the_default_project_or_experience` — 既定の案件（slug・案件 ID）、案件なし・未知の既定 → experience、retire、直さない場合。
- `celeris` `knowledge_curation::tests::extraction_tests::guessed_project_slugs_are_repaired_to_the_source_project_or_experience` — fixture の案件 `asmr-one-remix-rust` と案件の無い元 task、偽の抽出出力（`project:asmr-archive`・`project:pra`）。入力の `projects`・`project`、除外 0・受信箱 2・修正 2、置き場、日次要約の文面を確かめる。
- `task-worker` `langmem::tests::invoke_extraction_retries_the_trustcall_patch_state_bug_once_without_patch`。

検査（提出時の HEAD で実行。基準は `git merge-base HEAD main`）:

- `bash scripts/dev/test-affected.sh` — exit 0。nextest 6651 run・6651 passed・21 skipped。web の unit 685 passed。doc test 0 失敗。
- `cargo clippy --workspace -- -D warnings` — exit 0。
- `cargo fmt --all -- --check` — exit 0。

## 本番への移行

本番操作は人が行う。この run では本番 DB・KB・設定を変えず、本物の LLM も呼んでいない。config の書き換えは無い。

1. 新版を配備する。以後の日次・遡りの抽出は、推測した slug の候補を直して `_inbox` に置く。
2. 再抽出の対象を出す。本番 DB は読み取り専用で開く（`sqlite3 -readonly`。可能なら写しを使う）。`$DB` は本番 DB のパス、`$WS` は workspace の根（`/local/celeris/data/workspaces`）。

```sh
# (a) langmem の失敗で抽出が失敗した元 task（tool_call_id の 4 件など）
sqlite3 -readonly "$DB" "SELECT task_id FROM knowledge_runs WHERE via='langmem-daily' AND state='failed';" > failed.txt
# (b) 未知の案件で除外された候補の元 task
sqlite3 -readonly "$DB" "SELECT run_task_id FROM knowledge_runs WHERE summary_json LIKE '%neither a known project slug%';" | sort -u > extractions.txt
python3 - "$DB" "$WS" <<'PY' > excluded.txt
import json, sqlite3, sys
db, ws = sys.argv[1], sys.argv[2]
conn = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
out = set()
for rid in [l.strip() for l in open('extractions.txt') if l.strip()]:
    dropped = set()
    for (summary,) in conn.execute(
            "SELECT summary_json FROM knowledge_runs WHERE run_task_id=? AND summary_json IS NOT NULL", (rid,)):
        dropped |= {r.split(': placement: ')[0] for r in json.loads(summary).get('discarded_reasons', [])
                    if 'neither a known project slug' in r}
    cands = json.load(open(f"{ws}/{rid}/artifacts/knowledge-candidates.json"))['candidates']
    for c in cands:
        if c['path'] in dropped:
            out.update(s[5:] for s in c['sources'] if s.startswith('task:'))
print('\n'.join(sorted(out)))
PY
# (c) 対象の終端の UTC 日（遡りの extraction_date）
cat failed.txt excluded.txt | sort -u > rerun.txt
for id in $(cat rerun.txt); do sqlite3 -readonly "$DB" "SELECT id, substr(updated_at,1,10) FROM tasks WHERE id='$id';"; done
```

   `knowledge-candidates.json` の場所（`$WS/<抽出 task id>/artifacts/`）は抽出 task の workspace の配置から推した値で、本番では未確認。無ければ GUI の抽出 task の成果物から同じファイルを開く。除外された候補の数（少なくとも 17 件）と `excluded.txt` の元 task を突き合わせる。

3. 対象の各元 task を戻す: `celerisctl knowledge rerun <元task-id>`（`rerun.txt` の各行）。`knowledge_runs` の行は消さない。rerun は行を failed・via 無しに戻し、次の遡りか日次の抽出がその task を 1 回だけ拾い直す（`retry_requested`）。この行の run_task_id は日次の抽出 task なので、旧来の 1 task ずつの再試行（`knowledge_maint::retry_failed`）は起きない。
4. (c) で出た UTC 日ごとに、[日次抽出の進捗の本番への移行](2026-10-27-daily-knowledge-extraction.md#本番への移行) の手順 2〜5 と同じく、遡り job の `extraction_date` をその日にして `celerisctl cron run` で発火する。通常の日次 job とは同時に発火しない。
5. 確認: 日次要約の「報告からの候補抽出」で「除外」に `neither a known project slug` が無く、「置き場の修正」に直した件が出ていること。GUI の受信箱（KB の `_inbox`）で直した候補の置き場（`projects/<正しい slug>/…` か `experience/YYYY/MM/…`）を見て、取り込みを判断する。

## 未解決事項

- trustcall の不具合自体は上流のもの。runner はやり直しで避けるが、やり直しは LLM をもう 1 回呼ぶ。上流の修正版が出たら requirements を上げてよい（未確認）。
- 未知の環境分類（`UnknownEnvironmentCategory`）など、案件以外の置き場の誤りは直さない（今回の観測に無い）。

## 提案

- なし。
