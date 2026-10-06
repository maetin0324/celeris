---
tasks: [01M4896B1Q1617Q95NT2BNDNAF]
status: done
updated: 2026-10-06
---

# R7・R8 実機確認台本の修正

`scripts/dev/browser-web-live-check.sh` の拒否証跡採取を、daemon の browser run session ID ではなく launcher の `Started` 応答の session ID に合わせた。daemon はこの ID を `browser launcher session started session=...` と記録する。task 作成直前の `daemon.log` の byte offset から後だけを読み、ID が一意でないときは失敗する。対応する `state_dir/sessions/<launcher session ID>/egress-denied.jsonl` に、拒否された port・host・理由・時刻・同じ session ID があることを確かめる。3 回目の実機ログでは daemon の session ID `celeris-8086a3b15ad3fa0427d408d3227f3fa3` と launcher の ID `73bfab2e39024c652947a75868b3a344` が異なっていた。

許可ページの `#forbidden-link` は許可外 origin の port（既定 17731）へ向く。task の objective は、許可ページを開いて snapshot の `@e` 参照でこのリンクをクリックするよう指定する。これで Chrome の遷移要求を egress proxy に通し、拒否記録を採る。wrapper に許可外 URL を直接 `navigate` させる方法は使わない。許可外 server の GET が 0 件である判定は維持した。

検査: `bash -n scripts/dev/browser-web-live-check.sh`、埋め込み Python 10 塊の構文確認、3 回目の `daemon.log` からの launcher ID 抽出、`git diff --check` は成功した。launcher と Chrome を要する台本全体の実行は Fable の 4 回目の実機確認で行う。
