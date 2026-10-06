# chat-run / launch: queued 入力を CoS worker run として起動する

---
tasks: [01M47J2YNMZ14NQ4AXTNA355AA]
status: done
completed: 2026-10-06
---

## 実装

- `dispatcher/cos_chat/launch.rs` を tick の後段に接続。thread の durable queue を走査し、`chat_run_claim_next` の原子的な claim で thread ごとに 1 run を起こす。claim 後の準備失敗は理由を `run.reason` と status に残して終端にする。
- daemon の `[cos]` を `resolve_cos_provider` で解決し、harness・source・provider・account・tier・model・budget・API URL を渡す。プールの account は現役 session の account を優先し、使えないときは既存の least-loaded 評価で選び直す。固定 account の枠・ログイン・quota が不足すれば claim せず待つ。
- ADR-0089 の `max_cos_runs` は新 chat run と従来の CoS 対話 run の合算で数える。`max_cos_runs=0` は通常の全体枠・provider 枠・account 枠を使う。account の +1 は内部の検証済み chat 起動経路に限る。
- 一時 Task と `CosChatContext` を組み、要約・未要約履歴の範囲、read-only stage 済み添付の manifest、session handle と session_mode、2 つの CoS skill mount を request に渡す。必須 skill が KB に無ければ理由付きで起動を止める。run credential は store の生 secret に API の bearer prefix を付け、アダプタの env にだけ渡す。終端は `ChatRunSink` を通して `chat_events` に書き、credential を失効させる。`result.actions` は sink がエラーカードにする。
- 後段の `control` と `rollover` が実装するフックを launch に置いた。停止・割り込み・再起動回収・resume 拒否の再試行は後段の WorkUnit が実装する。

## 検証

- `cargo test -p task-dispatch cos_chat_run_launch_ -q`：10 件成功。偽 harness と一時 SQLite で、進行イベント・終端・session_mode・credential env と失効・request/log 非露出、2 thread の同 tick 起動、FIFO、容量上限、固定 account の +1 と待機、sticky account の切替、理由付き unavailable、image manifest を確認。
- `cargo check -p task-dispatch -p celeris`：起動配線の型検査。
- `cargo clippy --workspace --all-targets -- -D warnings`：成功。
- `CELERIS_TEST_JOBS=2 bash scripts/dev/test-parallel.sh`：4115 件成功、失敗 0 件、13 件 ignored。
- `cargo fmt --all -- --check`、`sh scripts/dev/check-doc-links.sh`、`sh scripts/dev/check-adr-numbers.sh`、`git diff --check`：成功。
