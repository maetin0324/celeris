//! 独立 grader（ADR 2026-10-16-mlsys-independent-grader）。
//! 入力は trial の成果物 directory と grader 側の case 定義だけ。成果物中の成功宣言・trial 記録の
//! outcome・mock の Passed は読まない。4 検査（実装テスト・実行証拠・独立再計算・レビュー）を独立に行う。
pub mod case {
    //! grader 側の case 定義（`celeris.grade-case/1`）。試行の作業場所には渡さない。
    use std::{collections::BTreeMap, fs, path::Path};

    use serde::{Deserialize, Serialize};
    use serde_json::Value;

    pub const GRADE_CASE_SCHEMA: &str = "celeris.grade-case/1";

    #[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "lowercase")]
    pub enum CaseSplit {
        Dev,
        Final,
        /// fixture/mock の制御試験。全検査が通っても最終合格にしない。
        Control,
    }

    #[derive(Clone, Debug, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct GradeCase {
        pub schema: String,
        pub id: String,
        pub split: CaseSplit,
        #[serde(default)]
        pub implementation: Option<ImplementationSpec>,
        #[serde(default)]
        pub experiment: Option<ExperimentSpec>,
        #[serde(default)]
        pub recompute: Option<RecomputeSpec>,
        #[serde(default)]
        pub review: Option<ReviewSpec>,
        /// 判定する工程（implementation / experiment / analysis / review）。省略時は持つ検査すべて。
        /// 持つとき analysis ⇔ recompute、review ⇔ review が一致せねばならない（ADR 2026-10-18-k2-judged-scope）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub judged_scope: Option<Vec<String>>,
    }

    #[derive(Clone, Debug, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct PristineTest {
        /// 成果物 repo 内の path（重ねる先）。pristine の実体は `pristine_dir/<path>`。
        pub path: String,
        pub sha256: String,
    }

    #[derive(Clone, Debug, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct ImplementationSpec {
        /// case file の dir からの相対 path。
        pub pristine_dir: String,
        /// 成果物 dir 内の repo の相対 path。
        pub repo: String,
        /// 公開テスト。成果物 repo に在るはずのもので、削除・改変（sha256 不一致）は失格。
        #[serde(default)]
        pub tests: Vec<PristineTest>,
        /// 評価側だけが持つ隠しテスト（`hidden_dir/<path>`）。成果物 repo には元から無く、作業 dir に挿入するだけ。
        /// 成果物側に無いことは削除判定にしない（在っても上書きし、改変判定もしない）。
        #[serde(default)]
        pub hidden_dir: Option<String>,
        #[serde(default)]
        pub hidden_tests: Vec<PristineTest>,
        pub command: Vec<String>,
        /// 出力に `test <name> ... ok` が現れねばならない試験名。
        #[serde(default)]
        pub must_pass: Vec<String>,
        /// 初期 tree（隠しテストなし）が評価と同じ build 設定で compile できるかを見る command。
        /// 評価が compile error で落ちたとき、grader 側の初期 tree でこれが落ちれば case/asset の不良
        /// （env_failure）であってモデルの失敗ではない。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub baseline_command: Option<Vec<String>>,
    }

    #[derive(Clone, Debug, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct ExperimentSpec {
        /// 成果物 dir 内の証拠一覧（既定 `evidence/jobs.json`）。
        #[serde(default = "default_jobs_file")]
        pub jobs_file: String,
        pub min_jobs: usize,
        /// 各 job の `conditions` に要求する値（node 数等）。
        pub conditions: BTreeMap<String, Value>,
    }

    fn default_jobs_file() -> String {
        "evidence/jobs.json".into()
    }

    #[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "lowercase")]
    pub enum Agg {
        Mean,
        Median,
        Sum,
        Min,
        Max,
        Count,
    }

    #[derive(Clone, Debug, Serialize, Deserialize)]
    #[serde(deny_unknown_fields, rename_all = "snake_case")]
    pub enum Source {
        /// 各 file の JSON pointer の値。
        Json { files: Vec<String>, pointer: String },
        /// 各 file の CSV 列（header 行あり）。`filter` の列＝値の行だけ。
        Csv {
            files: Vec<String>,
            column: String,
            #[serde(default)]
            filter: BTreeMap<String, String>,
        },
        /// 他の指標の比。
        Ratio {
            numerator: String,
            denominator: String,
        },
    }

    #[derive(Clone, Debug, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct MetricSpec {
        pub name: String,
        pub source: Source,
        #[serde(default = "default_agg")]
        pub agg: Agg,
        /// 相対許容差（|reported - recomputed| <= tol * max(|recomputed|, 1e-12)）。
        pub tolerance_rel: f64,
    }

    fn default_agg() -> Agg {
        Agg::Mean
    }

    #[derive(Clone, Debug, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct RecomputeSpec {
        /// 成果物 dir 内の報告（`metric <name> = <value>` 行）。
        pub report: String,
        /// 固定入力の sha256（成果物 dir 内の path → sha256）。
        #[serde(default)]
        pub pinned_raw: BTreeMap<String, String>,
        /// sha256 固定済みの入力 tgz。あれば grader が自分の作業場所へ展開し、`pinned_raw`・metrics の
        /// 生入力はその展開先から読む（作業者がどこに展開したかに依存しない）。
        #[serde(default)]
        pub input_archive: Option<InputArchive>,
        pub metrics: Vec<MetricSpec>,
        /// セル有効性の再計算と表・件数 metric の照合（cmp4 系。ADR 2026-10-23-k4-cells-validity-grading）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub cells: Option<crate::cells_validity::CellsSpec>,
    }

    #[derive(Clone, Debug, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct InputArchive {
        /// case file の dir からの相対 path。
        pub path: String,
        pub sha256: String,
        /// 展開先（grader の作業 dir 内の相対 path）。`pinned_raw` の path はこの dir を含む形で書く。
        #[serde(default = "default_extract_to")]
        pub extract_to: String,
    }

    fn default_extract_to() -> String {
        "inputs".into()
    }

    #[derive(Clone, Debug, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct DefectSpec {
        pub id: String,
        pub file: String,
        /// 含む行範囲 [start, end]。無ければ file 全体。
        #[serde(default)]
        pub lines: Option<(u64, u64)>,
        /// 指摘本文に 1 つ以上含まれるべき語（大文字小文字を区別しない）。
        pub keywords: Vec<String>,
    }

    #[derive(Clone, Debug, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct ReviewSpec {
        #[serde(default = "default_findings_file")]
        pub findings: String,
        pub known_defects: Vec<DefectSpec>,
        /// list 外だが実在が確認済みの指摘（検出にも誤指摘にも数えない）。
        #[serde(default)]
        pub also_valid: Vec<DefectSpec>,
        pub min_detection_rate: f64,
        pub max_false_positives: usize,
    }

    fn default_findings_file() -> String {
        "review/findings.json".into()
    }

    impl GradeCase {
        pub fn load(path: &Path) -> Result<Self, String> {
            let b = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let c: GradeCase =
                serde_json::from_slice(&b).map_err(|e| format!("{}: {e}", path.display()))?;
            c.validate()?;
            Ok(c)
        }

        pub fn validate(&self) -> Result<(), String> {
            if self.schema != GRADE_CASE_SCHEMA {
                return Err(format!("unknown case schema {}", self.schema));
            }
            if self.id.is_empty() {
                return Err("case id is empty".into());
            }
            if let Some(js) = &self.judged_scope {
                for (stage, has) in [
                    ("implementation", self.implementation.is_some()),
                    ("experiment", self.experiment.is_some()),
                    ("analysis", self.recompute.is_some()),
                    ("review", self.review.is_some()),
                ] {
                    if js.iter().any(|x| x == stage) != has {
                        return Err(format!(
                            "judged_scope and the {stage} check disagree (recompute/review may be null only for a not-judged stage)"
                        ));
                    }
                }
                if let Some(x) = js.iter().find(|x| {
                    !["implementation", "experiment", "analysis", "review"].contains(&x.as_str())
                }) {
                    return Err(format!("judged_scope has unknown stage {x}"));
                }
            }
            if let Some(i) = &self.implementation {
                if (i.tests.is_empty() && i.hidden_tests.is_empty()) || i.command.is_empty() {
                    return Err("implementation needs tests and a command".into());
                }
                if !i.hidden_tests.is_empty() && i.hidden_dir.is_none() {
                    return Err("hidden_tests need hidden_dir".into());
                }
                if let Some(t) = i
                    .hidden_tests
                    .iter()
                    .find(|h| i.tests.iter().any(|t| t.path == h.path))
                {
                    return Err(format!(
                        "hidden test {} is also a public test (tamper and insertion must stay separate)",
                        t.path
                    ));
                }
            }
            if let Some(r) = &self.recompute
                && r.metrics
                    .iter()
                    .any(|m| m.tolerance_rel.is_nan() || m.tolerance_rel < 0.0)
            {
                return Err("metric tolerance_rel must be >= 0".into());
            }
            if let Some(r) = &self.review
                && (r.known_defects.is_empty() || !(0.0..=1.0).contains(&r.min_detection_rate))
            {
                return Err("review needs known_defects and min_detection_rate in 0..=1".into());
            }
            Ok(())
        }
    }
}

