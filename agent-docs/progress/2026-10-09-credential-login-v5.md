---
title: "credential login v5: username 欄の一括注入とログイン後の読み取り（manaba 課題監視）"
tasks: [01M4GYJ3XGJNWZQDF35F1MDE0H]
status: done
updated: 2026-10-09
---

# credential login v5

- 完了日: 2026-10-09（実装・試験。本番適用と host 実証は人の手順待ち）
- branch: `ops/credential-login-v5`（main `2df8feca` + ADR 草案 `4aa1f1fa` を cherry-pick）。main への merge・push はしていない
- 決定: [ADR 2026-10-09 credential username / post-login](../adr/2026-10-09-browser-credential-username-and-post-login-read.md)
  （状態を「承認 2026-10-09」にし、Q1〜Q9 の回答 A1〜A9、D2-6（screenshot・download・click）、実装時の付記を記録）
- 運用手順: [docs/ops/browser-credential-login-v5.md](../../docs/ops/browser-credential-login-v5.md)

## 入れたもの

1. **site policy**: `username_selector`・`post_login {read_origins, actions}`（migration 0064、schema 64）。API（`PUT
   /browser/site-policies/{id}`、CoS op）・config seed・`CredentialPolicy`・vault 記録・`TrustedLogin`・describe・grant の照合・
   承認の固定。形式検証は `validate_trusted_login_full`（selector の文法、username ≠ password、read_origins は 1〜8 個の正規形 exact
   HTTPS origin で IdP を含まない、actions は 1 個以上で重複なし）。
2. **credentiald**: 注入要求 v2 の `username {selector, object_id, input_type}`。照合 4b で password と username の selector を
   lease の policy 断面と byte 一致（食い違い・欠落・余分は `selector_mismatch`、lease 未消費）。固定 `INJECT_PAIR_FUNCTION` で
   1 lease・1 resolve・1 sink frame に 2 欄（同じ document・接続済み・origin/depth・型・別要素を検査してから代入）。guard は password
   だけ（A1）。audit に username selector（値ではない）。
3. **controller（`CdpController`、両 runtime 共通）**: username 要素の解決（同じ top document、text/email、password と別の node）、
   pair の sink frame 照合（関数・objectId・引数の形）。区間の閉じ方（login document を離れた・top が read_origins・password 欄 0・
   注入値消去、15 秒で `post_login_unconfirmed`）。区間後の agent command の検査（前後 2 回、read_origins 外は
   `observation_origin_denied`、password 欄は `password_field_present`、RedisplayGuard）。read_origins 外の download の即時取消と、
   取消が間に合わなければ session の観測停止。controller だけの session の event は agent に流さない。CDP の受信上限 64 MiB。
4. **relay**: cookie・storage・応答本文などの CDP を拒否（`denied_method`）、event から Cookie / Set-Cookie / Authorization・
   header text・cookie 一覧を削る（`sanitize_agent_event`）。
5. **daemon 経路**: 区間が閉じたら store の auth section を外し、shim の policy.json / config.json と action server の allow を
   post-login の集合（task ∩ grant ∩ post_login.actions）へ。worker event の guard を外す。takeover / renew は control 状態の
   `credential_used`（session の終わりまで）で拒否。prompt に読める origin と操作を書く。
6. **launcher protocol 5**: `authenticate` に `username_selector`・`post_login`、応答に `observation (held|resumed)`。daemon は
   承認消費の前に必要な版（username / post_login を使えば 5）を確かめ、足りなければ「rebuild and replace celeris-browser-launcher
   (protocol 5 required)」で拒否。launcher は login 後の verb を `AfterLogin` で絞る。v4 の形の要求には v4 の形で答える。
7. **web**: site policy 編集に username selector と post_login（読み取り先 origin・操作・LLM に渡ることの確認）、承認画面に
   固定したログイン（login URL・入れる 2 欄・ログイン後の読み取り）。
8. **doctor**: site policy ごとに username_selector・post_login を表示、launcher の版の文言。
9. **文書**: 運用手順（新規）、既存の launcher 解放手順からの参照、architecture-map、API 文書（gui/docs）。

## 証拠

受け入れ条件ごと（コマンドはすべて worktree `/local/celeris/data/scratch/clv5-wt`、`CARGO_TARGET_DIR=/local/celeris/data/scratch/clv5-target`）。

- 全試験: `bash scripts/dev/test-parallel.sh` → exit 0、`passed 5016 / failed 0 / ignored 14`（175 binaries、nextest 174.8 s、
  doctest 9.8 s）。1 回目（load 70）は `e2e::api_scenarios phase3_auth_section_refuses_takeover_and_renew_until_left` だけ失敗 —
  区間を出た後の takeover が通る前提の試験で、D2-3 の「credential session の印で takeover を拒否」に合わせて
  `..._for_the_credential_session` に直した。2 回目（load 15）で 0 failed。
- `cargo clippy --workspace -- -D warnings` → exit 0（`--all-targets` も exit 0）
- `cargo fmt --all -- --check` → exit 0
- `CELERIS_USERNS_TESTS=1 CELERIS_ISOLATION_TESTS=require cargo nextest run -p task-api --test browser_h3_injection` → 3 passed
- `sh crates/task-worker/scripts/launcher-admission-evidence.sh --credential <log>` → `EXIT: 0`（credential-unit 34 passed、
  credential-broker 5 passed。前回 30 → 新しい launcher_credential_ 試験 4 本を含む）
