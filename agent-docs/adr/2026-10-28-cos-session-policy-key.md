# ADR 2026-10-28: CoS の継続 session を方針の版で切り替え、操作の方針の版は daemon が決める

- 状態: 実装済み（2026-10-28）
- tasks: [01M60JC1R91HP7Y1B9AKA07B7V]
- 関係: ADR 2026-10-05-cos-chat-home（D2 session の照合 key）、ADR 2026-10-08-cos-chat-prompt-cache（T5 skill の mount）、
  ADR 2026-10-09-cos-operations-all-mutations（`cos_operations` の監査）、ADR 2026-10-06-cos-inbox-triage（`[cos.triage] policy_version`）

## 問題

2026-10-28 03:5x に本番の `[cos.triage] policy_version` を "4" にし（reload 済み）、KB の `cos-inbox-triage` を v4 に差し替えた。
その後 05:20:51Z に BenchFS の CoS スレッドが記録した `inbox.escalate` の `cos_operations.policy_version` は "2" だった。

- CoS chat の session は thread ごとに resume される。照合 key（D2）は `(thread_id, harness, provider, llm_source, account_id,
  cwd, model)` で、mount する skill の中身も方針の版も含まない。skill を差し替えても次の run は古い session を resume し、
  前に読んだ skill の本文（版 "2"）を文脈に持ったまま判断できる。
- `cos_operations.policy_version` は run が body に書いた値をそのまま記録していた。daemon は run に方針の版を渡しておらず、
  run は skill の本文・Core の例（"5" の固定値）・`celerisctl` の既定（環境変数が無ければ "1"）のどれかを書いていた。
  記録の版は実際に効いていた方針を示さない。

## 決定

### D1. session の照合 key に方針の key を足す

照合 key に `policy_key` を足す。値は daemon が run ごとに組む文字列
`policy_version=<[cos.triage] policy_version>;<skill 名>=<中身の sha256>;…`（skill 名の昇順）。

- skill の中身の hash は KB の `skills/<name>/` の下の通常 file（相対 path・byte 長・中身。symlink と特殊 file は無視）から決める。
  worker の配送の hash（`task_worker::skills`）と同じ規則で、`.gitignore` の差し替えは含めない。
- mount する skill の集合（`cos-operator` と、受信箱の run の `cos-inbox-triage`）が変わっても key は変わる。
- 保存は `node_sessions.policy_key`（migration 0100）。既存の行は NULL なので、配備後の最初の run はどの thread も fresh になる。
- 判断の順は D2 のまま「key 変更 → …」で、方針の key だけが違うときの理由は `policy_changed`（`chat_runs` の `session_reason`）。
  他の欄が違えば従来どおり `key_changed`。fresh は summary＋未要約履歴で始める（D2 の fresh と同じ）。

### D2. 操作の方針の版は daemon が run に渡し、記録・検査する

- daemon は run の起動時に `[cos.triage] policy_version` と D1 の `policy_key` を `chat_runs.resolved_config_json` の
  `policy_version`・`policy_key` に記録する（run credential の発行より前）。run には worker protocol の
  `cos_chat.policy_version` と環境変数 `CELERIS_COS_POLICY_VERSION` で渡す。Core と skill の例は固定の数字をやめ、この値を使わせる。
- `POST /api/v1/cos/operations` と `POST /api/v1/cos/inbox/{i}/resolve` は body の `policy_version` を、その run に記録した版と
  比べる。違えば 422 `cos_policy_version_mismatch`（detail に両方の値）で、操作は実行しない。run に記録が無い（記録前の run）
  ときも 422 にする。記録する `cos_operations.policy_version` は daemon の版である（一致したときだけ記録されるので同じ値）。
- `celerisctl` の CoS 経路は `CELERIS_COS_POLICY_VERSION` が無ければ失敗する（既定 "1" を消す）。

**却下した案。** (a) body の `policy_version` を廃して daemon が埋める: 古い文脈の run が自分の版を誤って信じていることを
検出できない。食い違いを 422 で返すと、run は読み直す必要を知る。(b) policy_version だけを key に入れる: skill の本文だけを
直して版を上げ忘れた場合に resume が続く。中身の hash も入れる。

## 結果

- skill か版を変えた後の最初の CoS run は fresh になり、それ以降の操作の記録は新しい版になる。古い版を書いた run は 422 を受ける。
- 本番の KB の `cos-operator`・`cos-inbox-triage` は Core と同じく `policy_version` を run の値から取る書き方に直す必要がある
  （repo の `config/skills/` は直した。本番 KB への取り込みは人が行う）。
