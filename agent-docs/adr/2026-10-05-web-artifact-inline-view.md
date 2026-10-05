# web/ の成果物をブラウザ内で表示する経路と安全性（2026-10-05）

状態: 採用・実装済み

## 背景

旧 GUI（gui/）は成果物を形式ごとにブラウザ内で表示していた（MarkdownViewer・CodeViewer・ImageViewer など）。
web/ の `ArtifactPreview` は Markdown だけを表示し、他は download だけだった。gateway の `web/server/files.js` は
H8（同一 origin で能動的な内容を実行させない）のため、HTML・SVG・XML を常に `Content-Disposition: attachment`、
全応答に `CSP: sandbox; default-src 'none'; frame-ancestors 'none'` と `X-Frame-Options: DENY` を付けている。
daemon は Content-Type の閉じた表（api.md §3.8）に無い html・svg・pdf を `application/octet-stream` で返す。
このため HTML は iframe でも新しいタブでも表示できず、PDF・SVG も表示できなかった。

## 決定

- D1: gateway に表示用の query `view=1` を足す。gateway だけが解釈し daemon へは送らない。`download=1` との併用は 400。
  `view=1` の無い要求の挙動（attachment・`frame-ancestors 'none'`・`X-Frame-Options: DENY`）は変えない。
- D2: `view=1` では daemon が種類を決めなかった応答（octet-stream・無し）だけ、Content-Disposition の file 名の拡張子で
  `text/html`（html・htm）・`image/svg+xml`・`application/pdf` を補う。daemon が決めた種類は上書きしない
  （`.html` 名の text/plain を HTML にしない）。Disposition は `inline`（file 名は残す）。`nosniff` は常に付ける。
- D3: HTML・SVG（と PDF 以外の全ての `view=1`）の CSP は
  `sandbox; default-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:; form-action 'none'; base-uri 'none'; frame-ancestors 'self'`。
  `sandbox` に allow-* を付けない: document は opaque origin になり、script・form・popup・top 遷移・plugin は動かない。
  これは iframe の中でも新しいタブで直接開いても同じ（応答 header が効くので埋め込み側に依らない）。
  外への読み込み（connect・img の http・font）も `default-src 'none'` で止める。inline の style と data: の画像だけ許す。
- D4: PDF は `sandbox` を付けない（Chrome の PDF viewer は sandbox の document で表示を拒む）。代わりに
  `default-src 'none'; object-src 'self'; form-action 'none'; base-uri 'none'; frame-ancestors 'self'`。本文は
  `application/pdf` と `nosniff` で HTML として解釈されない。PDF の描画は browser の viewer（Chrome は別 process の
  extension、Firefox は pdf.js）に任せ、pdf.js を同梱しない（依存を増やさない）。
- D5: `view=1` の応答は `frame-ancestors 'self'` と `X-Frame-Options: SAMEORIGIN`。埋め込めるのは自 origin の SPA だけ。
  他 origin からの framing（clickjacking）は引き続き止まる。
- D6: SPA の document の CSP に `frame-src 'self'` を足し、`object-src 'none'` を `'self'` にする。SPA の script は
  `script-src 'self'` のままなので、埋め込む先を決めるのは SPA のコードだけ。自 origin で埋め込める応答は
  `view=1`（D3・D4 で縛る）だけで、`view=1` の無い `/files` は `frame-ancestors 'none'` のまま。
- D7: client（web/components/content）は二重に守る。HTML は `sandbox=""`（allow-* なし）の iframe と
  `referrerpolicy="no-referrer"`。SVG は `<img>` で読む（img の SVG は script を実行しない）。PDF は
  `<object type="application/pdf">`（viewer の無い browser では新しいタブ・download の案内を代わりに出す）。
  text・code・JSON・CSV・log は `offset`/`length` の範囲取得（128 KiB ずつ）で分けて読み、React の text node だけで描く。
- D8: 表示の種類は file 名の拡張子で決める（artifact-kind.ts）。表に無い形式は download だけ。

## 安全性の判断

- 同一 origin の script 実行: HTML・SVG は D3 の CSP sandbox（応答）と D7 の iframe sandbox（埋め込み）の二重で opaque origin。
  gateway の Cookie・localStorage・`/api` に触れない。form の POST も `sandbox`（allow-forms なし）と `form-action 'none'` で止まる。
- 残る危険: (a) HTML の見た目で利用者を欺く（phishing 風の表示）。sandbox の中では入力の送信・遷移・popup ができないので、
  操作を盗む経路は無い。(b) PDF viewer 自体の脆弱性（例: pdf.js の CVE-2024-4367）。browser の更新に依る。
  sandbox を外すのは PDF だけで、`default-src 'none'` により PDF の document から外への読み込みは無い。
- script を許す案（`sandbox allow-scripts` の opaque origin）は採らない。グラフ等の JS 付き HTML 報告は静的にしか見えないが、
  必要なら download して手元で開く。許す場合は別 origin（専用 host）からの配信と合わせて改めて決める。

## 試験

- gateway 単体: `web/server/files.test.mjs`（view=1 の Content-Type 補完・inline・CSP・XFO、download 併用の拒否、
  view 無しの不変、daemon へ view を送らない）。
- Playwright（functional）: `web/e2e/work/artifact-viewer.spec.ts`。実 gateway と偽 daemon で md・log・png・svg・pdf・
  html・script 付き html・zip を開き、script 付き HTML が iframe でも新しいタブでも走らず `window.origin === "null"`、
  親の title・`window.__pwned`・localStorage が変わらず、form の POST が daemon に届かないことを示す。
