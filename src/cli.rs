//! 구독 CLI(codex, claude)를 하위 프로세스로 실행한다. 취소와 제한 시간을 지킨다.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// 작업 취소 신호. Cmd+N 이 켠다.
#[derive(Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// 실행 파일 위치. Finder 로 띄운 앱은 셸 PATH 가 없으므로 흔한 설치 위치도 본다.
pub fn find(name: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(Path::new(&home).join(".local/bin"));
    }
    dirs.push("/opt/homebrew/bin".into());
    dirs.push("/usr/local/bin".into());
    dirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file())
}

/// 작업마다 새 임시 폴더.
pub fn work_dir(tag: &str) -> Result<PathBuf, String> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "mytripo_public-{tag}-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::SeqCst)
    ));
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// 명령을 끝까지 기다린다. stdout 은 `out`, stderr 는 `err` 파일에 쓴다.
/// 취소되거나 제한 시간을 넘으면 프로세스를 끝내고 오류를 낸다.
pub fn run(
    mut cmd: Command,
    stdin: Option<&Path>,
    out: &Path,
    err: &Path,
    cancel: &Cancel,
    timeout: Duration,
) -> Result<(), String> {
    let name = cmd.get_program().to_string_lossy().into_owned();
    let stdin = match stdin {
        Some(p) => Stdio::from(fs::File::open(p).map_err(|e| e.to_string())?),
        None => Stdio::null(),
    };
    let mut child = cmd
        .stdin(stdin)
        .stdout(Stdio::from(
            fs::File::create(out).map_err(|e| e.to_string())?,
        ))
        .stderr(Stdio::from(
            fs::File::create(err).map_err(|e| e.to_string())?,
        ))
        .spawn()
        .map_err(|e| format!("{name} 를 실행하지 못했습니다: {e}"))?;
    let started = Instant::now();
    let status = loop {
        if cancel.is_cancelled() || started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(if cancel.is_cancelled() {
                "취소했습니다".into()
            } else {
                format!(
                    "{name} 가 {}분 안에 끝나지 않았습니다",
                    timeout.as_secs() / 60
                )
            });
        }
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => std::thread::sleep(Duration::from_millis(200)),
            Err(e) => return Err(e.to_string()),
        }
    };
    if status.success() {
        Ok(())
    } else {
        let mut log = fs::read_to_string(err).unwrap_or_default();
        log.push_str(&fs::read_to_string(out).unwrap_or_default());
        Err(format!(
            "{name} 가 실패했습니다 ({status}): {}",
            tail(&log, 6)
        ))
    }
}

/// 마지막 n 줄을 " / " 로 잇는다.
pub fn tail(log: &str, n: usize) -> String {
    let lines: Vec<&str> = log.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join(" / ")
}
