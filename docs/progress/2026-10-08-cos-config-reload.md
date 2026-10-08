# CoS 起動設定の reload 反映

---
tasks: [01M4EKPXZF7M6QT78Q86GBDCVR]
---

## 実装

`daemon/cos_launch.rs::build_cos_chat_launch` に起動設定の組み立てを集約し、daemon 起動時と `POST /api/v1/reload` の両方から呼ぶ。主経路・fallbacks の provider/account/model 解決、API URL、実行予算、予約閾値、triage を含む。reload では稼働中の DB path と API listener を使う。

dispatcher は新しい run 用の設定だけを更新する。実行中のハンドル、account/provider の使用数、再試行待ち、triage 状態を維持する。run 開始時の設定をスレッドごとに保持し、同じ run の fallback と残り時間計算にはその設定を使う。終了後に保持した設定を除去する。

API の添付 middleware と背景 GC に固定される `[cos.attachments]` 全項目、および `[cos] stream_retention_days` の変更を含む reload は、更新前に HTTP 400 で拒否して再起動が必要と返す。その他の `[cos]` 項目は reload で反映する。

## 検証

試験では一時 SQLite、ループバック HTTP、プロセス内 fake adapter を使用する。実 LLM・外部ネットワーク・本番 DB は使用しない。新しい run の起動はチャネル受信、fallback は worker の join で待ち、負荷や固定 sleep に依存しない。

- HTTP reload 前後の `chat_runs.resolved_config_json` と fake に渡されたモデルを確認。tier だけの変更（旧モデル → 新モデル）、harness、明示モデル、fallback の変更を含む。
- 実行中 run の状態と解決済み設定を保持する。
- 保持日数と添付設定の各項目について HTTP 400・再起動理由・拒否後の旧設定維持を確認する。
- 起動設定の account 解決、triage 全項目、予約閾値、稼働中 DB/API endpoint の維持を確認する。
- fallback 待ちの run を持つ dispatcher を reload しても、旧経路で同じ run を完了し、更新後の disabled が次の run を止めることを確認する。

検査結果:

- `cargo test -p celeris --lib cos_reload -- --nocapture`: 2 passed、exit 0。
- `cargo test -p task-dispatch --lib cos_reload -- --nocapture`: 1 passed、exit 0。
- `cargo build -p celeris -p celerisctl`: exit 0（e2e 用の実行ファイルを事前構築）。
- `bash scripts/dev/test-parallel.sh`: **4,866 passed / 0 failed / 14 ignored**、166 binaries（nextest 156 + doctest 10）、exit 0。nextest 110.0 秒、doctest 9.8 秒。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`、`git diff --check`: exit 0。
- 文書レイアウト検査と変更文書のリンク検査: exit 0。

全体試験は Unix socket のパス長制限を避けるため、`unshare --user --map-current-user --keep-caps --mount` 内で run の `$TMPDIR` を `/tmp` に bind し、`setpriv --bounding-set=-all --inh-caps=-all --ambient-caps=-all` で権限を除去してから `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` を実行する。実際の一時ファイルは run の TMPDIR にあり、host の `/tmp` は変更しない。`CARGO_TARGET_DIR` とコンパイラ設定は渡された値を維持する。

## 適用範囲

コード変更のみ。本番の設定変更、daemon 再起動、デプロイは実施していない。

初回の全体試験は `--map-root-user` により UID が 0 となり、既存 daemon 試験が `/root/.config/celeris` を参照して起動拒否された（4,816 passed / 50 failed / 14 ignored、exit 100）。本体コードを変えず、UID を維持する上記環境で再実行して全件合格した。初回ログは run 成果物 `test-parallel-root-uid.log` に保存。
