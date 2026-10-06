//! Create a two-entry user namespace owned by the dedicated launcher UID.
//! Mapping failures are fatal; the holder child is always reaped.
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::Command;

use nix::libc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mapping {
    pub host_id: u32,
    pub sub_id: u32,
}

impl Mapping {
    pub fn helper_args(self, pid: i32) -> Vec<String> {
        vec![
            pid.to_string(),
            "0".into(),
            self.host_id.to_string(),
            "1".into(),
            "1000".into(),
            self.sub_id.to_string(),
            "1".into(),
        ]
    }

    fn expected(self) -> String {
        format!("0 {} 1\n1000 {} 1\n", self.host_id, self.sub_id)
    }
}

/// Find a single subordinate ID from the dedicated user's allocation.
/// Reject malformed rows, overflow, and accidental mapping of the launcher itself.
pub fn subordinate_id(contents: &str, user: &str, host_id: u32) -> io::Result<u32> {
    for line in contents.lines() {
        let fields: Vec<_> = line.split(':').collect();
        if fields.first() != Some(&user) {
            continue;
        }
        if fields.len() != 3 {
            return Err(io::Error::other("invalid subordinate range"));
        }
        let start: u32 = fields[1]
            .parse()
            .map_err(|_| io::Error::other("invalid subordinate start"))?;
        let len: u32 = fields[2]
            .parse()
            .map_err(|_| io::Error::other("invalid subordinate length"))?;
        if len == 0 || start.checked_add(len - 1).is_none() || start == host_id {
            return Err(io::Error::other("invalid subordinate range"));
        }
        return Ok(start);
    }
    Err(io::Error::other("dedicated subordinate range missing"))
}

/// The helper's outside IDs must both be representable in our parent user namespace.
pub fn check_parent_range(map: &str, ids: Mapping) -> io::Result<()> {
    let mut ranges = Vec::new();
    for line in map.lines() {
        let values: Vec<_> = line.split_whitespace().collect();
        if values.len() != 3 {
            return Err(io::Error::other("invalid parent id map"));
        }
        let first: u64 = values[0]
            .parse()
            .map_err(|_| io::Error::other("invalid parent id map"))?;
        let count: u64 = values[2]
            .parse()
            .map_err(|_| io::Error::other("invalid parent id map"))?;
        ranges.push((first, first + count));
    }
    if [ids.host_id, ids.sub_id].into_iter().all(|id| {
        ranges
            .iter()
            .any(|(a, b)| *a <= id as u64 && (id as u64) < *b)
    }) {
        Ok(())
    } else {
        Err(io::Error::other("mapping outside parent namespace"))
    }
}

fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    // SAFETY: fds has two elements; successful pipe2 returns owned descriptors.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

pub struct UserNamespace {
    fd: File,
    child: i32,
    _hold: OwnedFd,
}

impl UserNamespace {
    pub fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    /// Ask the mapped subordinate UID to empty browser-writable directories.
    /// The holder inherited this userns at creation, so no host privilege or
    /// setns operation is needed. The caller must stop the runtime first.
    pub fn clear_session_contents(&mut self, dir: &Path) -> io::Result<()> {
        let path = dir.as_os_str().as_bytes();
        if path.is_empty() || path.len() >= 4096 || path.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid session path",
            ));
        }
        let mut msg = Vec::with_capacity(path.len() + 1);
        msg.extend_from_slice(path);
        msg.push(0);
        // One write below PIPE_BUF keeps the path intact for the holder.
        let n = unsafe { libc::write(self._hold.as_raw_fd(), msg.as_ptr().cast(), msg.len()) };
        if n < 0 || n as usize != msg.len() {
            return Err(io::Error::last_os_error());
        }
        let mut status = 0;
        if unsafe { libc::waitpid(self.child, &mut status, 0) } != self.child {
            return Err(io::Error::last_os_error());
        }
        self.child = 0;
        if libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0 {
            Ok(())
        } else {
            Err(io::Error::other("subuid session cleanup failed"))
        }
    }
}

impl Drop for UserNamespace {
    fn drop(&mut self) {
        if self.child == 0 {
            return;
        }
        // SAFETY: child is our unreaped holder process and cannot be PID-reused before waitpid.
        unsafe {
            libc::kill(self.child, libc::SIGKILL);
            libc::waitpid(self.child, std::ptr::null_mut(), 0);
        }
    }
}

