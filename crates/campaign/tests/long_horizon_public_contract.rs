//! ADR 2026-11-05-phase3-p0c: bench_version 2 の公開の契約と保護領域の検査 script（接頭辞 `long_horizon_public_contract_`）。
//!
//! 全 10 課題について、公開の手順と複数の正解表現が合格し（K13）、不十分な解が落ちる（K14）ことを確かめる。判定は
//! 3 つの独立した実装（保護領域の `grader/check_outcome.py`、公開の `minichunk.py check`、この file の `judge`）で
//! 突き合わせる。leakage の否定（K15）と grader の隔離もここで確かめる。LLM を呼ばない（子 process は python3 だけ）。

use celeris_campaign::long_horizon::{GRADER_SCRIPT, Grader};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

const CASES: [&str; 10] = [
    "C01", "C02", "C03", "C04", "C05", "C06", "C07", "C08", "C09", "C10",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixtures() -> PathBuf {
    repo_root().join("tests/fixtures/long-horizon")
}

fn read(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn grader() -> Grader {
    Grader::open(&fixtures()).unwrap()
}

fn evidence(case: &str, id: &str) -> String {
    Sha256::digest(format!("minichunk {case} {id} chunk").as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 公開の repo-seed だけを写した作業 dir。
fn seed_dir(case: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let seed = fixtures().join("public").join(case).join("repo-seed");
    for e in std::fs::read_dir(&seed).unwrap() {
        let e = e.unwrap();
        std::fs::copy(e.path(), dir.path().join(e.file_name())).unwrap();
    }
    dir
}

fn outcomes(case: &str) -> Vec<Value> {
    read(&fixtures().join("public").join(case).join("case.json"))["request_outcomes"]
        .as_array()
        .unwrap()
        .clone()
}

/// acceptance の文から、backtick で囲んだ公開の手順を取り出す。
fn public_step(acceptance: &[Value]) -> Vec<String> {
    let text = acceptance
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    let start = text
        .find('`')
        .expect("acceptance names a command in backticks")
        + 1;
    let len = text[start..].find('`').unwrap();
    text[start..start + len]
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

fn python(dir: &Path, args: &[&str]) -> i32 {
    Command::new("python3")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
        .status
        .code()
        .unwrap()
}

/// 独立した judge: 公開の契約の文だけから書いた Rust の実装。検査 script と公開の `check` を読まずに同じ合否を出す。
fn judge(tree: &Path, case: &str, id: &str) -> bool {
    let out = tree.join("out");
    let path = out.join(format!("{id}.txt"));
    let (Ok(dmeta), Ok(meta)) = (out.symlink_metadata(), path.symlink_metadata()) else {
        return false;
    };
    if !dmeta.is_dir() || !meta.is_file() || meta.len() == 0 || meta.len() > 65536 {
        return false;
    }
    let Ok(text) = String::from_utf8(std::fs::read(&path).unwrap()) else {
        return false;
    };
    let text = text.trim_start_matches('\u{feff}');
    let mut fields: Vec<(String, Option<String>)> = Vec::new();
    let json = text
        .trim()
        .starts_with('{')
        .then(|| serde_json::from_str::<Value>(text.trim()).ok())
        .flatten();
    if let Some(Value::Object(m)) = json {
        for (k, v) in m {
            fields.push((k.to_lowercase(), v.as_str().map(str::to_owned)));
        }
    } else {
        for line in text.lines() {
            let mut l = line.trim_start();
            if let Some(rest) = l.strip_prefix("- ").or_else(|| l.strip_prefix("* ")) {
                l = rest.trim_start();
            }
            let Some(pos) = l.find([':', '：', '=']) else {
                continue;
            };
            let key = l[..pos].trim_end();
            if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphabetic() || c == '_') {
                continue;
            }
            let sep = l[pos..].chars().next().unwrap().len_utf8();
            let mut v = l[pos + sep..].trim();
            for q in ['`', '"', '\''] {
                if v.len() >= 2 && v.starts_with(q) && v.ends_with(q) {
                    v = v[1..v.len() - 1].trim();
                    break;
                }
            }
            fields.push((key.to_lowercase(), Some(v.to_owned())));
        }
    }
    let ev = evidence(case, id);
    [
        ("case", case, false),
        ("outcome", id, false),
        ("status", "verified", true),
        ("evidence", ev.as_str(), true),
    ]
    .iter()
    .all(|(k, want, fold)| {
        let got: Vec<&Option<String>> = fields
            .iter()
            .filter(|(x, _)| x == k)
            .map(|(_, v)| v)
            .collect();
        got.windows(2).all(|w| w[0] == w[1])
            && got.first().is_some_and(|v| {
                v.as_deref().is_some_and(|v| {
                    if *fold {
                        v.eq_ignore_ascii_case(want)
                    } else {
                        v == *want
                    }
                })
            })
    })
}

/// 3 つの実装の合否を得て、一致を確かめて返す。
fn verdict(tree: &Path, case: &str, id: &str) -> bool {
    let g = grader().check(case, id, tree).unwrap();
    let mine = judge(tree, case, id);
    assert_eq!(g.reached, mine, "{case} {id}: grader vs judge {g:?}");
    // 公開の `check` は写した seed の実装で走らせる（試行の木のコードは信用しない）。
    let seed = fixtures()
        .join("public")
        .join(case)
        .join("repo-seed/minichunk.py")
        .canonicalize()
        .unwrap();
    let public = python(tree, &["-I", seed.to_str().unwrap(), "check", case, id]) == 0;
    assert_eq!(
        g.reached, public,
        "{case} {id}: grader vs public check {g:?}"
    );
    g.reached
}

fn write_out(dir: &Path, id: &str, body: &[u8]) {
    std::fs::create_dir_all(dir.join("out")).unwrap();
    std::fs::write(dir.join("out").join(format!("{id}.txt")), body).unwrap();
}

/// 契約を満たす別の表現（K13）。公開の契約の文だけから書ける。
fn good_forms(case: &str, id: &str) -> Vec<(&'static str, String)> {
    let ev = evidence(case, id);
    let up = ev.to_uppercase();
    vec![
        (
            "lines",
            format!("case: {case}\noutcome: {id}\nstatus: verified\nevidence: {ev}\n"),
        ),
        (
            "reordered",
            format!("evidence: {ev}\nstatus: verified\noutcome: {id}\ncase: {case}\n"),
        ),
        (
            "spaces and equals",
            format!(
                "  case =   {case}  \n\n\toutcome=  {id}\nstatus= verified\nevidence  =  {ev}\n\n"
            ),
        ),
        (
            "report with extra lines",
            format!(
                "# 調査と実装の報告\n\n読み戻しは一致した。note: 親 dir も fsync した\n- case: {case}\n- outcome: {id}\n* status: `Verified`\n- evidence: \"{up}\"\n以上。\n"
            ),
        ),
        (
            "full-width colon and crlf",
            format!("CASE：{case}\r\nOUTCOME：{id}\r\nSTATUS：VERIFIED\r\nEVIDENCE：{ev}\r\n"),
        ),
        (
            "json",
            format!(
                "{{\"evidence\": \"{ev}\", \"outcome\": \"{id}\", \"case\": \"{case}\", \"status\": \"verified\", \"note\": \"別 process で読み戻した\", \"n\": 1}}\n"
            ),
        ),
    ]
}

/// K13: 公開の手順だけで全 10 課題・全 Outcome に届く。
#[test]
fn long_horizon_public_contract_reaches_from_public_steps_in_all_cases() {
    for case in CASES {
        let dir = seed_dir(case);
        for o in outcomes(case) {
            let id = o["id"].as_str().unwrap();
            assert!(!verdict(dir.path(), case, id), "{case} {id} before");
            let argv = public_step(o["acceptance"].as_array().unwrap());
            assert_eq!(
                argv,
                ["python3", "minichunk.py", "verify", case, id].map(String::from),
                "{case} {id}"
            );
            let args: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
            assert_eq!(python(dir.path(), &args), 0, "{case} {id}");
            assert!(verdict(dir.path(), case, id), "{case} {id}");
            // 公開の `check` は worker の command check にそのまま使える。
            assert_eq!(
                python(dir.path(), &["minichunk.py", "check", case, id]),
                0,
                "{case} {id}"
            );
        }
    }
}

/// K13: 公開の手順を使わず契約どおりに書いた複数の表現が、全 10 課題で合格する。
#[test]
fn long_horizon_public_contract_accepts_multiple_representations_in_all_cases() {
    for case in CASES {
        for o in outcomes(case) {
            let id = o["id"].as_str().unwrap();
            for (name, body) in good_forms(case, id) {
                let dir = tempfile::tempdir().unwrap();
                write_out(dir.path(), id, body.as_bytes());
                assert!(verdict(dir.path(), case, id), "{case} {id}: {name}: {body}");
            }
        }
    }
}

/// K14: 不十分な解は、全 10 課題で落ちる（空の木・自己申告の文・欄の欠け・別の課題の値・偽の表現）。
#[test]
fn long_horizon_public_contract_rejects_insufficient_outputs_in_all_cases() {
    for case in CASES {
        for o in outcomes(case) {
            let id = o["id"].as_str().unwrap();
            let ev = evidence(case, id);
            let other_case = if case == "C01" { "C02" } else { "C01" };
            let other_id = if id == "o1" { "o2" } else { "o1" };
            let good = format!("case: {case}\noutcome: {id}\nstatus: verified\nevidence: {ev}\n");
            let empty = tempfile::tempdir().unwrap();
            assert!(!verdict(empty.path(), case, id), "{case} {id}: empty tree");
            let bad: Vec<(&str, Vec<u8>)> = vec![
                ("empty file", vec![]),
                (
                    "v1 fixed line",
                    format!("minichunk {case} {id} verified: write+fsync+restart read-back ok\n")
                        .into_bytes(),
                ),
                ("self report", b"o1: all done. the check passed\n".to_vec()),
                ("incomplete", b"incomplete\n".to_vec()),
                ("wrong case", good.replace(case, other_case).into_bytes()),
                (
                    "wrong outcome",
                    good.replace(&format!("outcome: {id}"), &format!("outcome: {other_id}"))
                        .into_bytes(),
                ),
                (
                    "status failed",
                    good.replace("verified", "failed").into_bytes(),
                ),
                (
                    "other evidence",
                    good.replace(&ev, &evidence(other_case, id)).into_bytes(),
                ),
                (
                    "no evidence",
                    format!("case: {case}\noutcome: {id}\nstatus: verified\n").into_bytes(),
                ),
                (
                    "two status values",
                    format!("{good}status: failed\n").into_bytes(),
                ),
                (
                    "json with a non-string",
                    format!(
                        "{{\"case\": \"{case}\", \"outcome\": \"{id}\", \"status\": true, \"evidence\": \"{ev}\"}}"
                    )
                    .into_bytes(),
                ),
                ("not utf-8", [good.as_bytes(), b"\xff\xfe"].concat()),
                ("too large", {
                    let mut b = good.clone().into_bytes();
                    b.resize(65537, b'\n');
                    b
                }),
            ];
            for (name, body) in bad {
                let dir = tempfile::tempdir().unwrap();
                write_out(dir.path(), id, &body);
                assert!(!verdict(dir.path(), case, id), "{case} {id}: {name}");
            }
        }
    }
}

/// worker が置けるその他の形（symlink・dir）は通らず、木の外を読まない。
#[test]
fn long_horizon_public_contract_rejects_symlinks_and_dirs() {
    let g = grader();
    let good = format!(
        "case: C06\noutcome: o1\nstatus: verified\nevidence: {}\n",
        evidence("C06", "o1")
    );
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("o1.txt"), &good).unwrap();
    let link = tempfile::tempdir().unwrap();
    std::fs::create_dir(link.path().join("out")).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("o1.txt"),
        link.path().join("out/o1.txt"),
    )
    .unwrap();
    assert!(!g.check("C06", "o1", link.path()).unwrap().reached);
    let dlink = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), dlink.path().join("out")).unwrap();
    assert!(!g.check("C06", "o1", dlink.path()).unwrap().reached);
    let adir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(adir.path().join("out/o1.txt")).unwrap();
    assert!(!g.check("C06", "o1", adir.path()).unwrap().reached);
}

