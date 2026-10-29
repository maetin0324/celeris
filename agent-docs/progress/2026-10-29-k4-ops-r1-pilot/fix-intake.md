---
title: ops-r1 B の CoS 受付の記録（g-b-cos-intake）
tasks: [01M62TJ5V2B65Y0C804HHQ4E3Q]
status: done
updated: 2026-10-29
---
# ops-r1 B の CoS 受付の記録

完了日: 2026-10-29

## 結果
- `celeris-campaign ops-r1-intake` を追加した（`crates/campaign/src/ops_r1_intake.rs`、手順は `docs/ops/ops-r1-intake.md`、
  判断は ADR `2026-10-29-ops-r1-cos-intake-record`）。
  - `--bind-message <id>`: 人が web・celerisctl で案件 CoS thread に送った message を trial に結ぶ。
  - `--send`: 人の token で `--request-file` の固定依頼文を `POST /api/v1/chat/threads/{t}/messages` で送る
    （`client_message_id = ops-r1-intake:<trial_id>`、再実行しても 1 件）。
  - 出力 `ops-r1-intake/1`: trial_id・condition_id・thread_id・project_id・message_id・message_seq・accepted_at（daemon の
    created_at）・request_sha256・request_bytes・intake_mode・client_message_id・actor{kind,id}。
  - 呼ぶのは chat thread の GET 2 種と POST messages だけ。`POST /api/v1/tasks` は呼ばない。
  - author が `human:` でない message（operator の relay・MCP・author 無し）、本文の sha256 不一致、統括 thread は拒否する。
- daemon・task-*・web のコードは変えていない（daemon 側の変更は不要だった）。

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`）
- `cargo nextest run -p celeris-campaign -E 'test(ops_r1_intake)'` → 6 tests run: 6 passed
  （偽 API の unit 5 件 + 実 task-api router・一時 DB の結合 1 件。結合試験で tasks 表 0 件・author=human:test を確認）
- `bash scripts/dev/test-affected.sh` → exit 0、2815 tests run: 2815 passed、文書検査 ok
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- 許可 path の検査（`git diff --name-only $(git merge-base HEAD main)` を許可 prefix で除外）→ 出力なし

## 未解決事項
- 測定 daemon（g-cos-measure-daemon）に対する実機での受付は未実施（その葉の preflight で行う）。
- `--send` で daemon が人以外の author を返した場合、message は送られた後で記録だけを拒否する。人が message_id を確かめる。

## 提案
- trace export（g-b-trace-export）はこの記録の message_id・accepted_at を起点にすると、受付段の参照を同じ値で結べる。
