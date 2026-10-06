---
title: 実機確認 5 回（rerun5）の記録と 1〜4 回失敗の要約
tasks: [01M46W97H391DSFW1XJ745W0G9]
status: done
updated: 2026-10-06
completed: 2026-10-06
---

# 実機確認 5 回（rerun5）の記録と 1〜4 回失敗の要約

cdp-fix（F1・F4・台本 F2/F3）の統合後、Fable が 5 回目の実機確認を行い、証跡
[real-check-evidence/rerun5-2026-10-06/](real-check-evidence/rerun5-2026-10-06/)（親 branch
`celeris/01M46W97H391DSFW1XJ745W0G9` の commit `15c23817`、tested sha `f5b9dc4e` = cdp-fix 統合後の
tree、binary・script・doc の sha256 は `versions.txt`）を commit した。**Result: PASS**（コード・台本・
手順書は未パッチ）。本 WU（`record6`）は証跡を WU branch に取り込み（merge `0b582b8d`）、1〜4 回の
失敗と各欠陥、5 回目の結果をこの進捗と ADR 付記に記録する。

## 1〜4 回の失敗と各欠陥（要約）

| 回 | tree | 証跡 commit | 結果 | 主な欠陥 |
|---|---|---|---|---|
| 1（attempts 1〜8） | `8aecea65` | `1aac7a6d` | FAIL | shim は全 action を拒否（D6: shim policy に `launch` 無、`policy_sha256` 欠落）、action socket path が 107 byte 超過で `isolated_runtime_unavailable`（D4）、takeover の `invalid_phase`（pause なし）、wait open の http origin 拒否、egress 拒否の証跡なし |
| 再実行（rerun 2） | `0e167295`（launcher-fix `d8f3e5e6` 統合後） | `3680b70e` | FAIL | D4/D6 は実機で効く。が loopback 試験ページが launcher egress の設計（`check_egress` が IP literal・private/loopback を拒否）で許可 origin でも開けず、egress 拒否証跡なし。R1（台本 settings URL が 404）・R2（gateway が拒否 upgrade の RST で未処理 ECONNRESET 落ち、未認証リモート DoS）・R3（試験専用 egress 許可と拒否記録の欠落）・R4（cleanup が socket を残す）と別 session 拒否・lease 入力転送のカバー欠け |
| 3 回目（rerun 3） | `d750b121` | `c2696fee` | FAIL | R1・R2・R4 は直っていた。R5（launcher の session `policy.json` に `launch` が無く agent-browser が CDP attach できず全 action `Action launch denied by policy`。2 回目の真因もこれ）・R6（egress が CONNECT のみを受け、http:// の絶対 URI GET を malformed として落とす）・R7（拒否記録が daemon の session id で launcher の session dir を探し当たらない）・R8（許可外 origin を wrapper 経由で開くと wrapper が先に止めて egress 記録が出ない） |
| 4 回目（rerun 4） | `bee1c2e4`（R5〜R8 修正後） | `884c7baf` | FAIL | R5〜R8 は直っていた（Chrome は許可ページに到達）。F1（shared CDP relay が request/response のみで、agent idle 中に Chrome の `Page.loadEventFired` を転送せず、`open` が全件 25 s タイムアウト）・F2（拒否収集が pause/release/decision deny 後に走るため launcher session が死んでいる）・F3（`kind=private_address` を要求していたが IP literal は `ip_literal` になる）・F4（bwrap `HOME=/session` のため `.cache`/`.config` が subuid 所有で残り session dir 削除に `Permission denied`）。原因調査 commit `4cc2b535`（`f1-diagnosis-2026-10-06/`） |

欠陥の修正と検証記録: [2026-10-06-browser-launcher-fix.md](../2026-10-06-browser-launcher-fix.md)
（D 群）、[2026-10-06-browser-r5-r8-fix.md](../2026-10-06-browser-r5-r8-fix.md)（R5〜R8・session dir、
R1〜R4 は本 task の r2-gateway / r3-egress / script-fix2）、[2026-10-06-browser-cdp-fix.md](../2026-10-06-browser-cdp-fix.md)
（F1/F4/F2/F3）。各 ADR（[shared CDP idle event pump](../../adr/2026-10-06-shared-cdp-idle-event-pump.md)、
[HTTP forward GET](../../adr/2026-10-06-egress-http-forward-get.md)、試験専用 egress 許可の付記）
も参照。

## 5 回（rerun 5、2026-10-06）の各段の結果