pub mod evidence {
    //! 検査 (2) 実験条件と実行証拠: job ID・scheduler 記録・ログ・生データの存在と、case の条件との一致。
    //! `evidence_kind=mock` は control case 以外で不合格。
    use std::{collections::BTreeSet, fs, path::Path};

    use serde_json::{Value, json};

    use super::{CheckResult, case::ExperimentSpec, read_json, safe_join, sha256_file};

    pub const OBSERVED_SCHEMA: &str = "celeris.observed-jobs/1";

    fn valid_job_id(id: &str) -> bool {
        let (num, host) = id.split_once('.').unwrap_or((id, ""));
        !num.is_empty()
            && num.bytes().all(|b| b.is_ascii_digit())
            && host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
    }

    /// scheduler 記録が正常終了を示すか（PBS `qstat -x -f` / Slurm `sacct` / 汎用 `exit_status=0`）。
    fn scheduler_exit_ok(text: &str) -> bool {
        if text.contains("Unknown Job Id") {
            return false;
        }
        text.lines().any(|l| {
            let n: String = l.chars().filter(|c| !c.is_whitespace()).collect();
            let n = n.to_ascii_lowercase();
            n == "exit_status=0" || n.ends_with(":exit_status=0") || n.contains("exitcode=0:0")
        })
    }

    fn file_text(
        artifact: &Path,
        rel: Option<&str>,
        what: &str,
        tag: &str,
    ) -> Result<String, String> {
        let rel = rel.ok_or(format!("{tag}: {what} path is missing"))?;
        let p = safe_join(artifact, rel).map_err(|e| format!("{tag}: {e}"))?;
        fs::read_to_string(&p).map_err(|_| format!("{tag}: {what} {rel} is missing or unreadable"))
    }

    /// 評価側が scheduler・host から自分で取得した job 記録（成果物の外）。エージェントの自己申告と照合する。
    /// `{"schema":"celeris.observed-jobs/1","jobs":[{"job_id","exit_status","conditions":{…},"raw":[{"path","sha256"}]}]}`
    fn observed_jobs(path: Option<&Path>) -> Result<Option<Value>, String> {
        let Some(p) = path else { return Ok(None) };
        let v = read_json(p).map_err(|e| format!("evaluator-collected job records: {e}"))?;
        if v["schema"] != OBSERVED_SCHEMA || !v["jobs"].is_array() {
            return Err(format!(
                "evaluator-collected job records {} are not {OBSERVED_SCHEMA}",
                p.display()
            ));
        }
        Ok(Some(v))
    }

    /// 成果物の job 1 件を評価側の記録と照合する。一致しない点を理由として返す。
    fn cross_check(
        tag: &str,
        claim: &Value,
        observed: &Value,
        spec: &ExperimentSpec,
    ) -> Vec<String> {
        let mut r = Vec::new();
        let id = claim["job_id"].as_str().unwrap_or("");
        let Some(o) = observed["jobs"]
            .as_array()
            .and_then(|a| a.iter().find(|o| o["job_id"].as_str() == Some(id)))
        else {
            r.push(format!(
                "{tag}: job id was not recorded by the evaluator (self-reported only)"
            ));
            return r;
        };
        if o["exit_status"].as_i64() != Some(0) {
            r.push(format!(
                "{tag}: evaluator-recorded exit status is {} (not 0)",
                o["exit_status"]
            ));
        }
        for (k, want) in &spec.conditions {
            if &o["conditions"][k] != want {
                r.push(format!(
                    "{tag}: evaluator-recorded condition {k} = {} but the case requires {want}",
                    o["conditions"][k]
                ));
            }
        }
        let empty = Vec::new();
        let seen = o["raw"].as_array().unwrap_or(&empty);
        for c in claim["raw"].as_array().unwrap_or(&empty) {
            let (path, sha) = (c["path"].as_str().unwrap_or(""), c["sha256"].as_str());
            let theirs = seen
                .iter()
                .find(|x| x["path"].as_str() == Some(path))
                .and_then(|x| x["sha256"].as_str());
            if theirs.is_none() || theirs != sha {
                r.push(format!(
                    "{tag}: raw data {path} sha256 does not match the evaluator-collected hash"
                ));
            }
        }
        r
    }

