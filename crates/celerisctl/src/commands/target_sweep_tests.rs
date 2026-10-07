use super::*;
use std::fs::{File, FileTimes};
use std::time::{Duration, SystemTime};

/// `<root>/repo/wu-1/debug`（`.cargo-lock` つき）に、`age_days` 前の crate を 1 つ置く。返り値は rlib の path。
fn fake_target(root: &Path, age_days: u64) -> PathBuf {
    let prof = root.join("repo/wu-1/debug");
    let deps = prof.join("deps");
    std::fs::create_dir_all(&deps).unwrap();
    std::fs::write(prof.join(".cargo-lock"), b"").unwrap();
    let rlib = deps.join("libold-00000001.rlib");
    std::fs::write(&rlib, vec![0u8; 4096]).unwrap();
    let t = SystemTime::now() - Duration::from_secs(age_days * 86400);
    let f = File::open(&rlib).unwrap();
    f.set_times(FileTimes::new().set_accessed(t).set_modified(t))
        .unwrap();
    rlib
}

#[test]
fn target_sweep_cli_dry_run_json_deletes_nothing() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("cargo");
    let rlib = fake_target(&root, 30);

    let args = SweepArgs {
        roots: vec![root.clone()],
        dry_run: false,
        apply: false,
        json: true,
    };
    assert_eq!(args.mode(), TargetSweepMode::DryRun);
    let report = sweep(&args.roots, args.mode(), &SystemEnv);

    assert!(rlib.exists(), "dry-run removed a file");
    assert_eq!(report.mode, TargetSweepMode::DryRun);
    assert!(report.roots[0].deleted_items >= 1, "{report:?}");
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["mode"], "dry_run");
    assert!(json["roots"][0]["deleted_bytes"].as_u64().unwrap() > 0);
}

#[test]
fn target_sweep_cli_apply_with_root() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("cargo");
    let old = fake_target(&root, 30);
    let fresh_root = tmp.path().join("other");
    let fresh = fake_target(&fresh_root, 0);

    let args = SweepArgs {
        roots: vec![root.clone(), fresh_root.clone()],
        dry_run: false,
        apply: true,
        json: false,
    };
    assert_eq!(args.mode(), TargetSweepMode::Apply);
    let report = sweep(&args.roots, args.mode(), &SystemEnv);

    assert!(!old.exists(), "old item kept");
    assert!(fresh.exists(), "fresh item removed");
    assert!(report.errors.is_empty(), "{report:?}");
    assert!(report.deleted_bytes() > 0);
}

#[test]
fn target_sweep_cli_default_roots() {
    let roots = default_roots(Path::new("/x/build-cache"));
    assert_eq!(
        roots,
        vec![
            PathBuf::from("/x/build-cache/cargo"),
            PathBuf::from("/var/tmp/agent-platform-build")
        ]
    );
}
