---
tasks: [01M44T1MKXVD3MF5Y7HZRTHZ24]
---

# 統合の Ready 残留の回収を確かめる手順

task が Ready で工程統合 WU が Running のまま止まる残留を、修正版の active daemon が回収したことを確かめる手順。
仕組みは [ADR-0074 の 2026-10-05 付記](../../agent-docs/adr/0074-parallel-work-units-checkpoints-milestones-quota.md)。

## 昇格後の確認

修正版が current になり active daemon が稼働した後、本番 DB を読み取り専用で開いて、
残留していた統合 WU に `running → pending`、`reason: orphan_takeover` の `WorkUnitTransitioned` が一度だけ記録されたことを見る。

```sh
sqlite3 -readonly /local/celeris/data/db/celeris.sqlite3 \
  "SELECT task_id, ts, json_extract(json,'$.key'), json_extract(json,'$.from'), json_extract(json,'$.to')
     FROM events
    WHERE json_extract(json,'$.type') = 'work_unit_transitioned'
      AND json_extract(json,'$.reason') = 'orphan_takeover'
    ORDER BY id DESC LIMIT 20;"
```

その後、同じ WU が `pending → running`（reason=integrate）を経て Done へ進むことを GUI の task 詳細か events で確かめる。

生きている別の active / draining daemon がある間は、持ち主の保護により回収は見送られる。
引き継ぎが終わってから確認する。
