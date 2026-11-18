---
title: アーキ v4 D contract-tests — org の HTTP と CoS の書き込みの契約試験を旧コードに当てて固定する
tasks: [01M7MZTKB4RYM80S6CT3RST2N9]
status: done
updated: 2026-11-18
completed: 2026-11-18
---
# contract-tests（WorkUnit）

基準は `$CELERIS_WU_BASE`。`crates/task-api/src` と `crates/task-core` は変えていない（差分 0 行）。

## 変更
- `arch-v4 D contract-tests: org の HTTP と CoS の書き込みの契約試験を旧コードに当てて固定する` —
  `crates/task-api/tests/cos_delivery_suite/org_write_contract.rs`（async の試験 6 件）を足し、
  `cos_delivery_suite/main.rs` に `mod org_write_contract;` を 1 行足した（新しい test binary は作らない）。
- 書き込みは公開の経路（`/api/v1/org…`・`/api/v1/cos/operations`）だけで行う。DB は rusqlite で読むだけで、
  tx の途中の失敗は試験が置く SQLite の trigger `contract_fault` で起こす。各試験の不変条件と、既存の試験
  （organization.rs・cos_ops_admin_config.rs・cos_operations.rs）で検出できない理由は file の先頭の comment にある。

| 試験 | 守る条件 |
|---|---|
| `cos_org_writes_commit_row_browser_event_operation_and_audit_together` | (a) CoS の 4 操作が org の行・org_browser_events（actor `cos`、grant を変えるときだけ）・applied の行・監査 event を書く。`expected_revision` は送った値のまま残る |
| `cos_org_write_failing_inside_the_transaction_leaves_the_org_unchanged` | (a) applied の行・監査 event・chat の card のどこで落ちても org の行と org_browser_events は残らず、rejected の行 1 件と rejected の監査 event 1 件だけが残る（3 段 × 4 操作） |
| `http_browser_write_failing_after_the_row_leaves_the_org_unchanged` | (a) HTTP の browser の書き込みは org の行と org_browser_events（actor `admin`）を同じ tx で書く |
| `org_refusals_keep_status_code_and_cos_records_the_reason` | (b) 9 件の拒否で HTTP と CoS の status・code・detail が同じ。reason は plan の拒否で `rejected: <detail>; requested reason: …`、tx の中の拒否で要求の reason のまま |
| `org_writes_need_an_admin_principal_and_cos_writes_need_the_cos_run` | (c) 誤った token は 401、CoS の credential で直接呼ぶと 422、`/cos/operations` は token 無しで 401、人と operator の token で 403 |
| `org_http_and_cos_responses_keep_their_shape` | (d) HTTP の 201・Location・200・204 と body の key の集合と値、CoS の応答と operation view の key の集合と result |

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p task-api --test cos_delivery_suite org_write_contract` → exit 0、6 件 pass（20 件 skip）。
- 検出力の確認: `handlers/org.rs` の `org_commit` で、CoS の tx の前に `store.org_upsert(node)` を呼ぶ一時的な変更を入れて同じ command を走らせ、
  3 件が fail した（`operation-row org.create: org changed` ほか）。変更は `git checkout` で戻し、commit していない。
- `git diff $CELERIS_WU_BASE -- crates/task-api/src crates/task-core | wc -l` → 0。
- `bash scripts/dev/test-affected.sh` → exit 0（nextest 6109 件 pass・21 件 skip、web の tsc -b と vitest 705 件 pass、node --test 110 件 pass）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo clippy -p task-api --test cos_delivery_suite -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。

## 未解決事項
- 依頼の例の「expected_revision の不一致」と「既に解決した triage item の mark」は、今のコードでは org の操作に届かない
  （org.* は `expected_revision` を記録するだけで比べない。triage item の mark は `/cos/inbox/{i}/resolve` だけが付け、その回答は
  decision・authorization・question などの経路で org にならない）。同じ段の失敗を trigger で起こして代えた。

## 提案
- CoS の拒否の reason は、plan の段の拒否（`rejected: <detail>; requested reason: …`）と tx の中の拒否（要求の reason のまま。
  detail は result の `error`）で形が違う。この試験はその今の形を固定している。usecase の WorkUnit が検証を tx の内外に動かすと
  reason の形が変わるので、動かすなら ADR で形をそろえるかを決めてから試験を変える。
