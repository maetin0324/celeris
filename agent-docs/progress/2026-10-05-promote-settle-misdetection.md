---
title: promote.sh の「active が 2 つ」判定の誤検出と打ち切りの安全側（2026-10-05 03:44 の停止）
tasks: [01M452R2MYRS45WCF15R73VQ0J]
status: done
updated: 2026-10-05
---

# promote.sh の「active が 2 つ」判定の誤検出と打ち切りの安全側

完了日: 2026-10-05。設計は [ADR-0040 付記 2026-10-05b](../adr/0040-self-improvement-deploy.md)。
人が確かめる手順は [docs/ops/single-active-handoff.md](../../docs/ops/single-active-handoff.md)。
本番への昇格は未実施（この版の `promote.sh` は、この版が `current` になった次の昇格から効く。ADR-0040 付記 2026-10-02 の規則）。

## 何が起きていたか（原因の特定）

本番 2026-10-05 03:44〜03:46 UTC の停止。journal（`celeris@de69d5634efb` / `celeris@97468bdf3093`）と
`/local/celeris/state/logs/promote-20261005-034449.log` を突き合わせた。

- daemon 側は正しかった: 03:44:52 旧 `01M44YYWNK…` が `active -> draining`（API を閉じる）、03:44:53 新 `01M452M4C0…` が
  `standby -> active (no other active is alive)`、旧が手放した WU 検査を引き継いだ。
- `promote.sh` の `poll_instances_settled` は `GET /api/v1/releases` を**トークン無し**で読んでいた。この API は `/api/v1/health`
  以外の全 API と同じく Bearer 必須（`crates/task-api/src/middleware.rs`。この host で `curl` すると 401）。読めなかった失敗を
  「another instance is active」と記録して 60 秒繰り返し、「two active instances」の文言で新を止めた。旧は draining で
  listener を閉じていたので active が 0 になった。
- `sd_instances_settled` の判定そのものは draining を active に数えていない。誤検出の実体は **「読めない」を「二重 active」と
  決めつけたこと**と、打ち切りが「新を止める」一択だったこと。

## 直したこと

- `scripts/selfdeploy/lib.sh`: 生きている active の数え方を celeris 側の `is_live_active` と揃えた（役割 active・`drained_at`
  無し・pid が `/proc` に居る。draining・終了済み・死んだプロセスは数えない。pid 不明は生きている側）。判定は一語
  （`settled` / `two_active` / `new_not_active` / `no_active` / `unreadable`）。API が読めないときは DB を `mode=ro` で読む代替。
- `scripts/selfdeploy/promote.sh`: `/api/v1/releases` を `api.token` 付きで読む。60 秒待って揃わないとき、新を止めるのは
  `two_active` のときだけ。読めない・判断できないときは新を残して exit 1（人に知らせる）。新を止めた後は health を見て、誰も
  active を答えなければ新を起こし直す（`ensure_an_active_remains`）。文言は読めた事実に合わせ、`daemon_instances (api|db): …` の
  要約を添える。
- `crates/task-api/src/releases.rs`: 「トークン不要」と書いていた注釈を実装に合わせた（コードの変更は無し）。
- `docs/ops/single-active-handoff.md`: `curl` に Bearer、DB の読み取り、打ち切り後の見方。
- 試験が本番の `paths.env`（`CELERIS_BACKUPS_DIR` / `CELERIS_LOGS_DIR`）を環境から継いで、偽 sha `bbbbbbbbbbbb` の log と
  backup（8 byte の `fake-db`）を本番の `logs/` `backups/` に残していたのを直した。残っていた 17 log と 15 backup（全て偽 sha・
  `fake-db` の中身。本物の昇格 log 022147 / 024048 / 034449 / 034618 は残した）を消した。

## 証拠コマンドと結果

