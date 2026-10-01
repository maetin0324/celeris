# ADR-0114: identity 復元の state を controller の CDP に投入する（deliver_state）と試験 admission

- 状態: 採用
- 日付: 2026-09-30
- 関連: ADR-0102 D4、ADR-0105 D5、ADR-0108 D5、ADR-0109 D6

## 背景

ADR-0108 D5 で `LiveSessionEntry::accepts_state` / `deliver_state` の口を作ったが、supervisor の entry
（`SupervisedEntry`）と直起動の `LiveSession` はどちらも `accepts_state() == false` のままで、
`restore_isolated` で開封した state を controller に渡す成功経路が無かった。
またこの host には別 UID が無く、本番の検査（`verify_isolation`）は必ず `SameUid` で落ちるので、
実 runtime での成功経路を試験で実証する手段も無かった。

## 決定

### D1: 投入は controller の CDP だけ

- 開封済み state（`IdentityStatePlain` の JSON）は `browser_runtime::deliver_state_via` で
  `Storage.setCookies` の 1 command にして、controller が持つ CDP pipe に書く。対応する種別は
  `cookie` だけ。知らない種別・https でない origin が 1 件でもあれば 1 件も投入しない。
- `SupervisedEntry` は controller の口（`Arc<Mutex<CdpController>>`）を持つ slot を `Supervisor` と共有する。
  `Supervisor::attach_controller` が入れるまでは `accepts_state() == false`（開封しない）。
  停止時は registry から外した直後に slot を空にする。本番（`browser.rs`）は `SharedCdp` の controller を渡す。
- `LiveSession` は runtime が CDP pipe を持つ間だけ `accepts_state() == true` で、lock を持ったまま
  その pipe に投入し、応答を読み切ってから返す。
- state は agent の接続・LLM・run log・argv・ファイルに出さない。`--restore` / `--state` / `--profile` の
  ような引数やファイル受け渡しは使わない。

### D2: 試験 admission（`same-uid-harness` feature）

- `browser_runtime::RestoreAdmission` を置く。本番は `Attested`（`verify_isolation` そのまま）だけ。
- `same-uid-harness` feature（task-worker。既定では無効、dev-dependency でだけ有効）でのみ
  `SameUidHarness` があり、**実 runtime から採った事実**の違反が `SameUid` だけのときに限り、
  UID を別 UID として扱って attestation を作る。他の違反が 1 つでもあれば拒否する。
- `SupervisorOptions::admission`（既定 `Attested`）で supervisor の entry の attestation に使う。
- H5 の project + origin の束縛・期限・削除・session id の照合は `restore_in_session` /
  `restore_isolated` のまま変えない（開封前に拒否）。

## 結果

- `crates/task-worker/tests/browser_restore_deliver.rs` が実 bwrap + chrome-headless-shell で、
  試験 admission の成功経路（開封 → `Storage.setCookies` → controller の `Storage.getCookies` で見える）、
  本番 admission の `SameUid` 拒否、他 project・別 origin・期限切れ・削除済み・別 session の開封前拒否を確かめる。
- 別 UID を持つ host での本番 admission の成功経路は、別 UID の runtime ができるまで未実証のまま。
