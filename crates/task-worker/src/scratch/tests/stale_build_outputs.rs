//! ADR 2026-11-16-scratch-stale-build-outputs: 引き継ぎ・seed の写しの後に、元の場所を指す build 出力を消す。
//!
//! 守る不変条件: 引き継ぎ（rename）と seed の写しの後の target に、pool の `targets/`・`seeds/` の下の自分以外の
//! 場所を指す `rustc-link-search` を持つ package の build の出力が残らない。`root-output` の下だけを指す package
//! と、`targets/`・`seeds/` の外を指す package の出力は残る。判定は dir が在るかに依らない。
//! 既存の adopt・seed の試験は build script の出力（`output`・`root-output`）を作らないので、この欠陥を検出できない。
//! 分け方は `agent-docs/progress/2026-11-15-health-v2-v0/target-link-check.sh` の `--self-test` の 3 例と同じ
//! （root-output の下・外で元の場所でない・外で元の場所）。本物の cargo は走らせず、偽の build 出力を置く。

use super::*;
use crate::workspace_targets::CARGO_LOCK;

/// cargo の profile の dir（`.cargo-lock` と `.fingerprint/`。`workspace_targets::profile_dirs` が見る印）を作る。
fn make_profile(profile: &Path) {
    std::fs::create_dir_all(profile.join(".fingerprint")).unwrap();
    std::fs::write(profile.join(CARGO_LOCK), b"").unwrap();
}

/// package の unit の hash（cargo と同じ 16 桁の 16 進）。`[実行, build script の build, lib]`。
fn unit_hashes(n: u8) -> [String; 3] {
    [1, 2, 3].map(|k| format!("{n:02x}{k:014x}"))
}

/// cargo の配置を真似た偽の package を profile に作る（build はしない）:
/// `build/<pkg>-<run>/{output,root-output,out/}`、`build/<pkg>-<script>/build-script-build`、
/// `.fingerprint/<pkg>-{run,script,lib}/`、`deps/lib<crate>-<lib>.{rlib,rmeta}`、`deps/<crate>-<lib>.d`。
/// `root-output` は写す前の OUT_DIR（`<before>/<profile>/build/<pkg>-<run>/out`）を指す。
fn fake_package(profile: &Path, before_profile: &Path, pkg: &str, n: u8, link_search: &[String]) {
    let [run, script, lib] = unit_hashes(n);
    let crate_name = pkg.replace('-', "_");
    let run_dir = profile.join("build").join(format!("{pkg}-{run}"));
    std::fs::create_dir_all(run_dir.join("out")).unwrap();
    let root = before_profile
        .join("build")
        .join(format!("{pkg}-{run}"))
        .join("out");
    std::fs::write(run_dir.join("root-output"), root.display().to_string()).unwrap();
    let mut output =
        format!("cargo:rerun-if-changed=build.rs\ncargo:rustc-link-lib=static={crate_name}\n");
    for v in link_search {
        output.push_str(&format!("cargo:rustc-link-search={v}\n"));
    }
    std::fs::write(run_dir.join("output"), output).unwrap();
    let script_dir = profile.join("build").join(format!("{pkg}-{script}"));
    std::fs::create_dir_all(&script_dir).unwrap();
    std::fs::write(script_dir.join("build-script-build"), b"bin").unwrap();
    for h in [&run, &script, &lib] {
        let fp = profile.join(".fingerprint").join(format!("{pkg}-{h}"));
        std::fs::create_dir_all(&fp).unwrap();
        std::fs::write(fp.join("lib"), h.as_bytes()).unwrap();
    }
    let deps = profile.join("deps");
    std::fs::create_dir_all(&deps).unwrap();
    for ext in ["rlib", "rmeta"] {
        std::fs::write(deps.join(format!("lib{crate_name}-{lib}.{ext}")), b"lib").unwrap();
    }
    std::fs::write(
        deps.join(format!("{crate_name}-{lib}.d")),
        format!(
            "{}: /registry/src/{pkg}-1.0.0/src/lib.rs\n",
            deps.join(format!("lib{crate_name}-{lib}.rlib")).display()
        ),
    )
    .unwrap();
}

