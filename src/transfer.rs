use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Keeps argv well under ARG_MAX.
const CHUNK: usize = 500;

/// Fail fast instead of waiting on a password prompt nobody can see.
const SSH: &str = "ssh -o BatchMode=yes -o ConnectTimeout=10";

/// `%n` prints one line per file actually sent, so lines count transfers.
const OUT_FORMAT: &str = "--out-format=%n";

/// rsync exit codes for partial transfer; files that did go are still reported.
const PARTIAL: &[i32] = &[23, 24];

pub enum Reach {
    Ready,
    Missing,
    Unreachable(String),
}

pub struct Outcome {
    pub sent: usize,
    pub error: Option<String>,
}

pub fn is_remote(target: &str) -> bool {
    match (target.find(':'), target.find('/')) {
        (Some(c), Some(s)) => c < s,
        (Some(_), None) => true,
        _ => false,
    }
}

fn split_remote(target: &str) -> Option<(&str, &str)> {
    if is_remote(target) { target.split_once(':') } else { None }
}

/// Strips shell quoting so a pasted `My\ Photos` or `'My Photos'` means the same folder.
pub fn unquote(s: &str) -> String {
    let mut out = String::new();
    let mut quote = None;
    let mut chars = s.chars();

    while let Some(c) = chars.next() {
        match (quote, c) {
            (None, '\\') => out.extend(chars.next()),
            (None, '\'' | '"') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            _ => out.push(c),
        }
    }
    out
}

/// The remote shell parses remote paths, so unsafe characters need escapes.
/// Without them, a space splits the path and rsync writes to the wrong place.
fn shell_escape(path: &str) -> String {
    let mut out = String::new();
    for (i, c) in path.chars().enumerate() {
        let home = i == 0 && c == '~';
        let safe = c.is_ascii_alphanumeric() || "/._-+,@=%".contains(c);
        if !safe && !home {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Trailing slash makes rsync treat the target as a directory.
fn dir_arg(target: &str) -> String {
    let t = match split_remote(target) {
        Some((host, path)) => format!("{host}:{}", shell_escape(path)),
        None => expand_home(target).to_string_lossy().into_owned(),
    };
    if t.ends_with('/') { t } else { format!("{t}/") }
}

fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
        None => PathBuf::from(path),
    }
}

/// ssh exit code for connection or auth failure.
const SSH_FAILED: i32 = 255;

/// Never creates the folder: an unmounted disk would otherwise
/// turn into a stray folder under /Volumes on the boot disk.
pub fn check(target: &str) -> Reach {
    let Some((host, path)) = split_remote(target) else {
        return match expand_home(target).is_dir() {
            true => Reach::Ready,
            false => Reach::Missing,
        };
    };

    let out = Command::new("ssh")
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10", host, "test", "-d"])
        .arg(shell_escape(path))
        .stdin(Stdio::null())
        .output();

    match out {
        Ok(o) if o.status.success() => Reach::Ready,
        Ok(o) if o.status.code() != Some(SSH_FAILED) => Reach::Missing,
        Ok(o) => Reach::Unreachable(String::from_utf8_lossy(&o.stderr).trim().to_string()),
        Err(e) => Reach::Unreachable(e.to_string()),
    }
}

fn prepare(target: &str) -> Result<(), String> {
    match check(target) {
        Reach::Ready => Ok(()),
        Reach::Missing => Err(format!("folder not found: {target}")),
        Reach::Unreachable(e) => Err(format!("cannot reach host: {e}")),
    }
}

/// `--ignore-existing` and no `--delete`: the target only ever gains files.
fn base_cmd() -> Command {
    let mut cmd = Command::new("rsync");
    cmd.args(["-a", "--ignore-existing", OUT_FORMAT, "-e", SSH]);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd
}

fn run(mut cmd: Command, progress: &AtomicUsize) -> Outcome {
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Outcome { sent: 0, error: Some(format!("rsync: {e}")) },
    };

    let mut stderr = child.stderr.take().expect("piped");
    let err_reader = std::thread::spawn(move || {
        let mut s = String::new();
        stderr.read_to_string(&mut s).ok();
        s
    });

    let mut sent = 0;
    let stdout = child.stdout.take().expect("piped");
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        // Directory entries end with '/'; only count files.
        if !line.ends_with('/') {
            sent += 1;
            progress.fetch_add(1, Ordering::Relaxed);
        }
    }

    let status = child.wait();
    let stderr = err_reader.join().unwrap_or_default();
    let error = match status {
        Ok(s) if s.success() => None,
        Ok(s) => {
            let code = s.code().unwrap_or(-1);
            let kind = if PARTIAL.contains(&code) { "partial" } else { "failed" };
            Some(format!("rsync {kind} ({code}): {}", stderr.trim()))
        }
        Err(e) => Some(e.to_string()),
    };
    Outcome { sent, error }
}

/// Flat copy: selected files land directly in the target directory.
pub fn copy_files(files: &[PathBuf], target: &str, progress: Arc<AtomicUsize>) -> Outcome {
    if let Err(e) = prepare(target) {
        return Outcome { sent: 0, error: Some(e) };
    }

    let dest = dir_arg(target);
    let mut total = Outcome { sent: 0, error: None };
    for chunk in files.chunks(CHUNK) {
        let mut cmd = base_cmd();
        cmd.args(chunk).arg(&dest);
        let out = run(cmd, &progress);
        total.sent += out.sent;
        if out.error.is_some() {
            total.error = out.error;
            break;
        }
    }
    total
}

/// Mirrors the tree under `source` into `target`, adding only.
pub fn sync_tree(source: &Path, target: &str, progress: Arc<AtomicUsize>) -> Outcome {
    if let Err(e) = prepare(target) {
        return Outcome { sent: 0, error: Some(e) };
    }

    let mut src = source.to_string_lossy().into_owned();
    if !src.ends_with('/') {
        src.push('/');
    }
    let mut cmd = base_cmd();
    cmd.arg(src).arg(dir_arg(target));
    run(cmd, &progress)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_detection() {
        assert!(is_remote("nas:/photos"));
        assert!(is_remote("me@nas:photos"));
        assert!(!is_remote("/Volumes/Backup"));
        assert!(!is_remote("/tmp/a:b"));
        assert!(!is_remote("~/Pictures"));
    }

    #[test]
    fn unquote_forms() {
        assert_eq!(unquote(r"/a/My\ Photos/x"), "/a/My Photos/x");
        assert_eq!(unquote("h:'/a/My Photos'"), "h:/a/My Photos");
        assert_eq!(unquote("\"/a b\""), "/a b");
        assert_eq!(unquote("/plain"), "/plain");
    }

    #[test]
    fn remote_arg_escaped() {
        assert_eq!(dir_arg("nas:/media/My Photos"), r"nas:/media/My\ Photos/");
        assert_eq!(dir_arg("nas:~/a(b)"), r"nas:~/a\(b\)/");
    }
}
