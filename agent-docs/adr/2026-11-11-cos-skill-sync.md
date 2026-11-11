---
tasks: [01M75722RHE7ZQ6KS09FMSXD3V]
---
# ADR 2026-11-11: CoS の skill の正本は repo、KB は配備先。昇格で写し、版が合わなければ CoS run を起こさない

- 状態: 採用（実装済み）。
- 関係: [ADR 2026-10-05 cos-chat-home](2026-10-05-cos-chat-home.md) D3/D6（`[cos.triage]` の policy_skill・policy_version）、
  [ADR 2026-10-28 cos-session-policy-key](2026-10-28-cos-session-policy-key.md)（版と skill の hash を session key に入れる）、
  [ADR 2026-10-28 cos-routine-delegated-answers](2026-10-28-cos-routine-delegated-answers.md)（policy_version の bump の手順）、
  [ADR-0127](0127-skills-native-delivery.md)（KB の `skills/<name>/` からの配送）、
  [ADR 2026-10-11 破壊的変更](2026-10-11-breaking-changes-by-default.md)。
- 記録: [進捗](../progress/2026-11-11-cos-skill-sync.md)。

## 背景

CoS の run は skill を本番の KB（`[knowledge] root` の `skills/`）から読み、repo の `config/skills/` は読まない。
repo の skill を KB に写す段はどこにも無く、人が手で写していた。2026-11-11 に operator が読み取りで確かめた本番は次のとおり。

- KB の `cos-inbox-triage` の `metadata.version` は 10、本番 config の `[cos.triage] policy_version` は "11"
  （2026-11-05 の release の A7 の commit で 10→11）。daemon は 11 を CoS に渡し、CoS は 10 の方針で一次対応していた。
- Core（`task_worker::cos_chat::core`）が読めと書く `cos-operator/consult.md` と、`cos-operator/SKILL.md` が指す
  `project-controller.md` が KB に無かった。

version を上げても KB が古いままでは、CoS は版の違う方針で動き、それを誰も検出しない。

## 決定

### D1: 正本は repo、KB の `skills/<CoS の skill>/` は配備先

CoS の skill（`config/skills/cos-operator`・`config/skills/cos-inbox-triage`）の正本は repo。KB の同名の dir は
release の写しで、人や curation が KB の側で直さない（直した分は次の配備で上書きされる）。直すときは repo を変え、
release と昇格を通す。KB の他のページ（curation が書いた `projects/` など、CoS 以外の skill）は配備で変えない。

### D2: release に写しを入れ、昇格で KB に写して KB の git に commit する

- `release.sh` は gate の通った作業ツリーの `config/skills/<name>/`（`scripts/selfdeploy/cos-skills.txt` に並べた名前だけ）を
  release の `skills/<name>/` に入れる。release は本番に触れない（ADR-0040 D5 のまま）。
- `promote.sh` は昇格を記録した後（credentiald の切り替えと同じ位置）に
  `sync-skills.sh <release> <knowledge root>` を呼ぶ。`sync-skills.sh` は `cos-skills.txt` の各 dir を KB の
  `skills/<name>/` と丸ごと置き換え（release に無い file は消す）、その path だけを `git add` して、差があれば
  `selfdeploy: sync CoS skills from release <sha12>` で commit する。他の path は stage しないので、curation の未 commit の変更も
  commit されない。KB が git でなければ写すだけ。`rollback.sh` も戻した先の release で同じ段を呼ぶ。
- この ADR より前に作った release（rollback の戻り先など）は `skills/` を持たない。そのときは KB を変えずに log に残して終わる（exit 0）。
  KB が config と合わなければ D3 が CoS run を止めて知らせる。release.sh は束ねられなければ release を作らないので、新しい release は必ず `skills/` を持つ。
- 写しの失敗は昇格を取り消さない（daemon は既に新しい）。exit 4 と log で知らせ、D3 が CoS run を止めて受信箱に出す。
- 時機を昇格の後にした理由: 昇格の前に写すと、昇格が失敗して旧 daemon に戻ったときに KB だけが新しくなる。
  昇格の後なら、新しい daemon が起きてから写すまでの間（数秒）だけ D3 が CoS run を止める。

### D3: CoS run の claim の前に skill と config を照らす（版の組ごとに 1 回知らせる）

dispatcher の CoS の tick（`tick_cos_chat_launch`）は、新しい run を claim する前に
`task_ops::cos_skill_gate::SkillGate::ready` を呼ぶ。次のどれかなら claim せず（queue の発言・受信箱の件・relay・plan の
再判定のどれも run を起こさない）、`false` を返す。

1. `[cos.triage] policy_skill` の `SKILL.md` の front matter の `metadata.version` が `[cos.triage] policy_version` と違う（無いも違う）。
2. Core が読めと書く file（`CORE_SKILL_FILES`: `cos-operator/{SKILL.md,consult.md,operations.md,deploy.md}`）が無い。
3. `cos-operator` と policy skill の `SKILL.md` が同じ dir の `.md` を link（`](x.md)`）しているのに、その file が無い。

止めたときは notice（`NoticeKind::BadNews`、group `bad_news:cos:skill_drift`）を 1 件記録する。source key は
`cos:skill-drift:<skill>:<skill の版>:<policy_version>:<無い file>` で、同じ組は notice store が 1 回しか数えないので、
tick を重ねても daemon を再起動しても繰り返さない。組が変われば（例: 版は直ったが file が無い）新しい 1 件になる。
合えば次の tick から claim する。既に claim 済みの run の route 再試行・枠待ちは止めない（claim の時点で合っていた）。
判定はファイルを読むだけで LLM を使わない。

起動直前の「skill が無ければ prelaunch で `SkillsUnavailable`」の判定（`launch_claimed` の missing skills）は D3 と同じことを
claim の後で二度見ていたので消した。`SkillsUnavailable` は policy key の計算（skill tree の読み取り）の失敗にだけ残る。

### D4: 版を上げる手順

policy_version を上げる変更は、同じ commit で `config/skills/cos-inbox-triage/SKILL.md` の `metadata.version` と
`[cos.triage]` の既定（`crates/celeris/src/config/cos.rs`）を上げる（`task_ops::cos_skill_gate` の試験が repo の skill を
既定の版で通すことを確かめる）。本番では昇格（D2）で KB が新しい版になり、config の `policy_version` を人が書き換えて reload する。
どちらかが遅れている間、D3 が CoS run を止めて受信箱に知らせる。

## 帰結

- KB の CoS の skill を KB の側で直すと、次の昇格で消える。
- config と KB の版がずれると CoS は止まる（違う方針で黙って動くより良い）。受信箱の件は CoS の不在の fallback
  （`unavailable_after_secs`）で人に回る。
- 研究評価の記録は変えない。CoS の判断の根拠と実行者は従来どおり operation と resolve に残る。