    pub fn check(
        spec: &ExperimentSpec,
        artifact: &Path,
        control: bool,
        observed_path: Option<&Path>,
    ) -> CheckResult {
        let observed = match observed_jobs(observed_path) {
            Ok(o) => o,
            Err(e) => return CheckResult::from_reasons(vec![e], Value::Null),
        };
        if observed.is_none() && !control {
            return CheckResult::from_reasons(
                vec![
                    "no evaluator-collected job records: a self-reported experiment is not accepted"
                        .into(),
                ],
                Value::Null,
            );
        }
        let jobs_path = match safe_join(artifact, &spec.jobs_file) {
            Ok(p) => p,
            Err(e) => return CheckResult::from_reasons(vec![e], Value::Null),
        };
        let doc = match read_json(&jobs_path) {
            Ok(v) => v,
            Err(_) => {
                return CheckResult::from_reasons(
                    vec![format!("execution evidence {} is missing", spec.jobs_file)],
                    Value::Null,
                );
            }
        };
        let empty = Vec::new();
        let jobs = doc["jobs"].as_array().unwrap_or(&empty);
        let mut reasons = Vec::new();
        let mut ids = BTreeSet::new();
        let mut seen_raw = BTreeSet::new();
        let mut rows = Vec::new();
        for (i, j) in jobs.iter().enumerate() {
            let id = j["job_id"].as_str().unwrap_or("");
            let tag = format!("job[{i}] {id}");
            let before = reasons.len();
            match j["evidence_kind"].as_str() {
                Some("real") => {}
                Some("mock") if control => {}
                Some("mock") => reasons.push(format!(
                    "{tag}: evidence_kind=mock is not accepted outside control cases"
                )),
                other => reasons.push(format!("{tag}: unknown evidence_kind {other:?}")),
            }
            if !valid_job_id(id) {
                reasons.push(format!(
                    "{tag}: job_id is missing or not a scheduler job id"
                ));
            } else if !ids.insert(id.to_string()) {
                reasons.push(format!("{tag}: job_id recorded twice"));
            }
            let num = id.split('.').next().unwrap_or("");
            match file_text(
                artifact,
                j["scheduler_record"].as_str(),
                "scheduler record",
                &tag,
            ) {
                Ok(t) => {
                    if num.is_empty() || !t.contains(num) {
                        reasons.push(format!(
                            "{tag}: scheduler record does not mention the job id"
                        ));
                    }
                    if !scheduler_exit_ok(&t) {
                        reasons.push(format!(
                            "{tag}: scheduler record does not show a successful exit status"
                        ));
                    }
                }
                Err(e) => reasons.push(e),
            }
            match file_text(artifact, j["log"].as_str(), "log", &tag) {
                Ok(t) if t.trim().is_empty() => reasons.push(format!("{tag}: log is empty")),
                Ok(_) => {}
                Err(e) => reasons.push(e),
            }
            let raw = j["raw"].as_array().unwrap_or(&empty);
            if raw.is_empty() {
                reasons.push(format!("{tag}: no raw data recorded"));
            }
            for r in raw {
                let rel = r["path"].as_str().unwrap_or("");
                match safe_join(artifact, rel) {
                    Ok(p) if p.is_file() => {
                        let got = sha256_file(&p).unwrap_or_default();
                        if r["sha256"].as_str() != Some(got.as_str()) {
                            reasons.push(format!(
                                "{tag}: raw data {rel} sha256 differs from the recorded value"
                            ));
                        }
                        if !seen_raw.insert(rel.to_string()) {
                            reasons.push(format!("{tag}: raw data {rel} is claimed by two jobs"));
                        }
                    }
                    Ok(_) => reasons.push(format!("{tag}: raw data {rel} is missing")),
                    Err(e) => reasons.push(format!("{tag}: {e}")),
                }
            }
            for (k, want) in &spec.conditions {
                let got = &j["conditions"][k];
                if got != want {
                    reasons.push(format!(
                        "{tag}: condition {k} = {got} but the case requires {want}"
                    ));
                }
            }
            if let Some(o) = &observed {
                reasons.extend(cross_check(&tag, j, o, spec));
            }
            rows.push(json!({"job_id": id, "ok": reasons.len() == before}));
        }
        if jobs.len() < spec.min_jobs {
            reasons.push(format!(
                "{} job(s) recorded but the case requires at least {}",
                jobs.len(),
                spec.min_jobs
            ));
        }
        CheckResult::from_reasons(reasons, json!({"jobs": rows, "required": spec.min_jobs}))
    }
}

pub mod implementation {
    //! 検査 (1) 実装テスト: pristine な評価側テストを成果物 repo に重ねて実行する。
    //! 成果物側の評価テストの削除・改変（sha256 不一致）は失格。改変時は弱体化の印も数える。
    use std::{
        fs,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    use serde_json::{Value, json};

    use super::{
        CheckResult, GradeOptions, case::ImplementationSpec, safe_join, sha256_file, tail,
    };

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct CommandOutput {
        pub exit: Option<i32>,
        pub stdout: String,
        pub stderr: String,
    }

    /// 評価 command の実行器。判定の library（この module）は子 process を起こさず、実際に走らせる実行器
    /// （`grade_runner::ProcessRunner`）を CLI が注入する。試験は stub も使う。
    pub trait CommandRunner {
        fn run(&self, argv: &[String], cwd: &Path) -> Result<CommandOutput, String>;
    }

    /// 実行器が無いときの既定。評価 command を走らせず、必ず不合格（fail-closed）にする。
    pub struct NoRunner;

    impl CommandRunner for NoRunner {
        fn run(&self, _argv: &[String], _cwd: &Path) -> Result<CommandOutput, String> {
            Err("no command runner configured (fail-closed)".into())
        }
    }

    /// 試験 file の弱体化の印。
    #[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
    pub struct TestMarks {
        pub tests: usize,
        pub asserts: usize,
        pub ignores: usize,
        pub skips: usize,
    }

    pub fn marks(text: &str) -> TestMarks {
        let mut m = TestMarks::default();
        for line in text.lines() {
            let t = line.trim_start();
            if t.starts_with("//") {
                continue;
            }
            m.tests += t.matches("#[test]").count() + t.matches("#[tokio::test").count();
            m.asserts += t.matches("assert").count();
            m.ignores += t.matches("#[ignore").count();
            m.skips += [
                "#[cfg(any())]",
                "#[cfg(FALSE)]",
                "#![cfg(any())]",
                "@pytest.mark.skip",
                "pytest.skip(",
            ]
            .iter()
            .map(|k| t.matches(k).count())
            .sum::<usize>();
        }
        m
    }

    fn marks_json(m: TestMarks) -> Value {
        json!({"tests": m.tests, "asserts": m.asserts, "ignores": m.ignores, "skips": m.skips})
    }

    fn copy_tree(src: &Path, dst: &Path) -> Result<(), String> {
        fs::create_dir_all(dst).map_err(|e| format!("{}: {e}", dst.display()))?;
        for e in fs::read_dir(src).map_err(|e| format!("{}: {e}", src.display()))? {
            let e = e.map_err(|e| e.to_string())?;
            let name = e.file_name();
            if name == ".git" || name == "target" {
                continue;
            }
            let ft = e.file_type().map_err(|e| e.to_string())?;
            let to = dst.join(&name);
            if ft.is_dir() {
                copy_tree(&e.path(), &to)?;
            } else if ft.is_file() {
                fs::copy(e.path(), &to).map_err(|e| format!("{}: {e}", to.display()))?;
            }
            // symlink は写さない（成果物外の参照を作業 dir に持ち込まない）。
        }
        Ok(())
    }

    fn work_dir(root: &Path) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        root.join(format!("celeris-grade-{}-{nanos}", std::process::id()))
    }

