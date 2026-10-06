---
tasks: [01M46W97H391DSFW1XJ745W0G9]
---

# Browser web Live View 実機確認

`scripts/dev/browser-web-live-check.sh` と `docs/ops/browser-web-live-check.md` を追加した。台本は未 opt-in 時に何も起動せず exit 2。opt-in 時は試験専用の launcher socket、使い捨て DB の daemon、loopback 試験ページ、web gateway を起動し、browser task・Live View・owner session・control lease・decision wait を確認して `checks.json` を残す。試験 daemon と launcher は別 UID で動かす。

この worker sandbox では user namespace を作れず、root 所有 launcher 設定・別 UID・subuid・Chrome・実測 conformance 台帳も使えないため、実機実行と証跡採取は Fable が [運用手順](../../../docs/ops/browser-web-live-check.md) に従って行う。実行時は範囲外 origin への実ブラウザ遷移が拒否された harness 記録も必須とし、API のみの成功を実機合格と数えない。

## 人の決定 rerun-evidence: 回答 fable-rerun

決定 `rerun-evidence`（ADR-0079 D7）への回答は **`fable-rerun`**（推奨どおり）。Fable が launcher-fix 統合後の tree（`0e167295` 以降、`d8f3e5e6` を祖先に持つ）で `scripts/dev/browser-web-live-check.sh` を再実行し、証跡を [real-check-evidence/rerun-2026-10-06/](real-check-evidence/rerun-2026-10-06/) に commit（`3680b70e`）した。証跡の `versions.txt` には tree HEAD が `0e1672950daccb4268f94eca210d97e825d6171a` と記録され、`git merge-base --is-ancestor d8f3e5e6 0e167295` で launcher-fix（`d8f3e5e6`）が再実行 tree の祖先であることを確認した。

## 経緯: attempts 1〜8 → launcher-fix → 再実行

1. **attempts 1〜8**（tree `8aecea65`、修正前。証跡 [real-check-evidence/attempts/](real-check-evidence/attempts/)）: API 段は attempt 8 で全合格（`api_passed`、org・task・owner・live・control・waits の各状態遷移を確定）。ただし egress 拒否の証跡は無く、shim は全 action を拒否。途中（attempt 5）に action socket path が 107 byte 上限を超えて `isolated_runtime_unavailable`、attempt 6 で takeover の `invalid_phase`（pause なし）、attempt 7 で wait open の origin 拒否（http 拒否）が見つかった。
2. **launcher-fix**（`d8f3e5e6`、統合 `0e167295`）: D4 の action socket path を短い固定長に、D6 の shim config に `policy_sha256` を書き policy に `launch` を含める。詳細は [launcher-fix 進捗](../2026-10-06-browser-launcher-fix.md)。
3. **再実行**（tree `0e167295`、証跡 [real-check-evidence/rerun-2026-10-06/](real-check-evidence/rerun-2026-10-06/)、commit `3680b70e`）: 以下の各段の結果。

## 再実行の各段の結果（tree `0e167295`）

証跡は [rerun-2026-10-06/summary.txt](real-check-evidence/rerun-2026-10-06/summary.txt) と `attempts/4-diag2/checks.json`（最終）を根拠とする。再実行は 4 attempt（`1-as-documented`・`2-diag1-operator-reset-error`・`3-diag1`・`4-diag2`）。

| 段 | 結果 | 根拠 |
|---|---|---|
| 試験用 DB の daemon 起動 | 合格 | 試験 daemon（127.0.0.1:17728、使い捨て DB）起動、`/healthz` 200 |
| launcher 起動・run 開始（D4/D6 修正） | 合格 | `run-browser/config.json` に `policy_sha256` あり、`policy.json` が `launch` を許可、`action_socket=/tmp/celeris-browser-1001/<16hex>.sock`、run RUNNING。D4・D6 修正が実機で効いている |
| loopback 試験ページの navigate | **不合格** | 許可 origin（17730）でも browser への request が無い。sandboxd が Chrome を egress proxy 経由に強制し、task-core の `check_egress` が 127.0.0.1 を `IpLiteral`・loopback 解決 host を `PrivateAddress` として拒否するため、loopback ページは許可 origin でも開けない（shim navigate が session 開始 0.2 s で `browser action failed or was blocked`） |
| Live View proxy（同一 origin 認可） | 合格 | 認証済み GET 200、未認証 GET 401、未認証 WS upgrade 401。raw `live_view_url` は出ない |
| lease 取得と返却 | 合格 | pause → `paused`、takeover → `human_control`、release 200（試験 DB の control_state version 3） |
| 待ち（waits decision/credential） | 合格 | wait open 201（origin `https://real-check.celeris.invalid`）、pending 表示、web deny 200 → `denied`/`approval_denied` |
| 範囲外 origin の egress 拒否（証跡） | **取得できず** | 範囲外 17731 には request 無し（`denied-page.log` に GET 無し）。だが egress は拒否を記録しないため `egress-denied.json` が無く、拒否理由もログに残らない。この原因はコード読み（egress ログ）ではない |
| settings 編集（web 経由） | 合格（URL 修正後） | widen 200（2 origin）、`javascript:alert(1)` 422、restore 200、`GET /api/v1/org` が `[http://127.0.0.1:17730]` を表示 |

**再実行の結果: 不合格（FAIL）**。D4・D6 修正は実機で効いているが、loopback 試験ページが launcher egress の設計（IP literal・private/loopback 拒否）で開けず、許可 origin の navigate 成功・egress 拒否の証跡・screenshot を取れなかった。

## 残っている欠陥（再実行で発見、Fable 記録）

- **R1（台本）**: settings 編集の URL が `web+/api/v1/org/...` で gateway が `/api/v1/v1/` に写して 404。web の設定画面と同じ `web+/api/org/browser-execution/browser-settings` にする。
- **R2（コード・web gateway）**: `web/server/browser-live.js` の `upgrade()` の `reject()` が error listener 無しで socket を閉じ、拒否された upgrade を client が reset すると未処理 `ECONNRESET` で gateway（node）全体が落ちる（attempts/3-diag1/web.log）。`app.js` の host 検査も同じか確かめて直し、試験を足す（認証無しの client が gateway を落とせる＝可用性の欠陥、未認証リモート DoS）。
- **R3（確認の設計 × egress）**: Chrome は egress proxy を通り、loopback の試験ページは許可 origin でも開けない（上「loopback 試験ページの navigate」）。人の決定: 試験専用の egress 許可（既定 off、本番 config では無効、明示した loopback origin の完全一致だけ、有効時は起動ログと run 記録に出す）を足す。SSRF 防御の既定動作は変えない。ADR に付記。台本はこれで許可 origin の navigate 成功（screenshot）と許可外の拒否（egress 拒否の記録）を取る。egress が拒否を記録しない点も、拒否理由を記録するよう直す。
- **R4（台本）**: cleanup が `launcher.sock`・`owner.sock` を残し同じ dir の再実行が壊れる（`EADDRINUSE`）。
- **カバーの欠け**: 別 login session の Live View 拒否、lease 保持時の入力転送の成功が台本に無い。

## 次

- R1〜R4 を直す（台本 + web gateway R2 + egress 試験許可 R3・拒否記録）。
- 修正後に Fable が 3 回目の再実行を行い、許可 origin の navigate 成功（screenshot）・範囲外 origin の egress 拒否記録を証跡として commit する。
- 本 run（`rerun-record`）は記録のみで、上記のコード・台本修正は行わない（別 WU の範囲）。
