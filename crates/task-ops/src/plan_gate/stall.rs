//! ADR 2026-10-22-stalled-auto-replan: 進展の無い自動 replan の検出。daemon が失敗から自動 replan を起こすたびに
//! `Event::AutoReplanRequested{key, failure}` を積み（D1）、同じ WU・同じ失敗の識別子（D2）の自動 replan が、間に
//! 葉の進展（変更のある `WorkUnitCommitted`。D3）なしに [`STALLED_REPLAN_LIMIT`] 回続いたら、次は自動 replan をやめて
//! 人に聞く（D4 / D5）。events を読むだけ（LLM も I/O も無い）。

use task_core::{AutoReplanSource, Event, PhaseResumeMode};

/// D4: 同じ `(key, failure)` の自動 replan が進展なしにこの回数続いたら、次の失敗では自動 replan をしない
/// （1 回目の失敗で replan、同じ失敗の 2 回目でもう 1 回、3 回目で止める）。
pub const STALLED_REPLAN_LIMIT: u32 = 2;

/// D5: 進展の無い replan を止めたときの質問の頭。回答は人の replan の依頼として扱う（上限に数えない。
/// [`super::answered_replan_exhausted`]）。
pub const REPLAN_STALLED_QUESTION_PREFIX: &str =
    "同じ失敗で進展の無い replan が続いたので自動 replan を止めました";

/// D5: 進展の無い replan を止めたときの質問文（`WorkerQuestion` / `QuestionRaised` の本文）。
pub fn replan_stalled_question(key: &str, repeats: u32, why: &str) -> String {
    format!(
        "{REPLAN_STALLED_QUESTION_PREFIX}（WorkUnit {key}、同じ失敗の replan {repeats} 回、間の commit なし）: {why}。\
         計画では直らない原因（環境・依存の生成物など）なら直してから回答してください。回答すると、回答を planner への\
         指示として replan します（人の replan は上限に数えません）。やめるなら task を取り下げてください。"
    )
}

/// D2: 検査の cmd の正規化。`bash -c '<inner>'` / `sh -lc "<inner>"` の包みを外し、`&&`・`;`・改行で区切った段のうち
/// `set -…`・`export VAR=…`・代入だけの段を捨て、`env VAR=val cmd`・`VAR=val cmd` の代入の前置きを外し、空白を 1 つに
/// 詰める（planner が check の書き方だけを変えた計画を同じ失敗と見るため）。`cd` は意味を変えるので残す。
pub fn normalize_check_cmd(cmd: &str) -> String {
    normalize_depth(cmd, 0)
}

fn normalize_depth(cmd: &str, depth: u32) -> String {
    let segments: Vec<String> = split_segments(cmd)
        .into_iter()
        .filter_map(|seg| {
            let seg = seg.trim();
            if seg.is_empty() || is_set(seg) || is_export(seg) || is_assignments_only(seg) {
                return None;
            }
            let seg = strip_env_prefix(seg);
            if depth < 4
                && let Some(inner) = unwrap_shell_c(&seg)
            {
                let inner = normalize_depth(&inner, depth + 1);
                return (!inner.is_empty()).then_some(inner);
            }
            Some(seg.split_whitespace().collect::<Vec<_>>().join(" "))
        })
        .collect();
    segments.join(" && ")
}

/// 引用符の外の `&&`・`;`・改行で区切る。
fn split_segments(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        match quote {
            Some(q) => {
                if ch == '\\' && q == '"' {
                    cur.push(ch);
                    if let Some(next) = chars.next() {
                        cur.push(next);
                    }
                    continue;
                }
                if ch == q {
                    quote = None;
                }
                cur.push(ch);
            }
            None => match ch {
                '\'' | '"' => {
                    quote = Some(ch);
                    cur.push(ch);
                }
                ';' | '\n' => out.push(std::mem::take(&mut cur)),
                '&' if chars.peek() == Some(&'&') => {
                    chars.next();
                    out.push(std::mem::take(&mut cur));
                }
                _ => cur.push(ch),
            },
        }
    }
    out.push(cur);
    out
}

fn is_set(seg: &str) -> bool {
    seg == "set" || seg.starts_with("set ")
}

fn is_export(seg: &str) -> bool {
    seg.starts_with("export ")
}

fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_assignments_only(seg: &str) -> bool {
    seg.split_whitespace().all(is_assignment)
}