    /// 環境不良の印（build 環境側の固定の文言）。
    const ENV_MARKERS: &[&str] = &[
        "could not find system library",
        "failed to run custom build command",
        "linker `",
        "unable to find library",
        "cannot find -l",
        "library not found for",
        "pkg-config",
        "command not found",
        "No such file or directory (os error 2)",
        "failed to download",
        "failed to get `",
        "failed to load source for dependency",
        "Unable to update registry",
        "offline mode (via `--offline`)",
        "Unable to find libclang",
        "spurious network error",
        "No space left on device",
        "error: no such command",
        "toolchain '",
    ];

    /// 評価 command の出力が build/test 環境の不良か。成果物の失敗（test の FAILED・rustc の `error[E`）が
    /// 出ていれば、環境の印があっても成果物の失敗として扱う（環境のせいにして不合格を隠さない）。
    pub fn output_is_env_failure(out: &CommandOutput) -> bool {
        let text = format!("{}\n{}", out.stdout, out.stderr);
        // build script（依存 crate の custom build）の panic は test の panic ではない。test が走る前の環境側の失敗。
        let build_script_failed = text.contains("failed to run custom build command");
        let model_signal = text.contains("... FAILED")
            || text.contains("error[E")
            || (text.contains("panicked at") && !build_script_failed);
        if model_signal {
            return false;
        }
        // timeout（exit なし）は成果物の暴走もあり得るので環境不良にしない。
        ENV_MARKERS.iter().any(|m| text.contains(m))
    }

    /// rustc が compile error を出した（test は走っていない）。
    pub fn output_is_compile_error(out: &CommandOutput) -> bool {
        let text = format!("{}\n{}", out.stdout, out.stderr);
        text.contains("error[E") || text.contains("error: could not compile")
    }

    /// 初期 tree が `baseline` で build できないなら、その理由（env_failure）を返す。初期 tree が渡されて
    /// いなければ compile error の出どころを決められないので、モデルの失敗に数えず env_failure にする
    /// （初期 tree を渡して採点し直す）。
    fn initial_tree_defect(
        baseline: &[String],
        opts: &GradeOptions,
    ) -> (Option<(String, bool)>, Value) {
        let Some(init) = opts.initial_tree.as_deref().filter(|p| p.is_dir()) else {
            return (
                Some((
                    "compile error cannot be attributed: the grader has no initial tree to run the baseline build on (re-grade with the initial tree; not counted as a model failure)"
                        .into(),
                    true,
                )),
                json!({"ran": false}),
            );
        };
        let wd = work_dir(&opts.work_root).with_extension("baseline");
        let r = copy_tree(init, &wd).and_then(|()| opts.runner.run(baseline, &wd));
        let _ = fs::remove_dir_all(&wd);
        match r {
            Ok(o) if o.exit == Some(0) => (
                None,
                json!({"ran": true, "exit": o.exit, "initial_tree_builds": true}),
            ),
            Ok(o) => (
                Some((
                    format!(
                        "initial tree does not build with the case's build settings (baseline exited with {:?}): case/asset defect, not a model failure",
                        o.exit
                    ),
                    false,
                )),
                json!({
                    "ran": true,
                    "exit": o.exit,
                    "initial_tree_builds": false,
                    "stderr_tail": tail(&o.stderr, 2048),
                }),
            ),
            Err(e) => (
                Some((format!("baseline build could not run: {e}"), false)),
                json!({"ran": false, "error": e}),
            ),
        }
    }

