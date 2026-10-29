//! Submit frozen manifest problems as Celeris evaluation trials.
#[cfg(test)]
#[allow(clippy::duplicate_mod)] // 同じ支援 file を試験 crate ごとに読む
#[path = "../../../tests/support/http_sync.rs"]
mod fake_http;

use std::{fs, path::PathBuf, thread, time::Duration};

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    candidates::{CANDIDATES, Candidate},
    manifest::{Manifest, Problem},
};

#[derive(Debug, Serialize)]
struct TrialRequest {
    problem_id: String,
    trial_no: u32,
    source_id: String,
    model_id: String,
    harness_id: String,
    repo: String,
    base_commit: String,
    objective: String,
    check: Check,
}
#[derive(Debug, Serialize)]
struct Check {
    cmd: String,
    timeout_secs: u32,
}
#[derive(Debug, Deserialize)]
struct TrialRef {
    trial_id: String,
    problem_id: String,
    trial_no: u32,
    status: String,
}
#[derive(Debug, Deserialize)]
struct BatchResponse {
    batch_id: String,
    trials: Vec<TrialRef>,
}

pub fn run() -> Result<(), String> {
    run_args(std::env::args().skip(2).collect())
}

pub fn run_args(raw_args: Vec<String>) -> Result<(), String> {
    let mut args = raw_args.into_iter();
    let mut api = None;
    let mut manifest = None;
    let mut candidates = None;
    let mut token_file = None;
    let mut repo = None;
    let mut out = None;
    let mut wait = false;
    let mut poll_ms = 1000u64;
    let mut trial_offset = 0u32;
    while let Some(arg) = args.next() {
        let value = |args: &mut std::vec::IntoIter<String>, flag: &str| {
            args.next()
                .ok_or_else(|| format!("missing value for {flag}"))
        };
        match arg.as_str() {
            "--api" => api = Some(value(&mut args, "--api")?),
            "--manifest" => manifest = Some(value(&mut args, "--manifest")?),
            "--candidates" => candidates = Some(value(&mut args, "--candidates")?),
            "--token-file" => token_file = Some(value(&mut args, "--token-file")?),
            "--repo" => repo = Some(value(&mut args, "--repo")?),
            "--out" => out = Some(value(&mut args, "--out")?),
            "--poll-interval-ms" => {
                poll_ms = value(&mut args, "--poll-interval-ms")?
                    .parse()
                    .map_err(|_| "invalid --poll-interval-ms".to_string())?
            }
            "--trial-offset" => {
                trial_offset = value(&mut args, "--trial-offset")?
                    .parse()
                    .map_err(|_| "invalid --trial-offset".to_string())?
            }
            "--wait" => wait = true,
            _ => return Err(format!("unknown option: {arg}")),
        }
    }
    let api = api.ok_or("missing required option --api")?;
    let manifest_path = PathBuf::from(manifest.ok_or("missing required option --manifest")?);
    let ids = candidates.ok_or("missing required option --candidates")?;
    let manifest: Manifest =
        serde_json::from_slice(&fs::read(manifest_path).map_err(|e| e.to_string())?)
            .map_err(|e| format!("invalid manifest: {e}"))?;
    let repo = repo.unwrap_or_else(|| ".".into());
    let repo = fs::canonicalize(repo)
        .map_err(|e| format!("invalid --repo: {e}"))?
        .to_string_lossy()
        .into_owned();
    let token = token_file
        .map(|p| {
            fs::read_to_string(p)
                .map(|s| s.trim().to_owned())
                .map_err(|e| e.to_string())
        })
        .transpose()?;
    let selected = parse_candidates(&ids)?;
    let trials = make_trials(&manifest, &selected, &repo, trial_offset)?;
    let client = Client::builder().build().map_err(|e| e.to_string())?;
    let endpoint = format!("{}/api/v1/routing-eval/trials", api.trim_end_matches('/'));
    let mut request = client
        .post(&endpoint)
        .json(&json!({"manifest_sha": manifest.sha256, "trials": trials}));
    if let Some(token) = &token {
        request = request.bearer_auth(token);
    }
    let response = request.send().map_err(|e| format!("POST trials: {e}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "POST trials returned {}: {}",
            response.status(),
            response.text().unwrap_or_default()
        ));
    }
    let batch: BatchResponse = response
        .json()
        .map_err(|e| format!("invalid POST response: {e}"))?;
    let mut states: Vec<Value> = batch.trials.iter().map(|t| json!({"trial_id":t.trial_id,"problem_id":t.problem_id,"trial_no":t.trial_no,"status":t.status})).collect();
    if wait {
        loop {
            let mut request = client.get(format!("{endpoint}?batch_id={}", batch.batch_id));
            if let Some(token) = &token {
                request = request.bearer_auth(token);
            }
            let response = request.send().map_err(|e| format!("GET trials: {e}"))?;
            if !response.status().is_success() {
                return Err(format!(
                    "GET trials returned {}: {}",
                    response.status(),
                    response.text().unwrap_or_default()
                ));
            }
            let body: Value = response
                .json()
                .map_err(|e| format!("invalid GET response: {e}"))?;
            let current = body
                .get("trials")
                .and_then(Value::as_array)
                .ok_or("GET response has no trials array")?;
            states = current.iter().map(|v| json!({"trial_id":v.get("trial_id"),"problem_id":v.get("problem_id"),"trial_no":v.get("trial_no"),"status":v.get("status")})).collect();
            if current.iter().all(|v| {
                v.get("status")
                    .and_then(Value::as_str)
                    .is_some_and(is_terminal)
            }) {
                break;
            }
            thread::sleep(Duration::from_millis(poll_ms));
        }
    }
    let summary = json!({"schema":"celeris.routing-eval-run/1","batch_id":batch.batch_id,"manifest_sha":manifest.sha256,"trial_count":states.len(),"trials":states});
    let rendered = serde_json::to_string_pretty(&summary).map_err(|e| e.to_string())?;
    if let Some(path) = out {
        fs::write(path, format!("{rendered}\n")).map_err(|e| e.to_string())?;
    } else {
        println!("{rendered}");
    }
    Ok(())
}