/// The caller must already run as `celeris-browser`. The child uses only libc
/// between fork and _exit; no allocator or Rust library is called there.
pub fn create() -> io::Result<UserNamespace> {
    let uid = unsafe { libc::geteuid() };
    let gid = unsafe { libc::getegid() };
    let passwd = std::fs::read_to_string("/etc/passwd")?;
    let account = passwd
        .lines()
        .find(|l| l.split(':').next() == Some("celeris-browser"))
        .ok_or_else(|| io::Error::other("celeris-browser account missing"))?;
    let parts: Vec<_> = account.split(':').collect();
    if parts.len() < 4
        || parts[2].parse::<u32>().ok() != Some(uid)
        || parts[3].parse::<u32>().ok() != Some(gid)
    {
        return Err(io::Error::other("launcher is not celeris-browser"));
    }
    let uid_map = Mapping {
        host_id: uid,
        sub_id: subordinate_id(
            &std::fs::read_to_string("/etc/subuid")?,
            "celeris-browser",
            uid,
        )?,
    };
    let gid_map = Mapping {
        host_id: gid,
        sub_id: subordinate_id(
            &std::fs::read_to_string("/etc/subgid")?,
            "celeris-browser",
            gid,
        )?,
    };
    check_parent_range(&std::fs::read_to_string("/proc/self/uid_map")?, uid_map)?;
    check_parent_range(&std::fs::read_to_string("/proc/self/gid_map")?, gid_map)?;
    let (ready_r, ready_w) = pipe()?;
    let (hold_r, hold_w) = pipe()?;
    // SAFETY: child calls only async-signal-safe libc functions and _exit.
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(io::Error::last_os_error());
    }
    if pid == 0 {
        unsafe {
            libc::close(ready_r.as_raw_fd());
            libc::close(hold_w.as_raw_fd());
            let result = libc::unshare(libc::CLONE_NEWUSER);
            let status: u8 = if result == 0 { 1 } else { 0 };
            libc::write(ready_w.as_raw_fd(), &status as *const u8 as *const _, 1);
            if result != 0 {
                libc::_exit(1);
            }
            let mut path = [0u8; 4096];
            let n = libc::read(hold_r.as_raw_fd(), path.as_mut_ptr().cast(), path.len());
            if n <= 1 || path[(n - 1) as usize] != 0 {
                libc::_exit(0);
            }
            if libc::chdir(path.as_ptr().cast()) != 0
                || libc::setresgid(1000, 1000, 1000) != 0
                || libc::setresuid(1000, 1000, 1000) != 0
            {
                libc::_exit(1);
            }
            // Fixed command, cwd is opened before dropping to the subordinate
            // UID because session_root itself is 0700 for the launcher UID.
            let shell = c"/bin/sh";
            let arg0 = c"sh";
            let dash_c = c"-c";
            let script = c"status=0; for d in output home run actions profile tmp; do [ -d \"$d\" ] || continue; /bin/rm -rf -- \"$d\"/* \"$d\"/.[!.]* \"$d\"/..?* || status=1; done; exit \"$status\"";
            libc::execl(
                shell.as_ptr(),
                arg0.as_ptr(),
                dash_c.as_ptr(),
                script.as_ptr(),
                std::ptr::null::<libc::c_char>(),
            );
            libc::_exit(1);
        }
    }
    drop(ready_w);
    drop(hold_r);
    let mut holder = Holder(pid);
    let mut status = [0u8];
    File::from(ready_r).read_exact(&mut status)?;
    if status[0] != 1 {
        return Err(io::Error::other("unshare(CLONE_NEWUSER) failed"));
    }
    let proc = format!("/proc/{pid}");
    std::fs::write(format!("{proc}/setgroups"), "deny")?;
    run_map("/usr/bin/newuidmap", uid_map.helper_args(pid))?;
    run_map("/usr/bin/newgidmap", gid_map.helper_args(pid))?;
    verify_map(&format!("{proc}/uid_map"), uid_map)?;
    verify_map(&format!("{proc}/gid_map"), gid_map)?;
    if std::fs::read_to_string(format!("{proc}/setgroups"))?.trim() != "deny" {
        return Err(io::Error::other("setgroups was not denied"));
    }
    let fd = File::open(Path::new(&proc).join("ns/user"))?;
    // The holder stays alive until bwrap has entered the namespace.
    let child = holder.0;
    holder.0 = 0;
    Ok(UserNamespace {
        fd,
        child,
        _hold: hold_w,
    })
}

fn run_map(program: &str, args: Vec<String>) -> io::Result<()> {
    if !Command::new(program).args(args).status()?.success() {
        return Err(io::Error::other("user namespace map helper failed"));
    }
    Ok(())
}

fn verify_map(path: &str, expected: Mapping) -> io::Result<()> {
    let actual = std::fs::read_to_string(path)?;
    let normalized: Vec<_> = actual
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    if normalized.join("\n") == expected.expected().trim_end() {
        Ok(())
    } else {
        Err(io::Error::other("user namespace map mismatch"))
    }
}

struct Holder(i32);
impl Drop for Holder {
    fn drop(&mut self) {
        if self.0 > 0 {
            unsafe {
                libc::kill(self.0, libc::SIGKILL);
                libc::waitpid(self.0, std::ptr::null_mut(), 0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn helper_arguments_and_subordinates() {
        assert_eq!(
            Mapping {
                host_id: 2000,
                sub_id: 200000
            }
            .helper_args(42),
            ["42", "0", "2000", "1", "1000", "200000", "1"]
        );
        assert_eq!(
            subordinate_id(
                "other:100:2\nceleris-browser:200000:65536\n",
                "celeris-browser",
                2000
            )
            .unwrap(),
            200000
        );
        assert!(subordinate_id("celeris-browser:4294967295:2", "celeris-browser", 2000).is_err());
    }
    #[test]
    fn parent_range_is_required() {
        assert!(
            check_parent_range(
                "0 100000 65536",
                Mapping {
                    host_id: 2000,
                    sub_id: 200000
                }
            )
            .is_err()
        );
        assert!(
            check_parent_range(
                "0 0 4294967295",
                Mapping {
                    host_id: 2000,
                    sub_id: 200000
                }
            )
            .is_ok()
        );
    }

    #[test]
    fn real_namespace_when_host_is_ready() {
        // 実 unshare(CLONE_NEWUSER) を試すので userns opt-in の gate に入れる（ADR-0126 B）。
        if std::env::var("CELERIS_LAUNCHER_TESTS").as_deref() != Ok("require")
            && crate::test_support::skip_unless_userns_tests()
        {
            return;
        }
        match create() {
            Ok(ns) => {
                assert!(ns.as_raw_fd() >= 0);
            }
            Err(e) if std::env::var("CELERIS_LAUNCHER_TESTS").as_deref() != Ok("require") => {
                eprintln!("SKIPPED real launcher userns: {e}");
            }
            Err(e) => panic!("real launcher userns required: {e}"),
        }
    }
}