/// v1 の実 LLM の木（a9/llm。妥当な報告だが契約の前に書かれた）は v2 の契約を満たさない。raw は読むだけ。
#[test]
fn long_horizon_public_contract_v1_llm_trees_do_not_meet_the_contract() {
    let root = repo_root().join("agent-docs/progress/2026-11-04-long-horizon/a9/llm");
    for e in std::fs::read_dir(root.join("arm-map")).unwrap() {
        let p = e.unwrap().path();
        let case = read(&p)["case"].as_str().unwrap().to_owned();
        let t = p.file_stem().unwrap().to_str().unwrap().to_owned();
        let tree = root.join("trials").join(&t).join("repo");
        for o in outcomes(&case) {
            let id = o["id"].as_str().unwrap();
            assert!(!verdict(&tree, &case, id), "{case} {id}");
        }
    }
}

/// 公開側と台本の整合。acceptance の文は public と scenario で同じ。台本の worker が書く値はすべて公開の契約を満たし、
/// 課題の間で 4 通り以上の表現がある。
#[test]
fn long_horizon_public_contract_matches_scenario_in_all_cases() {
    let mut shapes = std::collections::BTreeSet::new();
    for case in CASES {
        let public = read(&fixtures().join("public").join(case).join("case.json"));
        let scenario = read(&fixtures().join("oracle").join(case).join("scenario.json"));
        assert_eq!(public["bench_version"], 2, "{case}");
        let strip = |v: &Value| -> Vec<Value> {
            v.as_array()
                .unwrap()
                .iter()
                .map(|o| serde_json::json!([o["id"], o["title"], o["acceptance"], o["depends_on"]]))
                .collect()
        };
        assert_eq!(
            strip(&public["request_outcomes"]),
            strip(&scenario["outcomes"]),
            "{case}"
        );
        for (id, body) in scenario["content"].as_object().unwrap() {
            let dir = tempfile::tempdir().unwrap();
            write_out(dir.path(), id, body.as_str().unwrap().as_bytes());
            assert!(verdict(dir.path(), case, id), "{case} {id}");
            let body = body
                .as_str()
                .unwrap()
                .replace(case, "CASE")
                .replace(id, "ID");
            shapes.insert(
                body.replace(&evidence(case, id), "EV")
                    .replace(&evidence(case, id).to_uppercase(), "EV"),
            );
        }
        for o in outcomes(case) {
            let text = o["acceptance"][0].as_str().unwrap();
            assert!(
                text.contains("README.md") && !text.contains("hidden"),
                "{text}"
            );
        }
        let readme = std::fs::read_to_string(
            fixtures()
                .join("public")
                .join(case)
                .join("repo-seed/README.md"),
        )
        .unwrap();
        for w in [
            "case", "outcome", "status", "evidence", "verified", "sha256",
        ] {
            assert!(readme.contains(w), "{case}: README lacks {w}");
        }
    }
    assert!(shapes.len() >= 4, "{shapes:?}");
}