/// `env [-i] VAR=val … cmd` と `VAR=val … cmd` の前置きを外す。
fn strip_env_prefix(seg: &str) -> String {
    let words: Vec<&str> = seg.split_whitespace().collect();
    let mut i = 0;
    if words.first() == Some(&"env") {
        i = 1;
        while i < words.len() && (words[i].starts_with('-') || is_assignment(words[i])) {
            i += 1;
        }
    }
    while i < words.len() && is_assignment(words[i]) {
        i += 1;
    }
    if i == 0 {
        return seg.to_string();
    }
    // 元の空白を保つため、外した語の後ろから切り出す（引用符の中の空白を崩さない）。
    let mut rest = seg;
    for _ in 0..i {
        rest = rest.trim_start();
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        rest = &rest[end..];
    }
    rest.trim().to_string()
}

/// `bash|sh [-opts] -c <script>`（`-lc`・`-ec` のような組も）の `<script>` を引用符を外して返す。
fn unwrap_shell_c(seg: &str) -> Option<String> {
    let seg = seg.trim();
    let (shell, mut rest) = seg.split_once(char::is_whitespace)?;
    let shell = shell.rsplit('/').next().unwrap_or(shell);
    if !matches!(shell, "bash" | "sh" | "dash" | "zsh") {
        return None;
    }
    let mut saw_c = false;
    loop {
        rest = rest.trim_start();
        if !rest.starts_with('-') {
            break;
        }
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let opt = &rest[..end];
        if !opt.starts_with("--") && opt.contains('c') {
            saw_c = true;
        }
        rest = &rest[end..];
        // `-o pipefail` の引数。
        if opt == "-o" || (opt.ends_with('o') && !opt.contains('c')) {
            rest = rest.trim_start();
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            rest = &rest[end..];
        }
    }
    if !saw_c {
        return None;
    }
    let script = rest.trim();
    let unquoted = if let Some(inner) = script.strip_prefix('\'').and_then(|s| s.strip_suffix('\''))
    {
        inner.replace("'\\''", "'")
    } else if let Some(inner) = script.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        inner.replace("\\\"", "\"").replace("\\$", "$")
    } else {
        script.to_string()
    };
    Some(unquoted)
}

/// D2: 検査の出力（`cmd=… exit=… stdout_tail="…" stderr_tail="…"` の Debug 形式を含む）を行に分ける。実の改行・
/// 逃がした `\n`・`_tail="` の境で区切り、各 tail の最初の行（末尾から切った tail の途中から始まり得る）は捨てる。
fn error_lines(output: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, part) in output.split("_tail=\"").enumerate() {
        let lines = part.split('\n').flat_map(|l| l.split("\\n"));
        for (j, line) in lines.enumerate() {
            if i > 0 && j == 0 {
                continue;
            }
            let line = line.trim().trim_end_matches('"').trim_end();
            if line.starts_with("e: ") || line.starts_with("error:") || line.contains("error[") {
                out.push(line.to_string());
            }
        }
    }
    out
}

/// D2: 失敗の識別子。正規化した cmd（昇順・重複除去）と、出力の `e: ` / `error:` / `error[` の行（出現順）の
/// FNV-1a 64 を 16 桁の hex にする。行が無ければ cmd だけの hash。
pub fn failure_signature<S: AsRef<str>>(cmds: &[S], output: &str) -> String {
    let mut normalized: Vec<String> = cmds
        .iter()
        .map(|c| normalize_check_cmd(c.as_ref()))
        .collect();
    normalized.sort();
    normalized.dedup();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    };
    for c in &normalized {
        feed(c.as_bytes());
        feed(&[0]);
    }
    feed(&[0x1e]);
    for l in error_lines(output) {
        feed(l.as_bytes());
        feed(&[0]);
    }
    format!("{h:016x}")
}

/// D3: 人の replan の依頼（連なりを切る）。
fn is_human_replan_request(e: &Event) -> bool {
    match e {
        Event::ExecutionHintSet { replan: true, .. } => true,
        Event::Transitioned { reason, .. } => {
            reason == PhaseResumeMode::PlanReplan.name()
                || reason == PhaseResumeMode::Replan.name()
                || reason == "answer"
        }
        _ => false,
    }
}

/// D3: 葉の進展（変更のある commit）。`base` が無い commit は変更ありとみる。
fn is_progress(e: &Event) -> bool {
    matches!(e, Event::WorkUnitCommitted { base, commit, .. } if base.as_deref() != Some(commit.as_str()))
}