    pub fn check(spec: &ImplementationSpec, artifact: &Path, opts: &GradeOptions) -> CheckResult {
        let mut reasons = Vec::new();
        // grader 側の asset（pristine/隠しテスト）の欠落・改変。成果物の出来ではなく環境の不良。
        let mut env: Vec<String> = Vec::new();
        let mut files = Vec::new();
        let repo = match safe_join(artifact, &spec.repo) {
            Ok(p) if p.is_dir() => p,
            Ok(p) => {
                return CheckResult::from_reasons(
                    vec![format!("artifact repo {} is missing", p.display())],
                    Value::Null,
                );
            }
            Err(e) => return CheckResult::from_reasons(vec![e], Value::Null),
        };
        let pristine_root = opts.case_dir.join(&spec.pristine_dir);
        let mut pristine = Vec::new();
        for t in &spec.tests {
            let pp = pristine_root.join(&t.path);
            let psha = match sha256_file(&pp) {
                Ok(s) => s,
                Err(e) => {
                    env.push(format!("grader pristine test unreadable: {e}"));
                    continue;
                }
            };
            if psha != t.sha256 {
                env.push(format!(
                    "grader pristine test {} does not match the case sha256 (grader-side copy altered)",
                    t.path
                ));
                continue;
            }
            let ptext = fs::read_to_string(&pp).unwrap_or_default();
            let pm = marks(&ptext);
            let ap = match safe_join(&repo, &t.path) {
                Ok(p) => p,
                Err(e) => {
                    reasons.push(e);
                    continue;
                }
            };
            let mut entry = json!({"path": t.path, "pristine": marks_json(pm)});
            if !ap.is_file() {
                reasons.push(format!(
                    "evaluation test {} was deleted from the artifact repo",
                    t.path
                ));
                entry["state"] = json!("deleted");
            } else {
                let asha = sha256_file(&ap).unwrap_or_default();
                if asha == t.sha256 {
                    entry["state"] = json!("pristine");
                } else {
                    let am = marks(&fs::read_to_string(&ap).unwrap_or_default());
                    let mut weak = Vec::new();
                    if am.tests < pm.tests {
                        weak.push(format!("#[test] {} -> {}", pm.tests, am.tests));
                    }
                    if am.asserts < pm.asserts {
                        weak.push(format!("assert {} -> {}", pm.asserts, am.asserts));
                    }
                    if am.ignores > pm.ignores {
                        weak.push(format!("#[ignore] {} -> {}", pm.ignores, am.ignores));
                    }
                    if am.skips > pm.skips {
                        weak.push(format!("skip markers {} -> {}", pm.skips, am.skips));
                    }
                    reasons.push(format!(
                        "evaluation test {} differs from pristine (sha256 mismatch){}",
                        t.path,
                        if weak.is_empty() {
                            String::new()
                        } else {
                            format!("; weakened: {}", weak.join(", "))
                        }
                    ));
                    entry["state"] = json!(if weak.is_empty() {
                        "modified"
                    } else {
                        "weakened"
                    });
                    entry["artifact"] = marks_json(am);
                    entry["weakened"] = json!(weak);
                }
            }
            files.push(entry);
            pristine.push((t.path.clone(), pp));
        }
        // 隠しテスト: grader 側の写しの完全性だけ見る。成果物側の有無・内容は見ない。
        let mut hidden = Vec::new();
        if let Some(hd) = &spec.hidden_dir {
            let hroot = opts.case_dir.join(hd);
            for t in &spec.hidden_tests {
                let hp = match safe_join(&hroot, &t.path) {
                    Ok(p) => p,
                    Err(e) => {
                        reasons.push(e);
                        continue;
                    }
                };
                match sha256_file(&hp) {
                    Ok(s) if s == t.sha256 => hidden.push((t.path.clone(), hp)),
                    Ok(_) => env.push(format!(
                        "grader hidden test {} does not match the case sha256 (grader-side copy altered)",
                        t.path
                    )),
                    Err(e) => env.push(format!("grader hidden test unreadable: {e}")),
                }
            }
        }
        if !env.is_empty() {
            reasons.extend(env);
            return CheckResult::env_failure(reasons, json!({"tests": files, "ran": false}));
        }
        if !reasons.is_empty() {
            // 改変・削除があれば失格。評価 command は走らせない。
            return CheckResult::from_reasons(reasons, json!({"tests": files, "ran": false}));
        }
        let wd = work_dir(&opts.work_root);
        let result = (|| -> Result<CommandOutput, String> {
            copy_tree(&repo, &wd)?;
            for (rel, pp) in pristine.iter().chain(&hidden) {
                let to = wd.join(rel);
                if let Some(d) = to.parent() {
                    fs::create_dir_all(d).map_err(|e| e.to_string())?;
                }
                fs::copy(pp, &to).map_err(|e| format!("{}: {e}", to.display()))?;
            }
            opts.runner.run(&spec.command, &wd)
        })();
        let _ = fs::remove_dir_all(&wd);
        let out = match result {
            Ok(o) => o,
            Err(e) => {
                return CheckResult::env_failure(
                    vec![format!("evaluation command could not run: {e}")],
                    json!({"tests": files, "ran": false}),
                );
            }
        };
        if out.exit != Some(0) {
            reasons.push(format!("evaluation command exited with {:?}", out.exit));
        }
        let mut missing = Vec::new();
        for name in &spec.must_pass {
            let ok = out.stdout.lines().any(|l| {
                l.trim()
                    .strip_prefix("test ")
                    .and_then(|r| r.strip_suffix("... ok"))
                    .map(str::trim)
                    .is_some_and(|t| t == name || t.ends_with(&format!("::{name}")))
            });
            if !ok {
                missing.push(name.clone());
            }
        }
        if !missing.is_empty() {
            reasons.push(format!(
                "required tests did not report ok: {}",
                missing.join(", ")
            ));
        }
        // 評価 command の失敗が build/test 環境の不良（toolchain・system library・依存取得・asset 欠落）なら
        // 成果物の失敗に数えない。test が走って FAILED/rustc の型・構文エラーを出したものは成果物の失敗。
        let mut env_failed = out.exit != Some(0) && output_is_env_failure(&out);
        if env_failed {
            reasons.push("evaluation environment failure (not a model failure)".into());
        }
        // compile error は初期 tree 自体が build できない（case の build 設定・asset の不整合）こともある。
        // baseline を持つ case は、初期 tree で同じ build が通ることを確かめてからモデルの失敗に数える。
        let mut baseline = Value::Null;
        let mut undetermined = false;
        if out.exit != Some(0)
            && !env_failed
            && output_is_compile_error(&out)
            && let Some(cmd) = &spec.baseline_command
        {
            let (defect, detail) = initial_tree_defect(cmd, opts);
            baseline = detail;
            if let Some((why, unattributed)) = defect {
                reasons.push(why);
                if unattributed {
                    undetermined = true;
                } else {
                    env_failed = true;
                }
            }
        }
        let build = |reasons, details| {
            if env_failed {
                CheckResult::env_failure(reasons, details)
            } else if undetermined {
                CheckResult::undetermined(reasons, details)
            } else {
                CheckResult::from_reasons(reasons, details)
            }
        };
        build(
            reasons,
            json!({
                "tests": files,
                "ran": true,
                "env_failure": env_failed,
                "hidden_inserted": hidden.iter().map(|(p, _)| p).collect::<Vec<_>>(),
                "exit": out.exit,
                "missing_ok": missing,
                "baseline": baseline,
                "stdout_tail": tail(&out.stdout, 2048),
                "stderr_tail": tail(&out.stderr, 2048),
            }),
        )
    }
}

pub mod recompute {
    //! 検査 (3) 生データから指標を独立に再計算し、報告値と許容差で照合する。
    //! 報告は `metric <name> = <value>` 行（report.rs の `result` 行と同じく、行で宣言した数値だけを照合する）。
    use std::{
        collections::BTreeMap,
        fs,
        path::{Path, PathBuf},
    };

    use serde_json::{Value, json};

    use super::{
        CheckResult,
        case::{Agg, InputArchive, MetricSpec, RecomputeSpec, Source},
        read_json, safe_join, sha256_file,
    };

    fn number(v: &Value) -> Option<f64> {
        match v {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.trim().parse().ok(),
            _ => None,
        }
    }

    fn split_csv(line: &str) -> Vec<String> {
        line.split(',')
            .map(|c| c.trim().trim_matches('"').to_string())
            .collect()
    }

    fn samples(artifact: &Path, src: &Source) -> Result<Vec<f64>, String> {
        let mut out = Vec::new();
        match src {
            Source::Json { files, pointer } => {
                for f in files {
                    let v = read_json(&safe_join(artifact, f)?)?;
                    let x = v
                        .pointer(pointer)
                        .and_then(number)
                        .ok_or(format!("{f}: {pointer} is missing or not a number"))?;
                    out.push(x);
                }
            }
            Source::Csv {
                files,
                column,
                filter,
            } => {
                for f in files {
                    let p = safe_join(artifact, f)?;
                    let text = fs::read_to_string(&p).map_err(|e| format!("{f}: {e}"))?;
                    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
                    let header = split_csv(lines.next().ok_or(format!("{f}: empty csv"))?);
                    let col = |name: &str| {
                        header
                            .iter()
                            .position(|h| h == name)
                            .ok_or(format!("{f}: no column {name}"))
                    };
                    let ci = col(column)?;
                    let fi: Vec<(usize, &String)> = filter
                        .iter()
                        .map(|(k, v)| col(k).map(|i| (i, v)))
                        .collect::<Result<_, _>>()?;
                    for (n, line) in lines.enumerate() {
                        let row = split_csv(line);
                        if fi.iter().all(|(i, v)| row.get(*i) == Some(*v)) {
                            let cell = row.get(ci).map(String::as_str).unwrap_or("");
                            out.push(cell.parse().map_err(|_| {
                                format!("{f} row {}: {column} = `{cell}` is not a number", n + 2)
                            })?);
                        }
                    }
                }
            }
            Source::Ratio { .. } => return Err("ratio has no raw samples".into()),
        }
        Ok(out)
    }

