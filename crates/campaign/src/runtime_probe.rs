//! Runtime 内の可読性と mount source を調べる。LLM・network は使わない（ops-r2 ADR D3）。
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::{self, Read},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct PathProbe {
    pub path: PathBuf,
    pub readable: bool,
    pub listable: bool,
    /// 読み取り・親の一覧取得で最初に起きた OS error。成功時は null。
    pub errno: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RuntimeProbe {
    pub schema: String,
    pub principal: String,
    pub uid: u32,
    pub paths: Vec<PathProbe>,
    pub mounts_outside_allow: Vec<String>,
    pub ok: bool,
}

fn errno(e: &io::Error) -> String {
    e.raw_os_error()
        .map(|n| format!("errno:{n}"))
        .unwrap_or_else(|| e.to_string())
}

/// mountinfo の octal escaping（空白・tab・改行・backslash）を戻す。
pub fn unescape(s: &str) -> String {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && i + 3 < b.len()
            && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c))
        {
            let n = (u16::from(b[i + 1] - b'0') * 64)
                + (u16::from(b[i + 2] - b'0') * 8)
                + u16::from(b[i + 3] - b'0');
            if let Ok(n) = u8::try_from(n) {
                out.push(n);
                i += 4;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// source 欄を照合する。絶対 path は配下も許可し、proc/tmpfs 等の名前は完全一致で許可する。
/// mountinfo を取得・解釈できない場合は成功扱いにしない。
pub fn mounts_outside_allow(mountinfo: &str, allow: &[PathBuf]) -> Result<Vec<String>, String> {
    let mut outside = BTreeSet::new();
    if mountinfo.is_empty() {
        return Err("empty mountinfo".into());
    }
    for line in mountinfo.lines() {
        let (left, right) = line
            .split_once(" - ")
            .ok_or("invalid mountinfo separator")?;
        if left.split_whitespace().count() < 6 {
            return Err("invalid mountinfo fields".into());
        }
        let fields: Vec<_> = right.split_whitespace().collect();
        if fields.len() < 3 {
            return Err("invalid mountinfo source".into());
        }
        let source = unescape(fields[1]);
        let path = Path::new(&source);
        if !allow.iter().any(|a| {
            if path.is_absolute() && a.is_absolute() {
                path.starts_with(a)
            } else {
                path == a
            }
        }) {
            outside.insert(source);
        }
    }
    Ok(outside.into_iter().collect())
}

pub fn probe(principal: &str, deny: &[PathBuf], allow: &[PathBuf]) -> Result<RuntimeProbe, String> {
    let uid = fs::metadata("/proc/self").map_err(|e| e.to_string())?.uid();
    let mounts = fs::read_to_string("/proc/self/mountinfo").map_err(|e| e.to_string())?;
    let mounts_outside_allow = mounts_outside_allow(&mounts, allow)?;
    let paths: Vec<_> = deny
        .iter()
        .map(|path| {
            // EOF も読み取り成功。空 file を「拒否」と扱わない。
            let read = File::open(path).and_then(|mut f| f.read(&mut [0u8; 1]));
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let list = fs::read_dir(parent).and_then(|mut entries| entries.next().transpose());
            PathProbe {
                path: path.clone(),
                readable: read.is_ok(),
                listable: list.is_ok(),
                errno: read
                    .as_ref()
                    .err()
                    .or_else(|| list.as_ref().err())
                    .map(errno),
            }
        })
        .collect();
    let ok = paths.iter().all(|p| !p.readable && !p.listable) && mounts_outside_allow.is_empty();
    Ok(RuntimeProbe {
        schema: "celeris.runtime-probe/1".into(),
        principal: principal.into(),
        uid,
        paths,
        mounts_outside_allow,
        ok,
    })
}

/// args は subcommand を含まない。out 指定時も同じ JSON を stdout に返す。
pub fn cli(args: &[String]) -> Result<bool, String> {
    let mut principal = None;
    let mut deny = Vec::new();
    let mut allow = Vec::new();
    let mut out = None;
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {flag}"))?;
        match flag.as_str() {
            "--principal" => principal = Some(value.clone()),
            "--deny" => deny.push(PathBuf::from(value)),
            "--allow" => allow.push(PathBuf::from(value)),
            "--out" => out = Some(PathBuf::from(value)),
            _ => return Err(format!("unknown probe-read option: {flag}")),
        }
    }
    let principal = principal
        .filter(|p| !p.trim().is_empty())
        .ok_or("--principal is required")?;
    if deny.is_empty() {
        return Err("--deny is required".into());
    }
    let result = probe(&principal, &deny, &allow)?;
    let json = serde_json::to_string(&result).map_err(|e| e.to_string())?;
    if let Some(out) = out {
        fs::write(out, format!("{json}\n")).map_err(|e| e.to_string())?;
    }
    println!("{json}");
    Ok(result.ok)
}