/// package の build の出力の数（`build/`・`.fingerprint/` の dir と `deps/` の file）。名前は完全一致で数える。
fn package_entries(profile: &Path, pkg: &str) -> usize {
    let mut n = 0;
    for sub in ["build", ".fingerprint"] {
        if let Ok(rd) = std::fs::read_dir(profile.join(sub)) {
            n += rd
                .flatten()
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .rsplit_once('-')
                        .is_some_and(|(p, _)| p == pkg)
                })
                .count();
        }
    }
    let crate_name = pkg.replace('-', "_");
    if let Ok(rd) = std::fs::read_dir(profile.join("deps")) {
        n += rd
            .flatten()
            .filter(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let base = name.split('.').next().unwrap_or("").to_string();
                let base = base.strip_prefix("lib").unwrap_or(&base).to_string();
                base.rsplit_once('-').is_some_and(|(c, _)| c == crate_name)
            })
            .count();
    }
    n
}

/// 1 package の entry の数（`fake_package` が作る数: build 2・fingerprint 3・deps 3）。
const PACKAGE_ENTRIES: usize = 8;

/// 3 例を `profile` に置く（`before` は写す前・引き継ぐ前の target）。返り値は例 3 の値。
/// - 例 1 `aws-lc-sys`: root-output の下（cargo が付け替える）→ 残す。
/// - 例 2 `v8-sys`: root-output の外で元の場所でない（pool の root の下の archive の置き場と pool の外の dir。どちらも
///   在る）→ 残す。名前が `v8-` で始まっても `v8` と一緒に消さないことも確かめる。
/// - 例 3 `v8`: root-output の外で元の場所（`<before>/debug/gn_out/obj`）→ 消す。
fn three_cases(pool: &Pool, profile: &Path, before: &Path, outside: &Path) -> String {
    let before_profile = before.join("debug");
    let [run, _, _] = unit_hashes(1);
    fake_package(
        profile,
        &before_profile,
        "aws-lc-sys",
        1,
        &[format!(
            "native={}",
            before_profile
                .join(format!("build/aws-lc-sys-{run}/out"))
                .display()
        )],
    );
    let archive = pool.root().join("rusty-v8/v150.4.0");
    std::fs::create_dir_all(&archive).unwrap();
    std::fs::create_dir_all(outside).unwrap();
    fake_package(
        profile,
        &before_profile,
        "v8-sys",
        2,
        &[
            format!("native={}", archive.display()),
            outside.display().to_string(),
        ],
    );
    let gn_out = before_profile.join("gn_out/obj").display().to_string();
    fake_package(
        profile,
        &before_profile,
        "v8",
        3,
        std::slice::from_ref(&gn_out),
    );
    gn_out
}

fn assert_kept(profile: &Path, pkg: &str) {
    assert_eq!(
        package_entries(profile, pkg),
        PACKAGE_ENTRIES,
        "{pkg} must be kept"
    );
}

fn assert_removed(profile: &Path, pkg: &str) {
    assert_eq!(package_entries(profile, pkg), 0, "{pkg} must be removed");
}

/// 引き継ぎ: rename で元の path が無くなった後、元の場所を指す v8 の出力が消え、例 1・2 は残る。引き継ぐ前の
/// seed の世代を指す package（連鎖。V0 の target で見た形）も消える。消した package は記録に残る。
#[test]
fn adopt_removes_packages_that_point_at_the_adopted_from_target() {
    let tmp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let pool = Pool::new(tmp.path());
    let repo = Path::new(SEED_REPO);
    let key = crate::build_cache::repo_cache_key(repo);
    let none = |_: &AdoptCandidate| None;
    let old = Owner::task("Old");
    allocate(&pool, &seed_req(&old, &none)).unwrap();
    let before = pool.target_dir(&old);
    let profile_before = before.join("debug");
    make_profile(&profile_before);
    let gn_out = three_cases(&pool, &profile_before, &before, &outside.path().join("lib"));
    std::fs::create_dir_all(&gn_out).unwrap();
    // 連鎖: 引き継ぎ元が前に seed から写した target で、seed の世代を指したまま残った出力。
    let seed_gen_obj = pool
        .root()
        .join(SEEDS_DIR)
        .join(&key)
        .join("gen-0/target/debug/gn_out/obj")
        .display()
        .to_string();
    fake_package(
        &profile_before,
        &before.join("debug"),
        "chained-sys",
        4,
        std::slice::from_ref(&seed_gen_obj),
    );
    for pkg in ["aws-lc-sys", "v8-sys", "v8", "chained-sys"] {
        assert_kept(&profile_before, pkg);
    }
    let candidate = AdoptCandidate {
        owner: old.clone(),
        repo_key: key,
        base_commit: None,
        last_write: at(T0),
        lease_mtime: mtime(&pool.lease_path(&old)),
    };
    let new = Owner::task("New");
    let cands = [candidate];
    let mut req = seed_req(&new, &none);
    req.adopt = true;
    req.checkout = Some(SystemTime::now() + Duration::from_secs(3600));
    req.candidates = &cands;
    let a = allocate(&pool, &req).unwrap();
    assert_eq!(a.adopted_from.as_deref(), Some("task-Old"));
    assert!(!before.exists(), "rename leaves nothing at the old path");
    let profile = a.target_dir.join("debug");
    assert_kept(&profile, "aws-lc-sys");
    assert_kept(&profile, "v8-sys");
    assert_removed(&profile, "v8");
    assert_removed(&profile, "chained-sys");
    let expected = vec![
        RemovedBuildOutput {
            profile: "debug".into(),
            package: "chained-sys".into(),
            link_search: vec![seed_gen_obj],
            removed_entries: PACKAGE_ENTRIES,
        },
        RemovedBuildOutput {
            profile: "debug".into(),
            package: "v8".into(),
            link_search: vec![gn_out],
            removed_entries: PACKAGE_ENTRIES,
        },
    ];
    assert_eq!(
        a.origin,
        TargetOrigin::Adopted {
            from: "task-Old".into(),
            removed: expected.clone(),
        }
    );
    let rec = read_target_origin(&pool.owner_dir(&new)).unwrap().unwrap();
    assert_eq!(rec.origin, "adopted");
    assert_eq!(rec.removed_build_outputs, Some(expected));
}