fn parse_candidates(ids: &str) -> Result<Vec<&'static Candidate>, String> {
    let mut result = Vec::new();
    for id in ids.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let candidate = CANDIDATES
            .iter()
            .find(|c| c.model_id == id || format!("{}:{}", c.source_id, c.model_id) == id)
            .ok_or_else(|| format!("unknown candidate: {id}"))?;
        if result.iter().any(|c: &&Candidate| {
            c.model_id == candidate.model_id && c.source_id == candidate.source_id
        }) {
            return Err(format!("duplicate candidate: {id}"));
        }
        result.push(candidate);
    }
    if result.is_empty() {
        return Err("--candidates must name at least one candidate".into());
    }
    Ok(result)
}

fn make_trials(
    manifest: &Manifest,
    candidates: &[&Candidate],
    repo: &str,
    trial_offset: u32,
) -> Result<Vec<TrialRequest>, String> {
    let mut result = Vec::new();
    for c in candidates {
        let problems: Vec<&Problem> = manifest
            .problems
            .iter()
            .filter(|p| c.problem_count != 12 || p.subset_12)
            .collect();
        if problems.is_empty() {
            return Err(format!(
                "manifest has no eligible problems for {}",
                c.model_id
            ));
        }
        for p in problems {
            let base_commit = p
                .base_commit
                .as_deref()
                .ok_or_else(|| format!("problem {} has no base_commit", p.id))?;
            for n in 1..=c.k {
                // 同じ manifest のやり直しは trial_no をずらして新しい行にする（旧い行は消さない）。
                let trial_no = u32::try_from(n)
                    .ok()
                    .and_then(|n| n.checked_add(trial_offset))
                    .ok_or("--trial-offset is too large")?;
                result.push(TrialRequest { problem_id:p.id.clone(),trial_no,source_id:c.source_id.into(),model_id:c.model_id.into(),harness_id:c.harness_id.into(),repo:repo.into(),base_commit:base_commit.into(),objective:format!("This is an evaluation problem. Make the requested repository change. Do not run or request evaluation checks.\n\n{}",p.prompt),check:Check{cmd:p.command.clone(),timeout_secs:600} });
            }
        }
    }
    Ok(result)
}
fn is_terminal(s: &str) -> bool {
    matches!(s, "passed" | "failed" | "error" | "cancelled")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, net::TcpListener};
    #[test]
    fn trial_objective_carries_problem_text_without_check() {
        let mut manifest: Manifest = serde_json::from_value(json!({"schema":"x","version":"1","cutoff":"x","seed":0,"sha256":"abc","problems":[
            {"id":"p1","task_id":"t1","work_unit_id":"w1","created_at":"","genre":"coding","kind":"leaf","command":"SECRET_CHECK","prompt":"Implement the widget","split":"dev","stratum":"x","subset_12":false,"head_commit":null,"base_commit":"abc","features_sha256":"","features":{},"sha256":""},
            {"id":"p2","task_id":"t2","work_unit_id":"w2","created_at":"","genre":"coding","kind":"leaf","command":"SECRET_CHECK2","prompt":"Fix the parser","split":"dev","stratum":"x","subset_12":true,"head_commit":null,"base_commit":"def","features_sha256":"","features":{},"sha256":""}] })).expect("fixture");
        manifest.problems.sort_by(|a, b| a.id.cmp(&b.id));
        let trials =
            make_trials(&manifest, &[&CANDIDATES[0], &CANDIDATES[3]], "/repo", 0).expect("trials");
        assert_eq!(trials.len(), 5); // 2 problems × k=2, plus subset problem × k=1
        assert!(trials[0].objective.contains("Implement the widget"));
        assert!(!trials[0].objective.contains("SECRET_CHECK"));
        assert_eq!(trials[4].problem_id, "p2");
        assert_eq!(trials[4].check.cmd, "SECRET_CHECK2");
        assert_eq!(trials[0].harness_id, "claude_code");
        assert_eq!(trials[4].harness_id, "claude_code");
        let codex = make_trials(&manifest, &[&CANDIDATES[4]], "/repo", 0).expect("codex");
        assert!(codex.iter().all(|trial| trial.harness_id == "codex"));
        assert!(
            CANDIDATES
                .iter()
                .all(|candidate| !candidate.harness_id.is_empty())
        );
    }

    #[test]
    fn run_posts_manifest_trials_to_local_api_without_leaking_check_to_stdout() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("local listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut stream, raw) =
                fake_http::accept(&listener, "POST", "/api/v1/routing-eval/trials");
            let data = raw.as_bytes();
            let body_start = raw.find("\r\n\r\n").unwrap() + 4;
            let body: Value = serde_json::from_slice(&data[body_start..]).expect("json body");
            let wire = body.to_string();
            assert!(wire.contains("PRIVATE_CHECK_TOKEN"));
            let objective = body["trials"][0]["objective"].as_str().expect("objective");
            assert!(!objective.contains("PRIVATE_CHECK_TOKEN"));
            let response = json!({"batch_id":"b1","trials":[{"trial_id":"t1","problem_id":"p1","trial_no":1,"status":"queued"}]}).to_string();
            write!(stream, "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response.len(), response).expect("write response");
            drop(stream);

            let (mut stream, request) =
                fake_http::accept(&listener, "GET", "/api/v1/routing-eval/trials?batch_id=b1");
            assert!(request.starts_with("GET /api/v1/routing-eval/trials?batch_id=b1 "));
            let response = json!({"trials":[{"trial_id":"t1","problem_id":"p1","trial_no":1,"status":"passed"}]}).to_string();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response.len(), response).expect("write poll response");
        });
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("manifest.json");
        let output = dir.path().join("summary.json");
        let manifest = json!({"schema":"x","version":"1","cutoff":"x","seed":0,"sha256":"abc","problems":[{"id":"p1","task_id":"t1","work_unit_id":"w1","created_at":"","genre":"coding","kind":"leaf","command":"PRIVATE_CHECK_TOKEN","prompt":"Repair the cache invalidation behavior","split":"dev","stratum":"x","subset_12":true,"head_commit":null,"base_commit":"abc","features_sha256":"","features":{},"sha256":""}]});
        fs::write(&path, manifest.to_string()).expect("write manifest");
        run_args(vec![
            "--api".into(),
            format!("http://{address}"),
            "--manifest".into(),
            path.to_string_lossy().into_owned(),
            "--candidates".into(),
            "claude-opus-5-5".into(),
            "--repo".into(),
            dir.path().to_string_lossy().into_owned(),
            "--wait".into(),
            "--poll-interval-ms".into(),
            "1".into(),
            "--out".into(),
            output.to_string_lossy().into_owned(),
        ])
        .expect("run command");
        server.join().expect("server");
        let summary = fs::read_to_string(output).expect("summary");
        assert!(summary.contains("passed"));
        assert!(!summary.contains("PRIVATE_CHECK_TOKEN"));
    }
    #[test]
    fn run_trial_offset_shifts_trial_no() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("local listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut stream, raw) =
                fake_http::accept(&listener, "POST", "/api/v1/routing-eval/trials");
            let data = raw.as_bytes();
            let body_start = raw.find("\r\n\r\n").unwrap() + 4;
            let body: Value = serde_json::from_slice(&data[body_start..]).expect("json body");
            let trials = body["trials"].as_array().expect("trials").clone();
            let refs: Vec<Value> = trials.iter().enumerate().map(|(i, t)| json!({"trial_id":format!("t{i}"),"problem_id":t["problem_id"],"trial_no":t["trial_no"],"status":"queued"})).collect();
            let response = json!({"batch_id":"b2","trials":refs}).to_string();
            write!(stream, "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response.len(), response).expect("write response");
            trials
        });
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("manifest.json");
        let output = dir.path().join("summary.json");
        let manifest = json!({"schema":"x","version":"1","cutoff":"x","seed":0,"sha256":"abc","problems":[{"id":"p1","task_id":"t1","work_unit_id":"w1","created_at":"","genre":"coding","kind":"leaf","command":"CHECK","prompt":"Fix it","split":"dev","stratum":"x","subset_12":true,"head_commit":null,"base_commit":"abc","features_sha256":"","features":{},"sha256":""}]});
        fs::write(&path, manifest.to_string()).expect("write manifest");
        run_args(vec![
            "--api".into(),
            format!("http://{address}"),
            "--manifest".into(),
            path.to_string_lossy().into_owned(),
            "--candidates".into(),
            "claude-opus-5-5".into(),
            "--repo".into(),
            dir.path().to_string_lossy().into_owned(),
            "--trial-offset".into(),
            "2".into(),
            "--out".into(),
            output.to_string_lossy().into_owned(),
        ])
        .expect("run command");
        let posted = server.join().expect("server");
        let numbers: Vec<u64> = posted
            .iter()
            .map(|t| t["trial_no"].as_u64().expect("trial_no"))
            .collect();
        // claude-opus-5-5 は k=2: offset 2 で 3, 4（既定の 1, 2 と重ならない）。
        assert_eq!(numbers, vec![3, 4]);
        let summary: Value =
            serde_json::from_str(&fs::read_to_string(output).expect("summary")).expect("json");
        assert_eq!(summary["trials"][0]["trial_no"], 3);
        assert_eq!(summary["trials"][1]["trial_no"], 4);
        let err = run_args(vec!["--trial-offset".into(), "-1".into()]).expect_err("negative");
        assert_eq!(err, "invalid --trial-offset");
    }
}