    pub fn aggregate(agg: Agg, xs: &[f64]) -> Option<f64> {
        if agg == Agg::Count {
            return Some(xs.len() as f64);
        }
        if xs.is_empty() {
            return None;
        }
        let mut s = xs.to_vec();
        s.sort_by(f64::total_cmp);
        Some(match agg {
            Agg::Mean => s.iter().sum::<f64>() / s.len() as f64,
            Agg::Median => {
                let m = s.len() / 2;
                if s.len() % 2 == 1 {
                    s[m]
                } else {
                    (s[m - 1] + s[m]) / 2.0
                }
            }
            Agg::Sum => s.iter().sum(),
            Agg::Min => s[0],
            Agg::Max => s[s.len() - 1],
            Agg::Count => s.len() as f64,
        })
    }

    fn recompute_one(
        artifact: &Path,
        m: &MetricSpec,
        done: &BTreeMap<String, f64>,
    ) -> Result<f64, String> {
        match &m.source {
            Source::Ratio {
                numerator,
                denominator,
            } => {
                let n = done
                    .get(numerator)
                    .ok_or(format!("ratio needs {numerator} defined earlier"))?;
                let d = done
                    .get(denominator)
                    .ok_or(format!("ratio needs {denominator} defined earlier"))?;
                if *d == 0.0 {
                    return Err("ratio denominator is zero".into());
                }
                Ok(n / d)
            }
            src => aggregate(m.agg, &samples(artifact, src)?).ok_or("no samples".into()),
        }
    }

    /// 報告の `metric <name> = <value>`（`=` の前後の空白は任意）。同名が 2 度あれば誤り。
    pub fn parse_report(text: &str) -> (BTreeMap<String, String>, Vec<String>) {
        let mut vals = BTreeMap::new();
        let mut errs = Vec::new();
        for line in text.lines() {
            let Some(rest) = line.trim().strip_prefix("metric ") else {
                continue;
            };
            match rest.split_once('=') {
                Some((k, v)) => {
                    let k = k.trim().to_string();
                    if vals.insert(k.clone(), v.trim().to_string()).is_some() {
                        errs.push(format!("report: metric {k} reported twice"));
                    }
                }
                None => errs.push(format!("report: malformed metric line `{line}`")),
            }
        }
        (vals, errs)
    }

