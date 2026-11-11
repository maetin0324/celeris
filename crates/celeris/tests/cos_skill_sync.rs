//! ADR 2026-11-11-cos-skill-sync D2: the release bundle and the promote-time sync of the CoS
//! skills (`scripts/selfdeploy/tests/sync_skills.sh`). Nothing else exercises `sync-skills.sh`
//! or `sd_bundle_cos_skills`; the script checks that curation's KB pages are not changed.

use std::path::Path;
use std::process::Command;

#[test]
fn promote_deploys_the_cos_skills_and_leaves_curation_pages() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = Command::new("bash")
        .arg(repo.join("scripts/selfdeploy/tests/sync_skills.sh"))
        .output()
        .expect("run sync_skills.sh");
    assert!(
        out.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