/// seed の写し: seed の世代（まだ在る）を指す v8 の出力が消え、例 1・2 は残る。seed の側は変えない。
#[test]
fn seed_copy_removes_packages_that_point_at_the_seed_generation() {
    let tmp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let pool = Pool::new(tmp.path());
    let seed = write_seed(&pool, MANIFEST);
    let seed_profile = seed.join("debug");
    std::fs::remove_file(seed_profile.join(".fingerprint")).unwrap();
    make_profile(&seed_profile);
    let gn_out = three_cases(&pool, &seed_profile, &seed, &outside.path().join("lib"));
    // 元の場所が在っても消す（判定は dir の有無に依らない）。
    std::fs::create_dir_all(&gn_out).unwrap();
    let owner = Owner::task("T1");
    let none = |_: &AdoptCandidate| None;
    let shared = |_: &Path| Ok(true);
    let ops = SeedCopyOps {
        copy: &cp_reflink_auto,
        is_shared: &shared,
    };
    let a = allocate_with_seed(
        &pool,
        &seed_req(&owner, &none),
        &SeedPolicy::unchecked(),
        &ops,
    )
    .unwrap();
    let TargetOrigin::Seed { removed, .. } = &a.origin else {
        panic!("expected a seed copy, got {:?}", a.origin);
    };
    let expected = vec![RemovedBuildOutput {
        profile: "debug".into(),
        package: "v8".into(),
        link_search: vec![gn_out],
        removed_entries: PACKAGE_ENTRIES,
    }];
    assert_eq!(removed, &expected);
    let profile = a.target_dir.join("debug");
    assert_kept(&profile, "aws-lc-sys");
    assert_kept(&profile, "v8-sys");
    assert_removed(&profile, "v8");
    assert!(a.target_dir.join("debug/deps/libbig.rlib").is_file());
    for pkg in ["aws-lc-sys", "v8-sys", "v8"] {
        assert_kept(&seed_profile, pkg);
    }
    let rec = read_target_origin(&pool.owner_dir(&owner))
        .unwrap()
        .unwrap();
    assert_eq!(rec.origin, "seed");
    assert_eq!(rec.removed_build_outputs, Some(expected));
}