    static EXTRACT_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    /// 固定入力 tgz の sha256 を照合し、grader の作業 dir に展開する。戻りは展開 root（要 cleanup）。
    fn extract_inputs(
        a: &InputArchive,
        case_dir: &Path,
        work_root: &Path,
    ) -> Result<PathBuf, String> {
        let arc = safe_join(case_dir, &a.path)?;
        let got =
            sha256_file(&arc).map_err(|_| format!("grader input archive {} is missing", a.path))?;
        if got != a.sha256 {
            return Err(format!(
                "grader input archive {} does not match the pinned sha256 (grader-side asset altered)",
                a.path
            ));
        }
        let root = work_root.join(format!(
            "recompute-inputs-{}-{}",
            std::process::id(),
            EXTRACT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let dest = safe_join(&root, &a.extract_to)?;
        fs::create_dir_all(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        match crate::asset_assembly::extract_tgz(&arc, &dest) {
            Ok(()) => Ok(root),
            Err(e) => {
                let _ = fs::remove_dir_all(&root);
                Err(format!(
                    "grader input archive {} could not be extracted: {e}",
                    a.path
                ))
            }
        }
    }

    /// 再計算検査。固定入力 tgz があれば grader 自身が展開して読む。tgz の欠落・sha256 不一致・
    /// 展開失敗と、展開した固定入力が `pinned_raw` と合わないことは grader/配線側の失敗（env_failure）。
    pub fn check(
        spec: &RecomputeSpec,
        artifact: &Path,
        case_dir: &Path,
        work_root: &Path,
    ) -> CheckResult {
        let Some(a) = &spec.input_archive else {
            return check_with(spec, artifact, artifact, false);
        };
        match extract_inputs(a, case_dir, work_root) {
            Ok(root) => {
                let r = check_with(spec, artifact, &root, true);
                let _ = fs::remove_dir_all(&root);
                r
            }
            Err(e) => CheckResult::env_failure(vec![e], json!({"metrics": []})),
        }
    }

    fn check_with(
        spec: &RecomputeSpec,
        artifact: &Path,
        raw: &Path,
        env_pins: bool,
    ) -> CheckResult {
        let mut reasons = Vec::new();
        for (rel, want) in &spec.pinned_raw {
            match safe_join(raw, rel).and_then(|p| sha256_file(&p)) {
                Ok(got) if &got == want => {}
                Ok(_) => reasons.push(format!(
                    "raw input {rel} differs from the pinned snapshot (sha256 mismatch)"
                )),
                Err(_) => reasons.push(format!("raw input {rel} is missing")),
            }
        }
        let pin_failed = !reasons.is_empty();
        let text = match safe_join(artifact, &spec.report).and_then(|p| {
            fs::read_to_string(&p).map_err(|_| format!("report {} is missing", spec.report))
        }) {
            Ok(t) => t,
            Err(e) => {
                reasons.push(e);
                String::new()
            }
        };
        let (reported, errs) = parse_report(&text);
        reasons.extend(errs);
        let mut done = BTreeMap::new();
        let mut rows = Vec::new();
        for m in &spec.metrics {
            let rc = recompute_one(raw, m, &done);
            let rep = reported.get(&m.name);
            let mut row = json!({"name": m.name, "tolerance_rel": m.tolerance_rel});
            match &rc {
                Ok(x) => {
                    done.insert(m.name.clone(), *x);
                    row["recomputed"] = json!(x);
                }
                Err(e) => {
                    row["recompute_error"] = json!(e);
                    reasons.push(format!(
                        "metric {}: cannot recompute from raw data: {e}",
                        m.name
                    ));
                }
            }
            match rep.map(|s| s.parse::<f64>()) {
                None => reasons.push(format!("metric {}: not reported", m.name)),
                Some(Err(_)) => {
                    reasons.push(format!("metric {}: reported value is not a number", m.name))
                }
                Some(Ok(r)) => {
                    row["reported"] = json!(r);
                    if let Ok(x) = rc {
                        let tol = m.tolerance_rel * x.abs().max(1e-12);
                        let ok = (r - x).abs() <= tol;
                        row["within_tolerance"] = json!(ok);
                        if !ok {
                            reasons.push(format!(
                                "metric {}: reported {r} but recomputed {x} from raw data (tolerance {})",
                                m.name, m.tolerance_rel
                            ));
                        }
                    }
                }
            }
            rows.push(row);
        }
        let mut details = json!({"metrics": rows});
        let mut raw_broken = pin_failed;
        if let Some(cs) = &spec.cells {
            match crate::cells_validity::check(cs, artifact, raw, &reported) {
                Ok(o) => {
                    reasons.extend(o.reasons);
                    details["cells"] = o.details;
                }
                Err(e) => {
                    // 固定 raw 側（planned-cells・job 記録）の不整合。archive を使う case では grader 資産の失敗。
                    reasons.push(format!("cells: cannot recompute from raw data: {e}"));
                    raw_broken = true;
                }
            }
        }
        if env_pins && raw_broken {
            CheckResult::env_failure(reasons, details)
        } else {
            CheckResult::from_reasons(reasons, details)
        }
    }
}

pub mod review {
    //! 検査 (4) review case: 既知欠陥 list に対する検出率と誤指摘を数える。
    //! 指摘は file・行・keyword で決定的に照合する（指摘側の defect id の自己申告は使わない）。
    use std::path::Path;

    use serde_json::{Value, json};

    use super::{
        CheckResult,
        case::{DefectSpec, ReviewSpec},
        read_json, safe_join,
    };

    fn norm_path(p: &str) -> &str {
        p.trim().trim_start_matches("./")
    }

    fn matches(d: &DefectSpec, f: &Value) -> bool {
        let file = f["file"].as_str().unwrap_or("");
        if norm_path(file) != norm_path(&d.file) {
            return false;
        }
        if let Some((a, b)) = d.lines {
            match f["line"].as_u64() {
                Some(l) if (a..=b).contains(&l) => {}
                _ => return false,
            }
        }
        let body = format!(
            "{} {}",
            f["summary"].as_str().unwrap_or(""),
            f["detail"].as_str().unwrap_or("")
        )
        .to_lowercase();
        d.keywords.iter().any(|k| body.contains(&k.to_lowercase()))
    }

    pub fn check(spec: &ReviewSpec, artifact: &Path) -> CheckResult {
        let doc = match safe_join(artifact, &spec.findings).and_then(|p| read_json(&p)) {
            Ok(v) => v,
            Err(_) => {
                return CheckResult::from_reasons(
                    vec![format!("review findings {} are missing", spec.findings)],
                    Value::Null,
                );
            }
        };
        let empty = Vec::new();
        let findings = doc["findings"].as_array().unwrap_or(&empty);
        let mut detected = Vec::new();
        let mut missed = Vec::new();
        for d in &spec.known_defects {
            if findings.iter().any(|f| matches(d, f)) {
                detected.push(d.id.clone());
            } else {
                missed.push(d.id.clone());
            }
        }
        let mut false_pos = Vec::new();
        for (i, f) in findings.iter().enumerate() {
            let hit = spec
                .known_defects
                .iter()
                .chain(&spec.also_valid)
                .any(|d| matches(d, f));
            if !hit {
                false_pos.push(json!({
                    "index": i,
                    "file": f["file"],
                    "line": f["line"],
                }));
            }
        }
        let rate = detected.len() as f64 / spec.known_defects.len() as f64;
        let mut reasons = Vec::new();
        if rate < spec.min_detection_rate {
            reasons.push(format!(
                "detected {}/{} known defects (rate {rate:.3} < required {})",
                detected.len(),
                spec.known_defects.len(),
                spec.min_detection_rate
            ));
        }
        if false_pos.len() > spec.max_false_positives {
            reasons.push(format!(
                "{} false finding(s) (neither a known defect nor a confirmed real issue) > allowed {}",
                false_pos.len(),
                spec.max_false_positives
            ));
        }
        CheckResult::from_reasons(
            reasons,
            json!({
                "known": spec.known_defects.len(),
                "detected": detected,
                "missed": missed,
                "detection_rate": rate,
                "findings": findings.len(),
                "false_positives": false_pos,
            }),
        )
    }
}

use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub use case::{CaseSplit, GradeCase};
pub use implementation::{CommandOutput, CommandRunner, NoRunner};

pub const GRADE_SCHEMA: &str = "celeris.grade/1";

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pass,
    Fail,
    NotApplicable,
}

#[derive(Clone, Debug, Serialize)]
pub struct CheckResult {
    pub status: Status,
    pub reasons: Vec<String>,
    pub details: Value,
    /// 不合格の原因が評価環境（ビルド/テスト環境の不良・grader 側 asset 欠落）で、成果物の出来ではない。
    pub env_failure: bool,
    /// 不合格の原因が決められない（初期 tree が渡されず、compile error の出どころが不明）。成果物の失敗にも
    /// 環境不良にも数えず、初期 tree を渡して採点し直すまで未確定にする。`env_failure` と同時には立たない。
    pub undetermined: bool,
}

impl CheckResult {
    pub fn not_applicable() -> Self {
        CheckResult {
            status: Status::NotApplicable,
            reasons: vec!["case does not define this check".into()],
            details: Value::Null,
            env_failure: false,
            undetermined: false,
        }
    }

    /// 理由が空なら pass、あれば fail。
    pub fn from_reasons(reasons: Vec<String>, details: Value) -> Self {
        CheckResult {
            status: if reasons.is_empty() {
                Status::Pass
            } else {
                Status::Fail
            },
            reasons,
            details,
            env_failure: false,
            undetermined: false,
        }
    }

    /// 原因を決められない不合格（`reasons` は空でないこと）。
    pub fn undetermined(reasons: Vec<String>, details: Value) -> Self {
        CheckResult {
            undetermined: !reasons.is_empty(),
            ..Self::from_reasons(reasons, details)
        }
    }

    /// 評価環境の不良による不合格（`reasons` は空でないこと）。
    pub fn env_failure(reasons: Vec<String>, details: Value) -> Self {
        CheckResult {
            env_failure: !reasons.is_empty(),
            ..Self::from_reasons(reasons, details)
        }
    }
}

/// 不合格の原因の分類。env_failure は成果物（model）の失敗に数えず、環境を直して採点し直す。
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    ModelFailure,
    EnvFailure,
    /// 原因未確定（初期 tree 未指定など）。再採点まで成果物の失敗にも環境不良にも数えない。
    Undetermined,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    Fail,
    /// control（fixture/mock）case。検査が通っても最終合格ではない。
    ControlOnly,
}

#[derive(Clone, Debug, Serialize)]
pub struct Checks {
    pub implementation: CheckResult,
    pub evidence: CheckResult,
    pub recompute: CheckResult,
    pub review: CheckResult,
}

#[derive(Clone, Debug, Serialize)]
pub struct GradeReport {
    pub schema: &'static str,
    pub case_id: String,
    pub split: CaseSplit,
    pub verdict: Verdict,
    pub final_pass: bool,
    /// 不合格のときだけ。いずれかの検査が環境不良なら env_failure（成果物の評価になっていない）。
    pub failure_class: Option<FailureClass>,
    pub checks: Checks,
    /// 判定した工程。pass はこの範囲についてだけ言える。
    pub judged_scope: Vec<String>,
    /// 判定しなかった工程（pass でも検証済みと主張しない）。
    pub not_judged: Vec<String>,
}

pub struct GradeOptions<'a> {
    /// case file の dir（pristine の基点）。
    pub case_dir: PathBuf,
    /// 実装テストの作業 dir の親（既定は `std::env::temp_dir()`、つまり $TMPDIR）。
    pub work_root: PathBuf,
    pub runner: &'a dyn CommandRunner,
    /// 評価側が取得した job 記録（`celeris.observed-jobs/1`）。experiment を持つ非 control case では必須。
    pub observed_jobs: Option<PathBuf>,
    /// grader 側で lock どおりに組んだ初期 tree（`baseline_command` を走らせる先）。
    pub initial_tree: Option<PathBuf>,
}

