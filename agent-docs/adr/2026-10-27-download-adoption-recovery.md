# timeout 後の download 採用でも browser daemon を回復する

- 日付: 2026-10-27
- 状態: 採用
- tasks: [01M5XXF7QAXTD8E7MM6DF25JRG]
- 関連: [credential / post-login ADR](2026-10-09-browser-credential-username-and-post-login-read.md) 付記 2026-10-10m

## 背景

配送 gate の `real_sandbox_launcher_chrome_downloads_inline_pdf_after_login` が、拒否した別 origin の download 後の snapshot で timeout した。
保存済み GUID の採用分岐は成功を返すため、timeout 時の daemon 回復を飛ばしていた。採用したファイルを launcher が拒否しても、次の操作は待ち続ける daemon に届く。

## 決定

timeout を観測した download は、採用の成否にかかわらず既存の daemon 回復を実施する。回復した場合は採用応答にも `recovery=daemon_restarted` を付ける。
GUID の一意性・出力先の存在検査・launcher の許可照合は変更しない。unknown ref 等、timeout 以外の失敗は回復対象にしない。
回帰試験は Python runner の関数を読み込み、偽 subprocess と一時ファイルで timeout・GUID 採用・次の snapshot を決定的に再現する。実 browser/userns の試験は release gate に残す。