- web（`web/`、`pnpm install --frozen-lockfile --offline` 後）: `pnpm test` → vitest 87 files / 643 passed、server の node test 78 passed、
  `pnpm typecheck` exit 0、`pnpm lint` 0 errors（4 warnings は main と同じ）、`pnpm build` → `pnpm check:secrets` OK、
  `check:boundaries` / `check:parity` exit 0、`WEB_E2E_SCOPE=functional pnpm exec playwright test e2e/browser/settings.spec.ts`
  → 3 passed（44 px の操作対象・横 scroll 無し・a11y を含む）
- 受け入れ試験（実 Chromium + loopback HTTPS の IdP・LMS・他 origin fixture `browser_post_login_fixture.py`。外部ネットワーク・
  CPU 負荷なし。待ちは 20〜50 ms の poll と期限）:
  - `launcher_credential_post_login_pair_login_then_reads_only_the_lms`（実 `LauncherServer` + FD 渡しの実 socket + 実 credentiald
    + 実 Chromium）: login form は origin 直下に無く username と password が同じ頁 → submit → LMS へ。`Recorded(Ok(Resumed))`、
    IdP の受信 = username と password、journal の `consumed` は 1 行、controller は区間を閉じ post-login。LMS で extract（課題名と
    「Signed in as <username>」）・snapshot（AX tree）・screenshot・click（課題詳細へ）・download（handout 完了）が通り、IdP root・
    他 origin・password 欄の頁で `observation_origin_denied` / `password_field_present`、他 origin の download は canceled。
    password と session cookie は agent の応答・event に 0 件、password と username は journal・store 呼び出し・結果に 0 件。
  - `daemon_post_login_pair_login_reads_lms_and_refuses_idp_other_and_password_pages`（daemon 経路の controller と
    `complete_trusted_login` / `await_post_login`、broker は本番の pair 関数を書く試験 sink）: 上と同じ一式に加え、注入要求が v2・
    username 欄付き・password と別 object、区間中は agent command が `auth_section_required`、password を表示する頁は
    `redisplay_detected`、session 切れで IdP form に戻った頁は拒否、controller だけの session の attach が agent に届かない。
  - `daemon_post_login_unconfirmed_keeps_observation_stopped`: IdP がログインを拒む・SP が password 欄の頁に着く・着地 origin が
    read_origins でない、の 3 通りで `post_login_unconfirmed`、区間は開いたまま、agent command は拒否、event は 0。
- 単体試験（主なもの）: credentiald `pair_injection_fills_username_and_password_in_one_frame_and_guards_only_the_password`・
  `pair_injection_selector_and_shape_mismatches_never_consume_the_lease`・`registered_username_selector_and_post_login_pin_the_grant`・
  `username_selector_and_post_login_are_validated_like_the_login_fields`、task-core の round trip・schema 64・control の印、
  task-worker `launcher_credential_sink_accepts_only_the_fixed_pair_injection_frame`・`post_login_gate_skips_only_plumbing_methods`・
  `post_login_downloads_outside_read_origins_are_cancelled_and_private_events_dropped`・
  `post_login_relay_denies_cookie_storage_body_and_strips_credential_headers`・`post_login_read_is_the_site_opt_in_narrowed_by_the_task_policy`・
  `launcher_credential_v5_*`・`launcher_post_login_verbs_follow_the_login_outcome`・`post_login_prompt_names_origins_and_actions_only_when_resumed`、
  celeris `browser_doctor_reports_username_selector_and_post_login_per_site_policy`。

## 未解決事項

- **launcher runtime の screenshot / download の artifact**: `LauncherExecutor` は両 verb を opaque な失敗で返す（既存の制限）。
  A3/A4 で許したが、本番（launcher runtime）では成果物にならない。controller の CDP 検査は両方を通す。
- **download の取消の競争**: read_origins 外の download は始まった時点で取消を送るが、本文が先に届けば完了しうる。そのときは
  session の観測を止める（fail closed）。download 元は egress（task の許可 domain = manaba と IdP）でも絞られる。
- **agent-browser 0.38.1 との組み合わせ**: 受け入れ試験は agent の CDP を直接使う。agent-browser が検査対象外の一覧に無い
  page-session の method を「読まない操作」に使っていれば、区間後に拒否されうる（host 実証で確かめる）。
- **既存の credential 登録**（`cred-01M4H1PP…`・`cred-01M4H35F…`）は username 欄を持たない site policy の写し。site policy を
  更新した後に登録し直す（手順 §4）。
- 実 host でしか確かめられないこと（手順 §7）: 実 launcher（UID 995）での v5 と Attested admission、IdP の実 form と同意画面、
  manaba の着地 origin、bwrap 内 Chromium の egress。
- gui/（旧 GUI）の生成型 `gui/app/celeris/types.ts` は再生成していない（node_modules が無く、Rust 側の試験は見ない）。

## 提案

- launcher protocol に artifact の受け渡し（launcher が作った screenshot / download を FD で daemon に渡し、daemon が大きさ・型を
  検査して artifact に登録）を足す task。A3/A4 の決定を launcher runtime で実際に使えるようにする。
- download を「取消」ではなく「始めない」にするなら、controller の Fetch interception（response 段で Content-Disposition の
  ある read_origins 外の要求を fail）を検討する。
