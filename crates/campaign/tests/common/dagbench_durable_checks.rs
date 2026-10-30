use super::*;

#[test]
fn graded_a2_second_wait_positive_negative_and_raw_rejection() {
    let root = tempfile::tempdir().unwrap();
    exercise(root.path(), false, &[17]);
}

#[test]
fn graded_a2_first_wait_positive_negative_and_raw_rejection() {
    let root = tempfile::tempdir().unwrap();
    exercise(root.path(), false, &[24]);
}

#[test]
#[ignore = "set DAGBENCH_A2_OUT to a fresh persistent directory; requires K3 input archive"]
fn real_k3_a2_repetitions() {
    let out = PathBuf::from(std::env::var("DAGBENCH_A2_OUT").expect("DAGBENCH_A2_OUT"));
    assert!(out.is_absolute() && !out.exists());
    fs::create_dir_all(&out).unwrap();
    exercise(&out, true, &[17, 24]);
}

fn exercise(root: &Path, real: bool, seeds: &[u64]) {
    let grading = if real {
        grading_fixture::k3_grading(&root.join("inputs")).expect("missing K3 archive")
    } else {
        grading_fixture::fixture(
            &root.join("inputs"),
            vec![(
                "report.md".into(),
                grading_fixture::FABRICATED_REPORT.into(),
            )],
        )
    };
    let mut summary = vec![];
    for &seed in seeds {
        for attempts in [1, 2] {
            for arm in ["baseline", "celeris"] {
                let mut p = params(
                    arm,
                    "camp-1",
                    Some("a2-restart-after-wait"),
                    &root.join(format!("seed-{seed}-attempts-{attempts}")),
                );
                p.seed = Some(seed);
                let mut g = grading.clone();
                g.max_attempts = attempts;
                p.grading = Some(g);
                let result = run(&p);
                if attempts == 1 {
                    assert!(result.is_err(), "negative must fail");
                } else {
                    result.unwrap_or_else(|e| panic!("{arm}/{seed}: {e}"));
                }
                celeris_campaign::dagbench::durable::verify(&p.out).unwrap();
                let m = compute(&p.out).unwrap();
                assert_eq!(m.completed.campaigns, usize::from(attempts == 2));
                assert_eq!(m.duplicates.extra_physical_launches, 0);
                assert_eq!(
                    fs::read_dir(p.out.join("artifacts/c0/analyze.attempts"))
                        .unwrap()
                        .count(),
                    attempts as usize
                );
                if attempts == 2 {
                    // The first-wait outage can interrupt another pre node. Count its
                    // actual redo, but never count external waiting as worker work.
                    assert_eq!(
                        m.execution_busy_secs,
                        1080 + m.lost_progress.rerun_work_secs,
                        "{m:?}"
                    );
                    assert!(m.lost_progress.rerun_work_secs >= 120);
                    if seed == 17 {
                        assert_execution_cost(&m);
                    }
                }
                assert_eq!(
                    serde_json::to_value(&m).unwrap(),
                    serde_json::to_value(compute(&p.out).unwrap()).unwrap()
                );
                summary.push(json!({"arm": arm, "seed": seed, "attempts": attempts, "raw": p.out, "metrics": m}));
                if !real {
                    let path = p.out.join("durable-wait.json");
                    let original = fs::read(&path).unwrap();
                    let mut bad: Value = serde_json::from_slice(&original).unwrap();
                    bad["job_id"] = json!("wrong-job");
                    fs::write(&path, serde_json::to_vec(&bad).unwrap()).unwrap();
                    assert!(compute(&p.out).unwrap_err().contains("identity mismatch"));
                    fs::write(&path, original).unwrap();
                    let path = p.out.join("process-events.jsonl");
                    let original = fs::read(&path).unwrap();
                    let mut rows = celeris_campaign::dagbench::process::read_events(&path).unwrap();
                    rows.retain(|e| e["kind"] != "process_stopped" || e["role"] != "control");
                    fs::write(
                        &path,
                        rows.iter().map(|e| format!("{e}\n")).collect::<String>(),
                    )
                    .unwrap();
                    assert!(compute(&p.out).is_err());
                    fs::write(&path, original).unwrap();
                }
            }
        }
    }
    fs::write(
        root.join("summary.json"),
        serde_json::to_vec_pretty(&summary).unwrap(),
    )
    .unwrap();
}