fn tree_text(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let b = std::fs::read(&p).unwrap();
                out.push((p, b));
            }
        }
    }
    out
}

fn contains(hay: &[u8], needle: &str) -> bool {
    hay.windows(needle.len()).any(|w| w == needle.as_bytes())
}

/// K15: worker の作業場所（public と、それを写した試行の木）に検査 script・holdout の値・答えが無い。
/// 同じ検査は陽性対照で空振りでないことを確かめる。
#[test]
fn long_horizon_public_contract_no_seed_or_answer_leak_to_worker() {
    let fx = fixtures();
    let script = std::fs::read_to_string(fx.join(GRADER_SCRIPT)).unwrap();
    let marker = script
        .split("GRADER_MARKER = \"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_owned();
    let holdout = read(&fx.join("oracle/holdout.json"));
    let needles = [
        marker,
        holdout["canary"].as_str().unwrap().to_owned(),
        holdout["trial_salt"].as_str().unwrap().to_owned(),
        Sha256::digest(script.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
        // 検査 script の本体の行。
        "def judge(tree, case, outcome):".to_owned(),
    ];
    let g = grader();
    for case in CASES {
        let public = fx.join("public").join(case);
        assert!(
            g.leaks(&public).is_empty(),
            "{case}: {:?}",
            g.leaks(&public)
        );
        let dir = seed_dir(case);
        assert!(g.leaks(dir.path()).is_empty(), "{case}");
        // public に答え（どの課題の evidence の値も）・台本の値が文字どおりには無い。
        let scenario = read(&fx.join("oracle").join(case).join("scenario.json"));
        for (path, bytes) in tree_text(&public) {
            let name = path.display().to_string();
            for n in &needles {
                assert!(!contains(&bytes, n), "{name}: {n}");
            }
            for c2 in CASES {
                for i in 1..=4 {
                    let ev = evidence(c2, &format!("o{i}"));
                    assert!(!contains(&bytes, &ev), "{name}: evidence of {c2} o{i}");
                }
            }
            for body in scenario["content"].as_object().unwrap().values() {
                assert!(!contains(&bytes, body.as_str().unwrap()), "{name}: content");
            }
        }
    }
    // 検査 script は public の外にある。
    assert!(!GRADER_SCRIPT.starts_with("public/"));
    assert!(!fx.join("public").join(GRADER_SCRIPT).exists());
    // 試行の raw（v2）にも無い。
    let raw = repo_root().join("agent-docs/progress/2026-11-05-phase3/p0c/replay-v2/trials");
    let mut n = 0;
    for e in std::fs::read_dir(&raw).unwrap() {
        let t = e.unwrap().path().join("repo");
        assert!(g.leaks(&t).is_empty(), "{}", t.display());
        n += 1;
    }
    assert_eq!(n, 40);
    // 陽性対照: 写しに script を置くと、名前でも本文でも当たる。
    let ctl = seed_dir("C01");
    std::fs::write(ctl.path().join("notes.txt"), &script).unwrap();
    assert_eq!(g.leaks(ctl.path()), ["notes.txt"]);
    std::fs::rename(
        ctl.path().join("notes.txt"),
        ctl.path().join("check_outcome.py"),
    )
    .unwrap();
    assert_eq!(g.leaks(ctl.path()), ["check_outcome.py"]);
    let ctl2 = seed_dir("C01");
    std::fs::write(
        ctl2.path().join("notes.txt"),
        holdout["canary"].as_str().unwrap(),
    )
    .unwrap();
    assert_eq!(g.leaks(ctl2.path()), ["notes.txt"]);
}

/// 保護領域の写し（検査 script を差し替えて manifest も合わせた fixtures）。
fn fake_fixtures(script: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("grader")).unwrap();
    std::fs::create_dir_all(dir.path().join("oracle")).unwrap();
    std::fs::write(dir.path().join(GRADER_SCRIPT), script).unwrap();
    std::fs::copy(
        fixtures().join("oracle/holdout.json"),
        dir.path().join("oracle/holdout.json"),
    )
    .unwrap();
    let hash: String = Sha256::digest(script.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    std::fs::write(
        dir.path().join("oracle/MANIFEST.sha256"),
        format!("{hash}  {GRADER_SCRIPT}\n"),
    )
    .unwrap();
    dir
}

/// grader の隔離: 改ざんした script では採点しない。木と保護領域が重なる配置を拒む。木のコードを実行も import もしない。
/// script は環境変数を引き継がず、空の cwd で走る。exit 0・1 以外は未到達でなく Err（観測できなかった）。
#[test]
fn long_horizon_public_contract_grader_is_isolated_from_the_worker() {
    // 改ざん: 本物の script を 1 行変えると manifest の照合で開けない。
    let real = std::fs::read_to_string(fixtures().join(GRADER_SCRIPT)).unwrap();
    let tampered = fake_fixtures(&real);
    std::fs::write(
        tampered.path().join(GRADER_SCRIPT),
        real.replace("return 0 if not violations else 1", "return 0"),
    )
    .unwrap();
    assert!(
        Grader::open(tampered.path())
            .unwrap_err()
            .contains("MANIFEST")
    );
    // 重なる配置: 木が保護領域の中、保護領域が木の中のどちらも拒む。
    let g = grader();
    assert!(
        g.check("C01", "o1", &fixtures().join("public/C01/repo-seed"))
            .unwrap_err()
            .contains("overlaps")
    );
    assert!(
        g.check("C01", "o1", &repo_root())
            .unwrap_err()
            .contains("overlaps")
    );
    // 木のコード: 罠の json.py・hashlib.py・minichunk.py があっても実行されず、到達の判定も変わらない。
    let dir = tempfile::tempdir().unwrap();
    let sentinel = dir.path().join("executed");
    let trap = format!(
        "open({:?}, 'w').write('x')\nraise SystemExit(0)\n",
        sentinel.display().to_string()
    );
    for name in ["json.py", "hashlib.py", "re.py", "minichunk.py", "site.py"] {
        std::fs::write(dir.path().join(name), &trap).unwrap();
    }
    std::fs::create_dir_all(dir.path().join("out")).unwrap();
    std::fs::write(dir.path().join("out/o1.txt"), "all done\n").unwrap();
    let c = g.check("C01", "o1", dir.path()).unwrap();
    assert!(!c.reached, "{c:?}");
    assert!(!sentinel.exists(), "tree code was executed");
    // 環境と cwd: env -i と空の dir（python 自身が足す LC_CTYPE だけ許す）。
    let spy = fake_fixtures(concat!(
        "import json, os, sys\n",
        "GRADER_MARKER = \"spy\"\n",
        "env = [k for k in os.environ if k != 'LC_CTYPE']\n",
        "ok = not env and os.listdir('.') == []\n",
        "print(json.dumps({'reached': ok, 'violations': env}))\n",
        "sys.exit(0 if ok else 1)\n",
    ));
    let spied = Grader::open(spy.path()).unwrap();
    let tree = tempfile::tempdir().unwrap();
    let c = spied.check("C01", "o1", tree.path()).unwrap();
    assert!(c.reached, "{c:?}");
    // 検査を実行できない（exit 2）・出力が壊れている は Err。
    assert!(g.check("bad", "o1", tree.path()).is_err());
    let liar = fake_fixtures("GRADER_MARKER = \"x\"\nprint('not json')\n");
    assert!(
        Grader::open(liar.path())
            .unwrap()
            .check("C01", "o1", tree.path())
            .is_err()
    );
    let liar = fake_fixtures(
        "GRADER_MARKER = \"x\"\nimport sys\nprint('{\"reached\": false}')\nsys.exit(0)\n",
    );
    assert!(
        Grader::open(liar.path())
            .unwrap()
            .check("C01", "o1", tree.path())
            .is_err()
    );
}

/// 再採点: 全 40 試行の木の全 Outcome で、3 つの実装の合否が一致し、new の必須の Outcome はすべて届く。
#[test]
fn long_horizon_public_contract_independent_judges_agree_on_replay_trees() {
    let raw = repo_root().join("agent-docs/progress/2026-11-05-phase3/p0c/replay-v2");
    let (mut seen, mut pass) = (0, 0);
    for e in std::fs::read_dir(raw.join("arm-map")).unwrap() {
        let m = read(&e.unwrap().path());
        let case = m["case"].as_str().unwrap();
        let tree = raw
            .join("trials")
            .join(m["trial"].as_str().unwrap())
            .join("repo");
        let oracle = read(&fixtures().join("oracle").join(case).join("oracle.json"));
        for o in outcomes(case) {
            let id = o["id"].as_str().unwrap();
            let v = verdict(&tree, case, id);
            seen += 1;
            pass += usize::from(v);
            let required = oracle["required_outcomes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r == id);
            if m["arm"] == "new" && required {
                assert!(v, "{case} {id} new");
            }
        }
    }
    assert!(seen >= 40 && pass > 0 && pass < seen, "{seen} {pass}");
}
