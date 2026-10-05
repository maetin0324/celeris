---
title: active は常に 1 つ（ライブ引き継ぎの split brain の修正）
tasks: [01M44Y0BR7HQRFNRWK2T2P2ZXE]
status: done
updated: 2026-10-05
---

# active は常に 1 つ（ライブ引き継ぎの split brain の修正）

完了日: 2026-10-05。設計は [ADR-0040 付記 2026-10-05（D4）](../adr/0040-self-improvement-deploy.md)。
人が昇格後に読み取りだけで確かめる手順は [docs/ops/single-active-handoff.md](../../docs/ops/single-active-handoff.md)。
本番への昇格は未実施（この版が `current` になった次の昇格から効く）。

## 何を直したか

本番（2026-10-04 23:13 UTC〜）で、新 instance が active になった後も旧 instance が `active` のまま約 3 時間 dispatch を続けた。
読んだコードの経路は ADR の「原因」の節のとおり:

1. 起動時の判断が heartbeat の新旧で「生きている active」を決め、古い heartbeat の生きた旧を無視した。掃除も旧の行を消した。
2. 引き継ぎ要求は生きた active の先頭 1 つにしか書かなかった。
3. standby の昇格と active の登録が、判断と書き込みを分けた別の SQL だった。
4. `promote.sh` は新の health が active なら「handoff done」にし、旧の drain を見なかった。

直したこと:

- 生きている active = 役割 `active`・`drained_at` 無し・プロセスが生きている（`is_live_active`）。heartbeat では決めない。
- 起動時の active 登録と standby の昇格は、同じ writer transaction で「他に生きた active が無い」を確かめてから書く（`instance_register_if` / `instance_set_role_if`）。
- standby は生きている全部の active に引き継ぎを要求する。
- 掃除は drained、または（active でなく heartbeat が古い）か（プロセスが死んでいる）の行だけ。生きた古い active の行は消さない。
- 毎 tick の監視: 生きた active が自分より新しい行を持つなら自分が drain。自分より古い生きた active には引き継ぎを要求する。
- `promote.sh`: 新が active になった後、`GET /api/v1/releases` で active が新 1 つだけになるまで 60 秒待つ。揃わなければ新を止めて失敗。

## 証拠コマンドと結果

ログは run の成果物ディレクトリ（`test-parallel.log`・`clippy.log`・`clippy-tests.log`）。

| 条件 | コマンド | 結果 |
| --- | --- | --- |
| 試験の追加と書き換え後の instance 試験 | `cargo test -p celeris --lib instance` | exit 0、28 passed |
| 統合試験（生きた旧の引き継ぎ・SIGKILL 後の引き継ぎ） | `cargo nextest run -p celeris --test instance_handoff` | exit 0、8 passed |
| 再現（修正前の規則を一時的に戻す。変更は元に戻した） | `cargo test -p celeris --lib instance`（heartbeat 窓で生存を決める版） | exit 101。12 件が落ちる（R 試験 `two_actives_never_coexist_when_the_old_one_is_stale_but_alive` を含む） |
| shell の判定 | `bash scripts/selfdeploy/tests/promote_handoff_settled.sh` | exit 0（6 件） |
| shell の marker 試験（偽 curl に `/api/v1/releases` を追加） | `sh scripts/selfdeploy/tests/promote_authorization_marker.sh` | exit 0、all ok |
| 文書の配置 | `bash scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | `check-doc-layout: ok` |
| フォーマット | `cargo fmt --all -- --check` | exit 0 |
| 全体（release gate） | `bash scripts/dev/test-parallel.sh` | exit 0。nextest `3934 passed, 0 failed, 12 skipped`、doc-test `0 failed`（`CELERIS_TEST_SUMMARY` の `passed=3934 failed=0`） |
| clippy（workspace） | `cargo clippy --workspace -- -D warnings` | exit 0 |
| clippy（試験コード） | `cargo clippy -p celeris -p task-core --all-targets -- -D warnings` | exit 0 |

最初の `test-parallel.sh` は 1 件落ちた（`crates/celeris/tests/instance_handoff.rs` の旧 (e)「heartbeat が止まった active を置き換える」。
新しい規則では生きた旧を置き換えないのが正しいので、試験を「生きた子プロセスを SIGKILL して reap してから引き継ぐ」に書き換えた）。書き換え後に全体を流し直して上の結果。

## 未解決事項

- 生きているのに応答しない旧（heartbeat が止まり、プロセスは生きている）は、手放すまで新が standby のまま待つ。
  強制的に奪うと二重 dispatch になるので、人が止める判断をする（WARN が出る）。
- 本番の既存の状態（2 つ目の active を含む行）は、本番の DB と daemon を変えるので自動では掃除しない。
  昇格後に [docs/ops/single-active-handoff.md](../../docs/ops/single-active-handoff.md) の手順で確かめる。
- 2 件の統合開始と ready 戻しの競合（`01M44NN2DCZXV0TZ5FXX5TMN58`・`01M44MZ1GW54XYXXH0EMEEDWFE`）が二重 dispatch の所為だったかは、ログの突き合わせが未了（推定）。

## 提案

- 本番の昇格（この版を `current` にする）は人の承認で 1 回。昇格後、上の手順で active が 1 つだけになったことを確かめる。
- `promote.sh` の 60 秒の待ちが二重 active の検出に使われるので、待ち時間を延ばすなら `[handoff]` の設定に出すことを検討する。