証跡は [rerun5-2026-10-06/summary.txt](real-check-evidence/rerun5-2026-10-06/summary.txt) を根拠とする。
3 attempt（`1-as-documented-harness-v1` は操作者 harness v1 の解析 bug で無効、`2-as-documented`、
`3-diag1-playwright-browsers-path` = 判定 attempt）。試験 daemon（127.0.0.1:17728、使い捨て DB）、
別 UID の試験 launcher（`test_loopback_allow = ["127.0.0.1:17730"]` 1 件のみ・起動拒否の fail-closed
検査を台本が確認）、loopback ページ 17730/17731、web gateway 17729。本番の daemon/web/launcher・
`~/.config/celeris`・DB は触っていない。外部ネットワークは使っていない。

| 段 | 結果 | 根拠 |
|---|---|---|
| run 一覧・Live View 認可 | 合格 | RUNNING run、`live_path=/browser/live/{task}/{run}`、raw `live_view_url` なし。owner GET 200、未認証 GET/WS 401、別ログイン session GET/WS 403 |
| gateway の RST 耐性（R2） | 合格 | 拒否 upgrade の socket を急閉しても gateway は停止せず、その後の live/control/release/decision が 200。web.log に ECONNRESET・クラッシュ無し |
| agent open/snapshot/click（F1） | 合格（F1 修正済み） | open `http://127.0.0.1:17730/` が 0.45 s で exit 0（修正前は全件 25 s タイムアウト）、snapshot 0.10 s（`- link "forbidden" [ref=e1]`）、click @e1 が 0.10 s。次の snapshot に Chrome の拒否ページ |
| 許可 loopback 遷移の screenshot | 合格 | page.log に run の GET / 200、allowed-page.png に `#inside` と forbidden リンク |
| egress 拒否の証跡（F2/F3） | 合格 | pause 前の launcher session 記録（R7 の `Started` id 探索）から `egress-denied.json` = `{"host":"127.0.0.1","kind":"ip_literal","port":17731,...}`。不許可ページ 17731 に GET 無し、許可 port 17730 に記録無し |
| lease 取得/返却・入力の転送 | 合格 | agent_running → pause → paused → takeover → human_control → release 200。lease 保持時の input_mouse だけが upstream fixture に届く（1 frame）、無いときは `input_denied/lease_required` で 0 frame（agent 実行中・release 後の両方） |
| waits decision | 合格 | wait open 201（origin `https://real-check.celeris.invalid`）、pending 表示、web deny 200 → `approval_denied` で run 終了（deny の後効として期待どおり） |
| settings 編集（web 経由） | 合格 | PATCH widen 200（2 origin）、`javascript:alert(1)` 422、restore 200、daemon の `GET /api/v1/org` が `[http://127.0.0.1:17730]` |
| cleanup・session dir（F4） | 合格（F4 修正済み） | 停止後 `launcher-state/` に session dir 無し（registry/sessions/supervisor のみ）、launcher.log に Permission denied 無し。port 17728-17732 のプロセス残留無し |

**5 回目の結果: PASS（コード・台本・手順書とも未パッチ）**。

## 軽微な残り（コード欠陥は無し）

- **手順書（HOME 上書き時の PLAYWRIGHT_BROWSERS_PATH）**: 手順書は HOME の上書きを要求しないが、
  操作者が daemon を `~/.config/celeris` から隔離するため HOME を試験ディレクトリへ変えると台本の
  Playwright 段が `Executable doesn't exist` で落ちる（attempt 2）。`PLAYWRIGHT_BROWSERS_PATH` を
  実際のキャッシュへ向けるか HOME を daemon プロセスに限定する（attempt 3 が後者相当で exit 0）。
  注記は [運用手順](../../../docs/ops/browser-web-live-check.md)「実行」に足した。
- **task 文言の『read #inside』**: shim の snapshot は interactive-only（`snapshot -i`）のため段落に
  `@e` 参照が付かず、wrapper 経由では読めない（attempt 3 の harness は `has_inside=false` を記録）。
  台本はこの項目は検査せず、Playwright が `#inside == "inside"` を別途確認しているので無害。文言を
  落とすか Playwright 依存の検査にする余地がある。
- Chrome が session ごとに 4 回 `category=other-startup-error` を stderr に出す（全 run で見られる、
  機能への影響は無し）。

## 文書検査（本 WU、統合 merge `0b582b8d` 後）

- `sh scripts/dev/progress-index.sh --check` — 本 WU が触れた file（record6.md・real-check.md・親進捗・
  ADR・ops doc）に関する指摘は無し。main 由来の他 task の front matter 不備（web-artifact-viewer・
  web-tabbar-first-screen 関連）は本 WU の範囲外のため既知の指摘のみ除外して判定（exit 0）。
- `sh scripts/dev/check-adr-numbers.sh` — exit 0。
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` — exit 0。
- `sh scripts/dev/check-doc-links.sh` — exit 0。
