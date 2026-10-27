# 外部 operator の消費を評価用 JSONL に保存する

- 日付: 2026-10-27
- 状態: 採用
- tasks: [01M5XXF7Q9FFYE8S18VMNQ34MH]

## 決定

1. `celeris-campaign operator-usage --transcript F --session-map M [--out O]` を追加する。
   本番 DB や内部 run の UsageRole は変えず、human-ops と同様の評価者所有ファイル
   `operator-usage.jsonl`（既定は M の隣）へ外部 operator 専用 schema の行を保存する。
   内部 worker 消費と自動合算しない。trial_key は既存 TrialKey の文字列を評価者が写す。
2. 対応表は JSON 配列。session_id、start/end（RFC3339、半開区間、null は無限）、
   category（prep/observe/intervene/shared）、trial_key を持つ。shared は trial_key=null、
   他は試行鍵必須。重なる区間、不正時刻、未知の欄を拒否する。
   応答の観測区間全体が一つの対応区間に入る場合だけ帰属し、未対応・区間境界を跨ぐ分は
   shared_unattributable / trial_key=null。均等配分・subagent への推測継承はしない。
3. assistant の message.id を単位として dedup_key=op:<sessionId>:<message.id>。
   同じ message の複数 content 行は token を加算せず、各 token 欄の最大観測値を採る
   （stream の累積 snapshot）。model・subagent 識別の矛盾は拒否する。
   start/end は当該 message の最初/最後の transcript timestamp（API 呼び出しの実時間ではない）。
   時刻が無ければ両方 null、帰属は shared_unattributable。usage/各 token 欄が無ければ null。
   isSidechain または agentId で subagent を識別する。sessionId は transcript の値を保持する。
4. 出力は専用 lock の下で既存行と key で照合し、同内容の再取り込みは no-op、異なる内容は
   conflict で拒否する。全入力検証後、一時ファイルから atomic rename で保存する。
   訂正や途中 snapshot の更新は別の出力へ全証跡から再生成し、旧版を証跡として残す。
5. transcript は測定前に保持期限を延長し、終了後 session ごとに subagent を含め証跡へ写す。
   運用手順は docs/ops/operator-usage.md。既に消えた transcript の消費を推定補完しない。
