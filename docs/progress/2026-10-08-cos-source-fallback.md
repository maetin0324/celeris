# CoS の代替経路・利用枠の予約

---
tasks: [01M4E0WA5ZYVT32A4W6E74GQGC]
---

[ADR](../adr/2026-10-08-cos-source-fallback.md) を先に作成し、設定・dispatcher・store を実装した。本番設定・daemon・DB は変更していない。配送後の設定は [運用手順](../ops/cos-chat.md) を参照。

## 実装

- `[[cos.fallbacks]]` を設定順に解決。主経路が quota/cooldown/rejected/未ログインなどで使えない場合と利用上限応答の場合、同じ run ID・入力・出力で次の経路を試す。各経路は一度まで。
- 切替は worker 終了後。旧 credential を失効し、新しい credential を発行。harness/source/provider/account/model が変われば新 session に DB の要約・履歴・添付を渡す。適用済み操作の receipt を渡し、未確定操作は人の確認へ回す。停止・割り込みが優先する。
- 実効経路は既存 run 欄に反映し、Run event とチャットの system message に経路・再試行を記録。返答できなければ理由と再送方法を本文に残す。既存 UI を使用し web/API schema は変更していない。
- `worker_reserve_five_hour`（既定0.90）以上の Claude account は通常 worker の新規選択と sticky 再利用から外す。CoS は ADR-0089 の pool 例外/account +1 を維持し、quota は守る。観測不明・reset 後は予約しない。実行中 worker は停止しないため、残り枠の保証ではない。
- CoS の account 会計を実効 adapter と ID で行う。経路を替えても run の wall-clock の残りだけを渡す。

## 検証

外部 LLM・ネットワークを使わない fake adapter と一時 DB を使用。join と時計注入で同期し、負荷での再現は行わない。

- 初期の `cargo test -p task-dispatch --lib cos_source_fallback`: 6 passed。
- `cargo test -p celeris -p task-core cos_source_fallback`: 設定・credential の試験成功。
- 初期の CoS 回帰試験: 115 passed。追加した履歴試験で fake Claude の session ID を resume 可能と仮定した誤りを修正（実装は正しく fresh にしていた）。
- `cargo check --workspace --tests --keep-going`: exit 0。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `bash scripts/dev/check-doc-links.sh` / `bash scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv`: exit 0。
- 全体検査の初回完走: 4,844 passed / 6 failed / 14 ignored（exit 100）。失敗は長い socket パス4件、mount capability による権限検査1件、TMPDIR を reflink 不可と仮定した既存試験1件。CoS の追加13試験は全て合格。
- 環境調整後の全体検査: 4,849 passed / 1 failed / 14 ignored（exit 100）。既存 `spawned_tasks_run_on_their_interval_and_stop_cleanly` の固定300ms待ちだけが失敗。単独再実行は1 passed（exit 0）。全体を同じ条件で再実行した結果は次のとおり。

- **最終 `bash scripts/dev/test-parallel.sh`: exit 0、4,850 passed / 0 failed / 14 ignored。** nextest 156 binaries + doc-test 10 crates。以下の隔離環境で実行し、TMPDIR の残骸0件。
- `cargo fmt --all --check` / `git diff --check`: exit 0。

全体検査の環境: 渡された TMPDIR は長く、既存 browser/credentiald 試験が Unix socket の `SUN_LEN` に当たったため初回を中断した（exit 130）。短い symlink も添付の symlink 拒否や canonical path 比較に当たるため中断した。最終検査では隔離 user/mount namespace 内だけで元の run TMPDIR を短いパスに bind mount し、device/inode の一致を確認してから実行した。CARGO_TARGET_DIR 等は変更していない。本番サービス・設定・DB は操作していない。

最終検査の起動コマンド（mount に必要な capability は試験開始前に落とす。mount 失敗時には試験を開始しない）:

```sh
unshare --user --map-current-user --keep-caps --mount --propagation private bash -euc '
  mount --bind "$TMPDIR" /tmp
  test "$(stat -Lc %d:%i /tmp)" = "$(stat -Lc %d:%i "$TMPDIR")"
  setpriv --inh-caps=-all --ambient-caps=-all --bounding-set=-all \
    env TMPDIR=/tmp bash scripts/dev/test-parallel.sh
'
```

`/tmp` の mount は隔離 namespace 内だけに存在し、実ファイルは元の run TMPDIR に保存する。最終ログ: run artifacts の `test-parallel-regate.log`、`clippy.log`（途中結果は `test-parallel-final.log` / `test-parallel-final2.log`）。

## 検証に伴う既存試験の修正

`task-worker::scratch::tests::reflink_unavailable_falls_back_to_empty_target` は、TMPDIR が tmpfs/ext 系だという前提を除去した。run の TMPDIR は btrfs に置けるため、既存の `SeedCopyOps.is_shared` へ false を注入して「共有できないときは空の target に戻す」を決定的に検証する。単独試験も合格（commit `dd5ccd2b`）。製品側の scratch 処理は変更していない。

本番での LLM 往復・配送・設定適用は未実施。