/// 判定の細部（seed の写しと同じく、走査する一時 dir と rename の後の `target/` が違う形で呼ぶ）: `cargo::` の書式と
/// 前置き、root-output が無いときの `build/<pkg>-<hash>/out`、component ごとの比較、自分の target、profile ごとの
/// 判定（`<triple>/debug`）。消すものが無ければ `[]`。
#[test]
fn stale_build_output_rule_details() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = Pool::new(tmp.path());
    let own = pool.target_dir(&Owner::task("Own"));
    let scan = pool.owner_dir(&Owner::task("Own")).join(".seed-copy-1");
    let other = pool.target_dir(&Owner::task("Other"));
    let debug = scan.join("debug");
    let cross = scan.join("x86_64-unknown-linux-gnu/debug");
    for p in [&debug, &cross] {
        make_profile(p);
    }
    assert_eq!(
        remove_stale_build_outputs(&pool, &scan, &own).unwrap(),
        Vec::new()
    );
    let other_debug = other.join("debug");
    // `cargo::` と `dependency=`: 別の owner の target を指す → 消す。
    fake_package(&debug, &other_debug, "newsyntax", 1, &[]);
    let [run1, _, _] = unit_hashes(1);
    let value = other_debug.join("native").display().to_string();
    std::fs::write(
        debug.join(format!("build/newsyntax-{run1}/output")),
        format!("cargo::rustc-link-search=dependency={value}\n"),
    )
    .unwrap();
    // root-output が無い: 同じ dir の `out` の下（`targets/` の下だが付け替えの対象）→ 残す。
    let [run2, _, _] = unit_hashes(2);
    let noroot_out = debug.join(format!("build/noroot-{run2}/out/lib"));
    fake_package(
        &debug,
        &other_debug,
        "noroot",
        2,
        &[format!("native={}", noroot_out.display())],
    );
    std::fs::remove_file(debug.join(format!("build/noroot-{run2}/root-output"))).unwrap();
    // component ごと: root-output が `.../out` のとき `.../outx` はその下ではない → 消す。
    let [run3, _, _] = unit_hashes(3);
    let outx = other_debug
        .join(format!("build/prefix-{run3}/outx"))
        .display()
        .to_string();
    fake_package(
        &debug,
        &other_debug,
        "prefix",
        3,
        std::slice::from_ref(&outx),
    );
    // rename の後の自分の target の下（OUT_DIR の外でも）→ 残す。
    fake_package(
        &debug,
        &other_debug,
        "ownref",
        4,
        &[own.join("debug/gn_out/obj").display().to_string()],
    );
    // 別の profile: 同じ package 名でも、その profile で当たらなければ残す。
    fake_package(
        &cross,
        &other.join("x86_64-unknown-linux-gnu/debug"),
        "prefix",
        5,
        &[own
            .join("x86_64-unknown-linux-gnu/debug/gn_out/obj")
            .display()
            .to_string()],
    );
    let removed = remove_stale_build_outputs(&pool, &scan, &own).unwrap();
    let got: Vec<(&str, &str)> = removed
        .iter()
        .map(|r| (r.profile.as_str(), r.package.as_str()))
        .collect();
    assert_eq!(got, vec![("debug", "newsyntax"), ("debug", "prefix")]);
    assert_eq!(removed[0].link_search, vec![value]);
    assert_eq!(removed[1].link_search, vec![outx]);
    assert_removed(&debug, "newsyntax");
    assert_removed(&debug, "prefix");
    assert_kept(&debug, "noroot");
    assert_kept(&debug, "ownref");
    assert_kept(&cross, "prefix");
}

/// D4: 引き継いだ target から消す処理が I/O で落ちたら、その target を `targets/.deleting-*` へ動かし、引き継がずに
/// 空の target から始める（消しかけの target を使わない）。
#[test]
fn adopt_discards_the_target_when_stale_outputs_cannot_be_removed() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = Pool::new(tmp.path());
    let repo = Path::new(SEED_REPO);
    let none = |_: &AdoptCandidate| None;
    let old = Owner::task("Old");
    allocate(&pool, &seed_req(&old, &none)).unwrap();
    let before = pool.target_dir(&old);
    let profile = before.join("debug");
    make_profile(&profile);
    fake_package(
        &profile,
        &profile,
        "v8",
        1,
        &[profile.join("gn_out/obj").display().to_string()],
    );
    // `deps` を file にして、消す処理の `read_dir` を落とす。
    std::fs::remove_dir_all(profile.join("deps")).unwrap();
    std::fs::write(profile.join("deps"), b"not a dir").unwrap();
    let cands = [AdoptCandidate {
        owner: old.clone(),
        repo_key: crate::build_cache::repo_cache_key(repo),
        base_commit: None,
        last_write: at(T0),
        lease_mtime: mtime(&pool.lease_path(&old)),
    }];
    let new = Owner::task("New");
    let mut req = seed_req(&new, &none);
    req.adopt = true;
    req.checkout = Some(SystemTime::now() + Duration::from_secs(3600));
    req.candidates = &cands;
    let a = allocate(&pool, &req).unwrap();
    assert_eq!(a.adopted_from, None);
    assert_eq!(a.origin, TargetOrigin::Empty { reason: None });
    assert!(is_empty_dir(&a.target_dir));
    let lease = read_lease(&pool.lease_path(&new)).unwrap().unwrap();
    assert_eq!(lease.adopted_from, None);
    let trash: Vec<PathBuf> = std::fs::read_dir(pool.targets_dir())
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(DELETING_PREFIX))
        })
        .collect();
    assert_eq!(trash.len(), 1, "{trash:?}");
    assert!(trash[0].join("debug/deps").is_file());
    assert!(!before.exists());
}
