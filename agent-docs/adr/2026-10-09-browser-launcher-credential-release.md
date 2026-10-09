# ADR 2026-10-09: launcher 経路の browser CredentialUse 解放

---
tasks: [01M4FRN13Z206RAWBZ0NHZQEGX]
---

- 日付: 2026-10-09
- 状態: 採用（launcher session に限る CredentialUse の条件と経路。実装・台帳更新・host 実証は後続工程）
- 決定者: 人（2026-10-09、manaba 課題監視 task 01M4FPA6ADFH639XYP894ES87J）
- 関連: [ADR-0116](0116-browser-launcher-implementation.md)、[ADR-0138](0138-browser-prod-admission-confidential-release.md)、[ADR-0080](0080-browser-phase2-policy-broker-approval.md)、[credential 台帳証拠 ADR](2026-10-09-browser-ledger-credential-evidence.md)

## 決定

launcher runtime で `CredentialUse`（必須能力 `CredentialInjection`）を使えるのは、同じ稼働 session について以下のすべてが成立した場合だけとする。

1. ADR-0138 D-L の `LauncherSessionProof` がある。
2. 証明の検証が成功し、launcher UID・session/instance・PID/starttime の束縛が一致する。
3. user namespace owner が daemon UID ではない。
4. `isolation_ok` が真で、実行時の隔離事実の採取・検証にも成功する。

一つでも欠ける、検証・IPC・採取が失敗する、launcher が無い場合は browser session 接続・credential 操作より前に拒否する。daemon runtime や別 backend へ fallback しない。拒否は理由を含む安全な固定 code とし、秘密・CDP payload は log、event、artifact、LLM 観測へ出さない。既存の承認・短い lease・origin/policy binding、H3 観測停止、credential lifecycle と cleanup は維持する。

## 証明の搬送と秘密の経路

launcher worker は既存の launcher IPC 応答から `LauncherSessionProof` を検証可能な形で得る。worker は credentiald の injection IPC にある `LauncherProofRegistration`（`session_id`, `instance_id`, `peer_uid`, `proof`）として登録し、broker の `LiveRegistry` に session 登録と結び付ける。credentiald は `Admission::Attested` の既存経路で `/proc` 等から実 process facts を採り直し、既存 `admit_attested` と `verify_launcher_session` を再利用する。登録された証明自体を信頼の根拠として無条件に受け入れない。証明なし・不一致・launcher UID 未設定・検査失敗は拒否する。daemon runtime は従来どおり証明なしで拒否される。

credentiald が secret を扱い、launcher 側の trusted controller / CDP sink が browser へ注入する。task controller、LLM、一般 action IPC に secret を返さない。注入は固定の認証操作として CDP の認証 sink へ渡し、既存の redaction と auth interval の遮断を適用する。controller は結果 status のみを受け取る。登録 IPC に含めるのは session 証明だけで credential 値は含めない。

## IdentityRestore

この決定では `IdentityRestore` を解放しない。restore は保存 identity の選択・復元とその後の session 全体にわたる観測停止、project/origin scope、期限・失効を伴う別の機密経路であり、今回の目的である手動承認付きの一回の login 注入からは必要性も lifecycle の適合も導けない。launcher の同じ隔離条件は将来の必要条件として維持するが、それだけで restore を許可しない。restore の解放は専用判断とその証拠を要する。

## 台帳と host 条件

`credential_backends` の credential 対応は、backend ID だけでなく証拠を生成した runtime（daemon / launcher）で区別する。launcher 対応を認めるのは launcher runtime で取得した `isolation_suite`・`egress_negative_suite` 等の実証証拠がその backend に紐付く場合だけとする。daemon の証拠を launcher の適合として流用しない。launcher が無い host は launcher credential evidence を生成せず、理由を記録して credential を空にする（非機密 capability の台帳結果は独立に扱う）。

機密経路の試験名は `launcher_credential_`、台帳試験名は `browser_ledger_launcher_` を接頭辞とする。

host 実証（`CELERIS_LAUNCHER_TESTS=require` の必須モード stutter 3 回、および統合後 HEAD で `ADMISSION[real-session]` を再取得）は運用セッションが root で行う。本 ADR の採用や合成試験だけで host 実証済み・本番有効とは扱わない。既存の運用手順を更新し、結果と HEAD を記録してから人が本番昇格を判断する。

## 既存 ADR への付記

- [ADR-0116](0116-browser-launcher-implementation.md) の末尾付記を参照。
- [ADR-0138](0138-browser-prod-admission-confidential-release.md) の末尾付記を参照。
- [ADR-0080](0080-browser-phase2-policy-broker-approval.md) の末尾付記を参照。