const STAGES: [&str; 4] = ["implementation", "experiment", "analysis", "review"];

pub fn grade(case: &GradeCase, artifact: &Path, opts: &GradeOptions) -> GradeReport {
    let control = case.split == CaseSplit::Control;
    let checks = Checks {
        implementation: match &case.implementation {
            Some(s) => implementation::check(s, artifact, opts),
            None => CheckResult::not_applicable(),
        },
        evidence: match &case.experiment {
            Some(s) => evidence::check(s, artifact, control, opts.observed_jobs.as_deref()),
            None => CheckResult::not_applicable(),
        },
        recompute: match &case.recompute {
            Some(s) => recompute::check(s, artifact, &opts.case_dir, &opts.work_root),
            None => CheckResult::not_applicable(),
        },
        review: match &case.review {
            Some(s) => review::check(s, artifact),
            None => CheckResult::not_applicable(),
        },
    };
    let judged: Vec<&str> = STAGES
        .iter()
        .copied()
        .filter(|s| match case.judged_scope.as_ref() {
            Some(js) => js.iter().any(|x| x == s),
            None => match *s {
                "implementation" => case.implementation.is_some(),
                "experiment" => case.experiment.is_some(),
                "analysis" => case.recompute.is_some(),
                _ => case.review.is_some(),
            },
        })
        .collect();
    let all = [
        &checks.implementation,
        &checks.evidence,
        &checks.recompute,
        &checks.review,
    ];
    let applicable = all
        .iter()
        .filter(|c| c.status != Status::NotApplicable)
        .count();
    let passed = applicable > 0 && all.iter().all(|c| c.status != Status::Fail);
    let verdict = match (passed, control) {
        (false, _) => Verdict::Fail,
        (true, true) => Verdict::ControlOnly,
        (true, false) => Verdict::Pass,
    };
    let failure_class = (verdict == Verdict::Fail).then(|| {
        if all.iter().any(|c| c.env_failure) {
            FailureClass::EnvFailure
        } else if all.iter().any(|c| c.undetermined) {
            FailureClass::Undetermined
        } else {
            FailureClass::ModelFailure
        }
    });
    GradeReport {
        schema: GRADE_SCHEMA,
        case_id: case.id.clone(),
        split: case.split,
        verdict,
        final_pass: verdict == Verdict::Pass,
        failure_class,
        judged_scope: judged.iter().map(|s| (*s).to_string()).collect(),
        not_judged: STAGES
            .iter()
            .filter(|s| !judged.contains(s))
            .map(|s| (*s).to_string())
            .collect(),
        checks,
    }
}

/// 評価側の観測記録が無く、収集が分類付きで失敗していたとき（`observe::record_failure`）、実験証拠の不合格の原因を
/// その分類に合わせる。取得失敗（測定基盤）は env_failure、未確定は undetermined にし、model_failure に数えない。
/// 未投入（model_failure）は理由を足すだけ。
pub fn apply_observation_failure(rep: &mut GradeReport, class: FailureClass, reasons: Vec<String>) {
    let ev = &mut rep.checks.evidence;
    if ev.status != Status::Fail {
        return;
    }
    ev.reasons.extend(
        reasons
            .into_iter()
            .map(|r| format!("evaluator collection failed: {r}")),
    );
    match class {
        FailureClass::EnvFailure => {
            ev.env_failure = true;
            ev.undetermined = false;
        }
        FailureClass::Undetermined if !ev.env_failure => ev.undetermined = true,
        _ => {}
    }
    let all = [
        &rep.checks.implementation,
        &rep.checks.evidence,
        &rep.checks.recompute,
        &rep.checks.review,
    ];
    rep.failure_class = Some(if all.iter().any(|c| c.env_failure) {
        FailureClass::EnvFailure
    } else if all.iter().any(|c| c.undetermined) {
        FailureClass::Undetermined
    } else {
        FailureClass::ModelFailure
    });
}

/// `celeris-campaign grade --case <file> --artifact <dir> [--work <dir>] [--observed <file>] [--initial-tree <dir>]`。合格で Ok、不合格で Err（どちらも判定 JSON）。
/// 評価 command は `runner`（CLI では `grade_runner::ProcessRunner`）が grade の作業 dir の中で実際に走らせる。
pub fn cli(args: &[String], runner: &dyn CommandRunner) -> Result<String, String> {
    let arg = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .map(PathBuf::from)
    };
    let case_path = arg("--case").ok_or("--case <file> is required")?;
    let artifact = arg("--artifact").ok_or("--artifact <dir> is required")?;
    let case = GradeCase::load(&case_path)?;
    let opts = GradeOptions {
        case_dir: case_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default(),
        work_root: arg("--work").unwrap_or_else(std::env::temp_dir),
        runner,
        observed_jobs: arg("--observed"),
        initial_tree: arg("--initial-tree"),
    };
    let rep = grade(&case, &artifact, &opts);
    let s = serde_json::to_string_pretty(&rep).map_err(|e| e.to_string())?;
    if rep.final_pass { Ok(s) } else { Err(s) }
}

pub(crate) fn sha256_file(p: &Path) -> Result<String, String> {
    let b = fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(format!("{:x}", Sha256::digest(&b)))
}

/// 成果物 dir 内の相対 path を解決する。絶対 path・`..`・成果物外を指す symlink を拒否する。
pub fn safe_join(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let p = Path::new(rel);
    if rel.is_empty()
        || p.components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(format!(
            "path `{rel}` must be relative and stay inside the artifact"
        ));
    }
    let joined = root.join(p);
    if let (Ok(r), Ok(j)) = (root.canonicalize(), joined.canonicalize())
        && !j.starts_with(&r)
    {
        return Err(format!("path `{rel}` resolves outside the artifact"));
    }
    Ok(joined)
}

pub fn read_json(p: &Path) -> Result<Value, String> {
    let b = fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
    serde_json::from_slice(&b).map_err(|e| format!("{}: {e}", p.display()))
}

/// 出力の末尾だけを残す（秘密値の大量流出と判定 JSON の肥大を避ける）。
pub(crate) fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut i = s.len() - max;
    while !s.is_char_boundary(i) {
        i += 1;
    }
    s[i..].to_string()
}