| 条件 | コマンド | 結果 |
| --- | --- | --- |
| 0 再現（修正前の HEAD de69d563 の scripts に同じ試験） | `SD_UNDER_TEST=<git show HEAD の promote.sh/lib.sh/web-follow.sh> SD_TEST_CASES=A bash scripts/selfdeploy/tests/promote_live_abort.sh` | FAILED。本番と同じ 3 行（`still not settled after 60s (another instance is active)` → `stopping celeris@bbbbbbbbbbbb` → `two active instances`）、新が止まり `/api/v1/releases was read without the token` |
| 0 修正後（A〜G の 7 ケース） | `bash scripts/selfdeploy/tests/promote_live_abort.sh` | exit 0、`promote_live_abort: all ok`（A: token 付きで読んで handoff done、F: 死んだ pid を数えない、G: drained_at を数えない） |
| 0 判定の単体 | `bash scripts/selfdeploy/tests/promote_handoff_settled.sh` | exit 0、20 件 ok（旧 6 件 + 一語の判定 11 件 + 要約 + 一時 DB の読み取り 2 件） |
| 1 安全側（C 読めない → 新を残す、D 二重 active → 新を止め旧が残る、E 止めた後 active 0 → 新を起こし直す） | 上の `promote_live_abort.sh` の C / D / E | ok（C: `stop celeris@new` 0 回・`live handoff not confirmed … was left running`・`promote_failed.json` あり、D: stop 1 回・start 1 回・`two active instances (the old celeris was still active`、E: stop 1 回・start 2 回・`starting celeris@… again`・`rerun promote.sh`） |
| 2 既存の marker 試験 | `sh scripts/selfdeploy/tests/promote_authorization_marker.sh` | exit 0、all ok |
| 2 文書の配置・リンク・ADR 番号 | `bash scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` / `bash scripts/dev/check-doc-links.sh` / `bash scripts/dev/check-adr-numbers.sh` | 全部 exit 0（`check-doc-layout: ok` / `check-doc-links: ok` / `check-adr-numbers: ok (136 files)`） |
| 2 フォーマット | `cargo fmt --all -- --check` | exit 0 |
| 2 全体（release gate） | `bash scripts/dev/test-parallel.sh` | exit 0。nextest `Summary [64.497s] 3934 tests run: 3934 passed, 12 skipped`、doc-test 0 failed、`CELERIS_TEST_SUMMARY passed=3934 failed=0`（log は run の artifacts `test-parallel.log`） |
| 2 clippy | `cargo clippy --workspace -- -D warnings` | exit 0、warning 0（log は run の artifacts `clippy.log`） |
| 本番 dirs への漏れ | `ls /local/celeris/state/logs/ \| grep promote-20261005` | 試験後も本物の昇格 log 4 本だけ（偽 sha の log・backup は無し） |

## 未解決事項

- `jq` しか無い環境の `sd_instances_rows` の jq 分岐は未検証（この host に jq が無い。本番は python3）。
- 本番での確認は次の live 昇格（この版が `current` になった後のさらに次）で行う。昇格後は
  [docs/ops/single-active-handoff.md](../../docs/ops/single-active-handoff.md) の手順で `daemon_instances` を読む。
- 03:44 の停止で打ち切られた run 2 件（`01M452GY6YKYAR06VTBKX7Y1H3` / `01M451TZP8C6XMV3WBDT4TETY8`）の再実行は daemon の retry に任せた（この task では触っていない）。

## 提案

- `promote.sh` の `/api/v1/health` 以外の読み取り（`sd_http_get` / `sd_http_status`）に、`$SD_API_TOKEN_FILE` を既定で付ける包み
  （`sd_api_get`）を作り、トークン無しの呼び出しを shellcheck 的に見つけられるようにする。
- `GET /api/v1/releases` の `instances[]` に daemon が判定した `alive`（pid の生死）を載せれば、shell 側の `/proc` 判定が要らなくなる
  （schema 変更を伴うので別 task）。
