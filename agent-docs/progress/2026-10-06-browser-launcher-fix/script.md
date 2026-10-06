---
title: script — 実機確認の台本と手順書の欠陥（D1〜D3・D5・D7）を直し確認を足す
tasks: [01M47QXZR0QMCYZM9KAZC81BCD]
status: done
updated: 2026-10-06
completed: 2026-10-06
---

# script — 実機確認の台本と手順書の修正

WorkUnit `script`。`scripts/dev/browser-web-live-check.sh` と `docs/ops/browser-web-live-check.md` を、Fable の実機確認（attempts 1〜8、[証跡](../2026-10-05-browser-web-live-view/real-check-evidence/attempts/exit-codes.tsv)）で見つかった欠陥に合わせて直した。crates/ は変えていない。台本は launcher が要るので実行していない（実機の再確認は Fable）。

## 直したこと

- D1: launcher ラッパーで `os.dup2(fd, 3, inheritable=True)` の後に `os.set_inheritable(3, True)`。socket がすでに fd 3 だと dup2 は何もせず O_CLOEXEC が残る（attempt 1）。
- D2: task 作成後に `PUT /api/v1/tasks/{id}/browser/policy`（task-api `browser.rs` の routes、本体 `BrowserTaskPolicy`）を送り、`GET` で読み戻して `checks.json` に残す（attempt 3 の `browser_policy_required`）。
- D3: 既定を `/var/tmp` の下に。証跡は `/var/tmp/cb-<UID>`、launcher config は `/var/tmp/celeris-browser-config-<UID>/launcher.toml`。ほかの証跡ファイルの既定はこの下。
  - 次のどれかが `/tmp` の下（resolve 後）なら開始前に拒否する: 証跡、launcher config とその親、daemon config、db・workspace、launcher の socket・state_dir・session_root、bwrap 等の実体（attempt 4）。
- D5: takeover の前に `pause` を送る。`pausing` なら `paused` になるまで待ち、takeover 後は `human_control` を要求する（attempt 6）。
- D7: decision wait の origin は wait 専用の `DECISION_ORIGIN`（既定 `https://real-check.celeris.invalid`、`CELERIS_BROWSER_TEST_DECISION_ORIGIN` で変えられる）。
  - 台本が正規形を検査する（task-core `valid_exact_origin` と `normalize_https_origin` と同じ: https、path なし、:443 なし）。
  - http の loopback ページは egress の確認にだけ使う（attempt 7 の 422 origin）。

## 足した確認

- 設定の編集: web gateway 経由で `PATCH /api/v1/org/browser-execution/browser-settings` を 3 回送る。
  - 広げる → 200 で反映。不正 origin → 422。戻す → 200。最後に `GET /api/v1/org` の保存値を確かめる。
- 未認証の Live View: cookie なしの client で、`live_path` の GET と stream の WebSocket upgrade の両方が 401 になる。
- lease 無しの入力: owner session で stream に upgrade し、`input_mouse` を送る。agent 実行中と lease 返却後の 2 回。
  - gateway が `input_denied`/`lease_required` を返すことを確かめる。
  - upstream に届かないことを確かめるため、Live View の upstream を新しい fixture（`LIVE_PORT` 既定 17732、stdlib の Python、受けた frame を `live-upstream-frames.jsonl` に記録）に替えた。
  - 以前は upstream が試験ページ（python http.server）で、WebSocket を受けられなかった。

## 証拠

- `bash -n scripts/dev/browser-web-live-check.sh` → exit 0
- 台本に埋め込んだ Python 9 塊を `python3 -m py_compile` → 全部 ok
- Live View fixture だけを一時 port で起動し、WS upgrade（101）、input frame の記録、`/` の HTML 応答を確かめた
- `git diff --quiet $CELERIS_WU_BASE HEAD -- crates/` → exit 0

## 未解決事項

- 台本の全体は未実行。launcher・userns が要るので、実機の再確認は Fable が行う。
- egress の拒否の証拠（`egress-denied.json`）は shim の policy（D6、葉 run-path）と egress allow（葉 egress）が直るまで出ない見込み（attempt 8）。

## 提案

- なし