/// D4: 末尾から見て、同じ `(key, failure)` の自動 replan が進展（[`is_progress`]）も人の依頼も挟まずに何回続いたか
/// （最後の自動 replan の後に進展があれば 0）。呼び出し側は [`STALLED_REPLAN_LIMIT`] 以上なら自動 replan をしない。
pub fn stalled_replans(events: &[(u64, Event)], key: &str, failure: &str) -> u32 {
    let mut chain = 0u32;
    let mut last: Option<(&str, &str)> = None;
    let mut progressed = false;
    for (_, e) in events {
        if is_human_replan_request(e) {
            last = None;
            chain = 0;
            continue;
        }
        if is_progress(e) {
            progressed = true;
            continue;
        }
        if let Event::AutoReplanRequested {
            key: k, failure: f, ..
        } = e
        {
            if !progressed && last == Some((k.as_str(), f.as_str())) {
                chain += 1;
            } else {
                chain = 1;
            }
            last = Some((k.as_str(), f.as_str()));
            progressed = false;
        }
    }
    if !progressed && last == Some((key, failure)) {
        chain
    } else {
        0
    }
}

/// D7: 最後に採用された版の後に、人の依頼を挟まずに統合の失敗（`source: integration`）からの自動 replan があるか
/// （その replan に応える planner の計画は done の unit の checks だけを変えてはいけない）。
pub fn pending_integration_auto_replan(events: &[(u64, Event)]) -> Option<String> {
    for (_, e) in events.iter().rev() {
        match e {
            Event::AutoReplanRequested {
                key,
                source: AutoReplanSource::Integration,
                ..
            } => return Some(key.clone()),
            Event::AutoReplanRequested { .. } | Event::ExecutionPlanned { .. } => return None,
            e if is_human_replan_request(e) => return None,
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auto(key: &str, failure: &str) -> Event {
        Event::AutoReplanRequested {
            key: key.to_string(),
            failure: failure.to_string(),
            source: AutoReplanSource::Integration,
        }
    }

    fn commit(key: &str, base: &str, commit: &str) -> Event {
        Event::WorkUnitCommitted {
            work_unit_id: format!("wu-{key}"),
            key: key.to_string(),
            branch: format!("celeris-wu/t/{key}"),
            base: Some(base.to_string()),
            commit: commit.to_string(),
        }
    }

    fn hint_replan() -> Event {
        Event::ExecutionHintSet {
            mode: task_core::ExecutionMode::Compound,
            previous: None,
            previous_decision: None,
            source: "human".into(),
            note: Some("n".into()),
            replan: true,
        }
    }

    fn numbered(events: Vec<Event>) -> Vec<(u64, Event)> {
        events
            .into_iter()
            .enumerate()
            .map(|(i, e)| (i as u64 + 1, e))
            .collect()
    }

    #[test]
    fn normalization_strips_shell_wrappers_set_and_env_prefixes() {
        let plain = "./gradlew --offline :core:test";
        for variant in [
            "./gradlew --offline :core:test",
            "  ./gradlew   --offline :core:test ",
            "bash -c './gradlew --offline :core:test'",
            "bash -lc \"./gradlew --offline :core:test\"",
            "sh -ec './gradlew --offline :core:test'",
            "/bin/bash -o pipefail -c './gradlew --offline :core:test'",
            "bash -c 'set -e; ./gradlew --offline :core:test'",
            "set -eu && ./gradlew --offline :core:test",
            "export JAVA_HOME=/opt/jdk17; ./gradlew --offline :core:test",
            "bash -c 'set -e; export JAVA_HOME=/opt/jdk17; ./gradlew --offline :core:test'",
            "JAVA_HOME=/opt/jdk17 ./gradlew --offline :core:test",
            "env JAVA_HOME=/opt/jdk17 GRADLE_OPTS=-Xmx2g ./gradlew --offline :core:test",
            "JAVA_HOME=/opt/jdk17; bash -c 'env -i PATH=/usr/bin ./gradlew --offline :core:test'",
        ] {
            assert_eq!(normalize_check_cmd(variant), plain, "{variant}");
        }
        // `cd` は意味を変えるので残す。
        assert_eq!(
            normalize_check_cmd("bash -c 'cd android && ./gradlew test'"),
            "cd android && ./gradlew test"
        );
        // 別の検査は別のまま。
        assert_ne!(
            normalize_check_cmd("./gradlew :core:test"),
            normalize_check_cmd("./gradlew :app:test")
        );
    }

    #[test]
    fn signature_uses_compile_error_lines_not_timings() {
        let a = "cmd=\"./gradlew :core:test\" exit=Some(1) expected=0 stdout_tail=\"> Task :core:compileTestKotlin FAILED\\nBUILD FAILED in 12s\\n\" stderr_tail=\"warning: x\\ne: file:///w/core/src/test/RealSshdSessionTest.kt:185:13 Unresolved reference 'destroy'.\\n\"";
        let b = "cmd=\"bash -c 'set -e; ./gradlew :core:test'\" exit=Some(1) expected=0 stdout_tail=\"> Task :core:compileTestKotlin FAILED\\nBUILD FAILED in 47s\\n\" stderr_tail=\"warning: y\\ne: file:///w/core/src/test/RealSshdSessionTest.kt:185:13 Unresolved reference 'destroy'.\\n\"";
        let c = "cmd=\"./gradlew :core:test\" exit=Some(1) expected=0 stdout_tail=\"x\\n\" stderr_tail=\"warning: x\\ne: file:///w/core/src/test/RealSshdSessionTest.kt:190:1 Unresolved reference 'close'.\\n\"";
        let sa = failure_signature(&["./gradlew :core:test"], a);
        let sb = failure_signature(&["bash -c 'set -e; ./gradlew :core:test'"], b);
        let sc = failure_signature(&["./gradlew :core:test"], c);
        assert_eq!(sa, sb, "only the wrapper and the timing differ");
        assert_ne!(sa, sc, "a different compile error is a different failure");
        assert_eq!(sa.len(), 16);
        let rust = "stderr_tail=\"Compiling x\\nerror[E0425]: cannot find function `f`\\nerror: could not compile `x`\\n\"";
        assert_ne!(
            failure_signature(&["cargo test"], rust),
            failure_signature(&["cargo test"], ""),
        );
    }

    #[test]
    fn the_same_failure_without_commits_is_counted() {
        let sig = "abc";
        let events = numbered(vec![auto("integrate-build", sig)]);
        assert_eq!(stalled_replans(&events, "integrate-build", sig), 1);
        let events = numbered(vec![
            auto("integrate-build", sig),
            // 変更の無い commit（base == commit）は進展ではない。
            commit("b", "aaa", "aaa"),
            auto("integrate-build", sig),
        ]);
        assert_eq!(stalled_replans(&events, "integrate-build", sig), 2);
        // 別の失敗・別の WU は数えない。
        assert_eq!(stalled_replans(&events, "integrate-build", "other"), 0);
        assert_eq!(stalled_replans(&events, "a", sig), 0);
    }

    #[test]
    fn a_work_unit_commit_in_between_resets_the_count() {
        let sig = "abc";
        let events = numbered(vec![
            auto("integrate-build", sig),
            commit("fix", "aaa", "bbb"),
            auto("integrate-build", sig),
        ]);
        assert_eq!(stalled_replans(&events, "integrate-build", sig), 1);
        // 最後の自動 replan の後の commit も進展。
        let events = numbered(vec![
            auto("integrate-build", sig),
            auto("integrate-build", sig),
            commit("fix", "aaa", "bbb"),
        ]);
        assert_eq!(stalled_replans(&events, "integrate-build", sig), 0);
    }

    #[test]
    fn a_human_replan_request_resets_the_count() {
        let sig = "abc";
        let events = numbered(vec![
            auto("integrate-build", sig),
            auto("integrate-build", sig),
            hint_replan(),
        ]);
        assert_eq!(stalled_replans(&events, "integrate-build", sig), 0);
    }

    #[test]
    fn the_pending_integration_replan_ends_at_the_next_plan_or_a_human_request() {
        let events = numbered(vec![auto("integrate-build", "x")]);
        assert_eq!(
            pending_integration_auto_replan(&events).as_deref(),
            Some("integrate-build")
        );
        let events = numbered(vec![auto("integrate-build", "x"), hint_replan()]);
        assert_eq!(pending_integration_auto_replan(&events), None);
        let events = numbered(vec![Event::AutoReplanRequested {
            key: "a".into(),
            failure: "x".into(),
            source: AutoReplanSource::WorkUnit,
        }]);
        assert_eq!(pending_integration_auto_replan(&events), None);
    }
}
