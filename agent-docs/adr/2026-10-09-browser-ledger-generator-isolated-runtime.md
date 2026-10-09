# 2026-10-09 browser 適合台帳の生成器を isolated runtime 以後の構成に合わせる

- 状態: 採用
- 関連: ADR-0106、ADR-0112、[2026-10-08-browser-prod-enablement](2026-10-08-browser-prod-enablement.md)、ADR-0104（egress）、ADR 2026-10-05 付記 E1（試験専用 loopback）

## 背景

`scripts/browser-conformance.py --protocol-scripted --fallback-scenario` は本番 host で一度も成功していない
（release 31779bc1 で手動再実行しても 3 backend とも `accepted: false`）。原因は生成器が P4-C 当時の構成のままだったこと:

1. shim（`browser_cli.py`）は 7c730961 以後 agent-browser を直接起動せず action socket にだけ話す。protocol 段には
   socket を提供する worker runtime が無く、全 action が失敗する。
2. 生成器の config の `allowed_domains: ["localhost"]` は旧 bare host 形式で `https://localhost:443` と読まれ、
   loopback fixture（`http://localhost:<port>`）の open が policy_block になる。
3. routing / fallback 試験は daemon の isolated runtime（bwrap・sandboxd・egress）を使うが、`cargo test --lib` は
   `celeris-browser-sandboxd`・`celeris-browser-egress` を build しないので `isolated_runtime_unavailable`。
4. fallback 試験は task 要求 `https://example.com` と grant `localhost` の交差が空（`empty_browser_domains`）。
5. egress は loopback を拒否し（試験専用 `127.0.0.1:<port>` を除く）、plain HTTP は GET/HEAD しか転送しない。
   fixture の click は POST だったので fallback で観測できない。

## 決定

- D1 protocol 段では生成器が backend ごとに action socket を host 上で提供し、旧 shim と同じ upstream flag
  （`--action-policy`・`--allowed-domains`・`--content-boundaries`）で実 agent-browser を起動する。sandbox は P4-C の
  case ではなく、fallback 段が実 isolated runtime を通す。config の `allowed_domains` は正規の origin（scheme・host・port）。
- D2 routing 段の前に生成器が sandboxd・egress の bin を build する。
- D3 fallback 試験は fixture を `http://127.0.0.1:<port>/` で開き、task 要求・grant・policy をその origin にする。
  daemon runtime の egress policy の `test_loopback_allow` は `#[cfg(test)]` の static からだけ入る（本番 binary では常に空。
  本番で入れられるのは従来どおり launcher の root 所有 config だけ）。
- D4 fixture の click の副作用は `GET /clicked`（POST をやめる）。fixture 名 `p4c-local-v1` は変えない（case と判定は同じ）。

## 範囲外

- `credential_backends` は空のまま。CredentialInjection には `isolation_suite`・`egress_negative_suite` の証拠も要るが、
  生成器はまだ書かない。credential_use の task は `ledger_lacks_credential` で止まる（従来どおり fail closed）。
