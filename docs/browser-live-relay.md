# Browser Live View relay（Phase 2）

---
tasks: [01M3MZKB3DFYJNBH015MJGQ0BT]
---

GUI server は固定版 agent-browser **0.38.1** の dashboard を、本人の認証済み cookie session にだけ同一 origin で配信する。設定例は `CELERIS_GUI_LIVE_VIEW_UPSTREAM=127.0.0.1:27848`。受け付ける宛先は起動設定の loopback `host:port` だけで、task event や HTTP query に入った URL を宛先には使わない。GUI の password 認証と `CELERIS_GUI_OWNER_SOCKET` の設定、ローカル CLI による owner-session 承認も必要。

HTML の入口は `/browser/live/:taskId/:runId`。上流が絶対 URL を使うため、同じ GUI server の `/_next/static/*` と読み取り API、`/api/session/:port/stream` も relay が受け持つ。各要求で owner grant を確認し、HTML を開いた session に束縛した task/run が active・RUNNING か検査する。完了した認可結果は cache せず、同時に走る検査だけを共有する。他 run への差し替えは 404、別 cookie session は 403、未認証は 401。

転送する HTTP API は GET の `/api/sessions`、`/api/chat/status`、`/api/session/:port/tabs` と `/status` に限る。`/api/exec`、`/api/kill`、chat、session 作成などは 403。WS の client message は frame の受信確認・速度設定（`ack`、`config`）だけを転送し、mouse・keyboard・touch 入力などは捨てる。dashboard の操作ボタンが見えていても操作権限は付かない。

logout・owner grant 再登録は既存 WS を閉じる。cookie の期限と task/run は上流 frame を転送する前にも検査し、通信のない WS は 5 秒間隔でも検査する。dashboard は namespace 内の全 session を表示するので、別 task 経由の認証区間の閲覧も禁止する。未完了 task のどれかに credential 登録・利用の履歴があれば relay 全体を閉じる（archived project の task も含む）。安全性の照会失敗や一覧の取得上限も拒否する。この検査は保守的で、同じ namespace の無関係な公開ページも一時的に開けなくなる。

上流へは固定 loopback の Host/Origin で接続する。0.38.1 はこの接続に bootstrap token を要求しないため、token を取得・永続化・クライアント転送しない。上流の Location、Set-Cookie、その他のヘッダも転送しない。dashboard 起動コマンドの stdout は token を含むのでログへ保存しない。upstream 未設定時だけは本人にも `503 live_view_relay_unavailable`、接続失敗は 502 を返す。

## 再実行

依存は Rust、Node/pnpm、sqlite3、Python 3、agent-browser 0.38.1 と Playwright Chromium。GUI の依存は `cd gui && pnpm install --frozen-lockfile` で用意する。既存の `CARGO_TARGET_DIR` などビルド環境変数をそのまま使う。

```sh
G14_AGENT_BROWSER=/absolute/path/to/agent-browser \
E2E_ARTIFACTS_DIR=/absolute/path/to/empty-evidence-directory \
gui/scripts/browser-live-e2e.sh
```

スクリプトはこの checkout の daemon・CLI・credentiald と GUI を build し、実 GUI を password 認証付きで起動する。LLM の代わりに fake worker を使う。scratch は実行ごとに一意な directory となり、設定・DB・broker の HOME は全てそこに置く。既定 port は GUI 27700、daemon API 27710、dashboard 27848、fixture 27861。本番 7700/7710 は明示的に拒否し、終了時はこの実行のプロセスを止める。証跡 directory は空であることを要求し、前回の証跡を削除しない。

`gui/e2e/g14-browser-live.spec.ts` は次を実ブラウザで検査する。

- WAITING_FOR_AUTH の登録依頼、owner-session の CLI 承認、GUI から手動登録、新しい worker run の開始。
- WAITING_FOR_APPROVAL の GUI 承認と再開、別 wait の拒否と `approval_denied`。
- 認証 task が残る間の relay 拒否、その task を終了した後の本人 dashboard 成功、実 WS frame 受信、他 session 403、未認証 401、他 run 404、操作 API 403、logout 後の WS 切断。
- DB・WAL・events API・daemon/GUI ログ・broker vault・成果物の秘密 sentinel 走査（終了後も再走査）。trace・video・自動 screenshot は無効で、秘密入力前と登録後の明示 screenshot だけを保存する。

証跡は `01-waiting-for-auth.png` から `11-task-failed.png`、`live-view-owner.png`、`live-view-non-owner.png`、`live-view-unauthenticated.png`、`live-view-ws.json`、`sentinel-scan.txt`、各ログ。WS の単体負例は `gui/test/unit/browser-live-relay.test.ts` にあり、操作 message 非転送・認証区間での frame 非転送・grant 失効も検査する。

## 検証の範囲

GUI と daemon、broker、dashboard、Chromium は実物。wait の発行と `browser_updated` event は試験が supervisor の代わりに作るため、この試験は worker の credential plugin 接続の成功を意味しない。

実 agent-browser の auth login は別の `scripts/browser-auth-login-check.py` で診断する。現行 worker の plugin config（map）、argv（`--credential-ref` と positional name の欠落）、FD 3 の渡し方は 0.38.1 と非互換で fail closed になる。さらに segment policy の `url` 欠落と policy パス変更による session 再起動がある。試験専用の修正・token 受け渡しを使ったローカル HTTPS fixture の login と lease 再使用拒否は確認できたが、**worker の結線は未修正であり、Phase 2 全体の実機認証成功とは扱わない**。

namespace は本人専用にする。同一 UID の直接 loopback 接続に対する隔離はなく、別 network namespace・外部 host からの到達不能はこの試験では未検証。persistent auth、GUI 内の独自 frame 描画、container/egress 隔離は対象外。
