---
title: 健全性v2 R2 unit adr — ADR worker-capability-ports と計測台本 measure.py
tasks: [01M6TYVJT0Z1CBGNK8VTNK8ZJH]
status: done
updated: 2026-11-10
completed: 2026-11-10
---
# 健全性v2 R2 unit adr — ADR と計測台本

完了日: 2026-11-10

task 01M6TYVJT0Z1CBGNK8VTNK8ZJH の WorkUnit adr の記録。task のまとめは [R2 の進捗](../2026-11-10-health-v2-r2.md)。基準は `$CELERIS_WU_BASE`。Rust のコードは変えていない。

## 成果
- [ADR 2026-11-10 worker の能力の分離](../../adr/2026-11-10-worker-capability-ports.md)（状態は提案）。決めたこと:
  - D1: `EventSink` に残す通知・観測の 13 method。`notify.rs`（人への通知の重複排除の台帳）とは意味が違うので統合しない。
  - D2: Browser の能力の trait 2 つ（既定実装なし、method 名は今のまま）。`BrowserAuthority`（`browser_policy.rs`、承認と saved credential の 6 個）と `BrowserControl`（`browser_live.rs`、lifecycle・auth section・control gate・live の 4 個）。注入は参照の組 `BrowserPorts<'a>`。`StoreSink` が実装し、`run_worker` は `requests_browser` の run にだけ `Some` を渡す。`run_with_executable_attempt` は browser の run で `None` なら、policy・runtime・harness より前に `AdapterError::Other("browser capability not injected")` を返す。
  - D3: `fn configure(&self, &AdapterSettings) -> Configured`（既定は何も受けず、指定を全部 `unsupported` に入れる）。adapter ごとの受ける項目の表と、呼び出し側 8 か所の unsupported の扱い（今の挙動を保つ）の表。
  - D4: write_set の拡張。B0 の R2 の外で変える path と理由、要るときだけ変える path。
  - D5: 撤去対象 6 件と、不変条件ごとの試験（既存の名前と新規の `browser_port_`・`adapter_settings_`）。
- 計測台本 [measure.py](measure.py)（`--json`・`--check-browser-split`・`--check-adapter-settings`）と基準値 [baseline.json](baseline.json)。基準値の表は task の進捗の「計測」節。

## 判断
- Browser の trait を 2 つに分けた: `BrowserAuthority` だけが secret を扱う。live event の出口と control gate だけを要る処理は `&dyn BrowserControl` だけを受け、secret に届かない。束ねる supertrait は作らず、参照の組で渡す。
- `configure` には既定実装を置いた: `WorkerAdapter` の impl は試験に 108 個、`crates/campaign` の `StubWorker`（R2 の範囲の外）にあり、どれも設定を受けない。既定は受けない項目を報告するだけで、許可側に倒れない。
- `--check-browser-split` は objective の 4 sink に `BrowserSink` を足して見る（D5 の撤去対象「`BrowserSink` の browser_* の転送」を検出するため）。`--check-adapter-settings` は試験の adapter の with_* も数える（trait から消せば試験も compile できないので、残りを検出できる）。

## 証拠（提出時の HEAD で実行）
- `python3 agent-docs/progress/2026-11-10-health-v2-r2/measure.py --json` → exit 0（約 2 秒）。
- `python3 agent-docs/progress/2026-11-10-health-v2-r2/measure.py --check-browser-split` → exit 1。`FAIL event_sink.browser_methods`（10 個）・`FAIL browser_traits.BrowserAuthority`・`FAIL browser_traits.BrowserControl`（trait が無い）・`FAIL unrelated_sinks.BrowserSink: browser_fns=7`。
- `python3 agent-docs/progress/2026-11-10-health-v2-r2/measure.py --check-adapter-settings` → exit 1。`FAIL worker_adapter.with_fns`（5 個）・`FAIL adapter_impls_nontest with_*`（10 impl、計 31）・`FAIL adapter_impls_test with_*: 31 fn / 22 impl`・`FAIL browser_specialist_setting_forwards: 5`・`FAIL worker_task_with_env_calls: 4`。
- 台本が目標の状態で exit 0 になることを、`$TMPDIR` に作った小さな crates の木（`EventSink` に browser_* なし、2 trait に既定なし、`configure` だけ）で確かめた: `--check-browser-split` → exit 0。`BrowserControl` に既定実装を 1 つ足すと exit 1（`FAIL browser_traits.BrowserControl.default_impls: 1`）。`#[cfg(test)] mod` の中の試験の adapter の `with_env` は `--check-adapter-settings` で検出した（exit 1）。木は確かめた後に消した。
- 計画の check 2（`measure.py --json` が 0、両 check が非 0）→ exit 0。
- 計画の check 3（ADR に `write_set` と `EventSink` を含み、`git diff --quiet $(git merge-base HEAD main) -- crates/`）→ exit 0。`git diff --stat $CELERIS_WU_BASE -- crates` も空。
- 範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"` の path が ADR と R2 の進捗だけ）と文書の検査 3 本（`check-doc-links.sh`・`check-adr-numbers.sh`・`progress-index.sh --check`）は task の報告に結果を書く。

## 未解決
- 無し。
