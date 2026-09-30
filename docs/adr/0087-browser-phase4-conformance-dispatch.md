# ADR-0087: P4-C の実測適合記録と実行時 fallback

---
tasks: [01M3QGRC6AQ81PWM1XP4C7BH45]
---

- 日付: 2026-09-29
- 状態: Accepted（実装の受け入れは別途検証する）
- 関連: [ADR-0084](0084-browser-phase4-isolation-injection-routing.md)、[ADR-0085](0085-browser-phase4-runtime-selection.md)

## 決定

1. 適合記録は固定 agent-browser 0.38.1 を実際に起動した runner の出力だけから作る。runner は loopback の fixture server と同一 task 定義を使い、ACP・明示 Claude・browser-specialist ごとに結果を保存する。宣言や unit test の成功を実測結果に変換しない。実行時は backend ID と browser バージョンが一致する記録だけを読む。記録が無い、壊れている、古い場合は適合なしと扱う。
2. browser-specialist は既存 harness の browser 専用設定として登録する。既存 loop と同じ supervisor・policy・CLI shim を通す。公開能力の比較だけを対象とし、`CredentialInjection` と `IdentityRestore` は P4-A/B の実適合記録ができるまで宣言しない。
3. dispatch が持つ候補 adapter を worker に渡し、起動不可・失敗時は要求能力をすべて満たす次の backend を新しい browser session で試す。途中に認証 wait または auth_section が生じた run は fallback しない。候補が尽きれば理由を明示して拒否する。単なる適合順序の計算を実行時 fallback の証拠に数えない。
4. 同一 task 比較の試験では LLM を scripted にし、実 agent-browser とローカル fixture を使う。実 LLM 比較は認証が利用できる環境で別途実施し、実施できない環境ではコマンド・判定手順・未実施理由を PROGRESS に残す。

## 理由

旧実装は全 fixture を通った `ConformanceResult` を worker 内で組み立てていたため、routing が実測値を読んでいなかった。また、fallback リストを返すだけで dispatch は選択された adapter を一度しか起動しなかった。両方とも P4-C の受け入れを満たさない。
