---
title: CoS の継続 session を方針の版で切り替え、操作の方針の版を daemon が決める
tasks: [01M60JC1R91HP7Y1B9AKA07B7V]
status: done
updated: 2026-10-28
---
# CoS の継続 session を方針の版で切り替え、操作の方針の版を daemon が決める

完了日: 2026-10-28

ADR: [2026-10-28-cos-session-policy-key](../adr/2026-10-28-cos-session-policy-key.md)

## 変更

- 照合 key に `policy_key` を足した（`policy_version=<[cos.triage] policy_version>;<skill>=<中身の sha256>;…`）。
  保存は `node_sessions.policy_key`（migration 0100、SCHEMA_VERSION 100）。方針の部分だけが違うときは
  `policy_changed` で retire して fresh（summary＋未要約履歴）にする（`task_dispatch::sessions::cos_chat`）。
  hash は `dispatcher/cos_chat/policy.rs`。
- daemon は launch 時に `chat_runs.resolved_config_json` の `policy_version`・`policy_key` を記録し
  （`SqliteStore::chat_run_record_policy`、credential の発行より前）、run には `cos_chat.policy_version` と
  環境変数 `CELERIS_COS_POLICY_VERSION` で渡す。
- `/cos/operations` と `/cos/inbox/{i}/resolve` は body の `policy_version` が run の記録と違う、または記録が無いとき
  422 `cos_policy_version_mismatch`（daemon の版で rejected 行を残す。`task_api::cos::operations::bind_run_policy`）。
- 旧い仕組みを消した: Core の OP 例の固定値 `"5"`、`celerisctl` の `CELERIS_COS_POLICY_VERSION` 既定 `"1"`、
  skill（`config/skills/cos-operator`・`cos-inbox-triage`）の固定の版の例。
- worker protocol の schema（`docs/protocol/worker-protocol.schema.json`）と `docs/api/v1/api.md` を更新した。

commit（件名）:
- phase 1: CoS の継続 session を方針の版と skill の中身で切り替え、操作の方針の版を daemon が決める
- phase 1: 方針の版の検査に合わせて migration 試験・celerisctl・代答の試験を直す
- phase 1: 進捗ファイルを書く

## 本番で "2" が記録された原因の調べ

本番 DB・本番 log は読んでいない（run から読めない）。repo の履歴から分かったこと:

1. 照合 key に skill も版も入っていなかった。KB の skill を差し替えても、BenchFS の thread の次の run は
   同じ claude session を resume する。Claude Code の skill の本文は、読んだ turn の文脈に残る。
   `cos-inbox-triage` の版 2 は 2026-10-08 から 2026-10-27 まで使われていた（`config/skills/cos-inbox-triage/SOURCE.md`）。
   この thread の session が版 2 の時期に skill を読み、その文脈のまま 05:20:51Z に resolve した、という見立てと矛盾しない。
2. daemon は run に版を渡しておらず、API は body の値をそのまま記録していた。run が書く値は skill の本文・Core の例
   （2026-10-27 までは `"3"`、以後 `"5"`）・`celerisctl` の既定 `"1"` のどれでもあり得た。01:41 の `"4"` と 05:20 の `"2"` が
   違うのは、run ごとにどれを読んだかで決まっていたため、と考えられる。
3. 未確認: 05:20:51Z の run が resume だったか（`chat_runs.resolved_config_json.session_mode`）。人が確かめるなら
   本番 DB の写しで次を読む:
   `SELECT r.run_id, json_extract(r.resolved_config_json,'$.session_mode'), r.session_row_id FROM cos_operations o JOIN chat_runs r ON r.run_id=o.run_id WHERE o.action='inbox.escalate' AND o.policy_version='2';`

この変更の後は 1・2 とも起きない（版か skill が変われば fresh、違う版は 422）。

## 証拠（提出時の HEAD で実行）

- `cargo nextest run -p task-dispatch -E 'test(cos_chat_policy) | test(without_policy_key)'` → 2 passed。
  `cos_chat_policy_change_makes_next_run_fresh_and_records_new_version`: 同じ方針は resumed、版を 7 にした次の run は
  fresh / `policy_changed` で run の記録と `cos_chat.policy_version` が "7"、未要約履歴が渡る。skill の本文を変えた次の run も
  `policy_changed`、その次は resumed。`cos_chat_session_without_policy_key_is_retired_as_policy_changed`: NULL の旧い行は resume しない。
- `cargo nextest run -p task-api -E 'test(policy_version_must_match)'` → 2 passed（resolve と OP: 違う版は 422・適用なし・
  rejected 行は daemon の版、記録の無い run は 422、正しい版は 200 で daemon の版を記録）。
- `bash scripts/dev/test-affected.sh` → exit 0（6655 passed、21 skipped。web の tsc と unit も通過）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。

## 本番への移行

- migration 0100 は昇格時に自動で当たる。既存の session 行は `policy_key` が NULL なので、配備後の最初の run はどの thread も
  fresh（`policy_changed`）になる。
- 本番 KB の `cos-operator`・`cos-inbox-triage` を repo の `config/skills/` の版に差し替える（人が行う）。差し替えないと
  skill の例が固定の版を書いたままなので、run が skill の値を使うと 422 になる（Core の指示と run の値が優先なので、多くは通る見込み。未確認）。
- config の変更は無い。

## 未解決事項

- `scripts/dev/jev_source_reader.py` の `KNOWN_SCHEMA_VERSIONS` は 98 のまま（99 の時点から未更新。別 task の followup）。

## 提案

- `cos-operator` の `metadata.version` と `[cos.triage] policy_version` は別の数で、両方を「policy_version」と呼んでいる。
  記録の版は後者だけになったので、cos-operator の版は `policy_key` の hash で追う形に一本化し、`metadata.version` の
  「policy_version」の呼び名をやめる案がある。
