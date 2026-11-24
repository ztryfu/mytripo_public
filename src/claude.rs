//! 로그인된 claude CLI(Claude 구독)로 구조 읽기를 한 번 부른다. API 키를 쓰지 않는다.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::cli::{self, Cancel};
use crate::pipeline::Photo;
use crate::structure::{self, Structure};

const TIMEOUT: Duration = Duration::from_secs(600);

/// claude -p 인자. 도구는 Read 하나만 허용하고, 사용자 설정·플러그인·훅은 끈다(--safe-mode).
pub fn args(work: &Path) -> Vec<String> {
    let mut a: Vec<String> = [
        "-p",
        "--safe-mode",
        "--no-session-persistence",
        "--tools",
        "Read",
        "--allowed-tools",
        "Read",
        "--permission-prompts",
        "none",
        "--output-format",
        "json",
        "--json-schema",
        structure::SCHEMA,
        "--add-dir",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    a.push(work.display().to_string());
    a
}

/// claude -p --output-format json 의 출력에서 구조를 꺼낸다.
pub fn parse_output(stdout: &str, n_images: usize) -> Result<Structure, String> {
    let v: serde_json::Value =
        serde_json::from_str(stdout).map_err(|e| format!("claude 출력을 읽지 못했습니다: {e}"))?;
    if v["is_error"].as_bool().unwrap_or(false) {
        return Err(format!(
            "claude 구조 읽기가 실패했습니다: {}",
            v["result"].as_str().unwrap_or("이유 없음")
        ));
    }
    let so = v
        .get("structured_output")
        .filter(|s| s.is_object())
        .ok_or_else(|| {
            format!(
                "claude 응답에 구조가 없습니다: {}",
                v["result"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(200)
                    .collect::<String>()
            )
        })?;
    Structure::parse(&so.to_string(), n_images)
}

/// 사진들의 구조를 읽는다. 취소되면 하위 프로세스를 끝낸다.
pub fn read_structure(photos: &[Photo], cancel: &Cancel) -> Result<Structure, String> {
    let claude = cli::find("claude")
        .ok_or("claude CLI 를 찾지 못했습니다. Claude Code 를 설치하고 로그인하세요")?;
    let work = cli::work_dir("claude")?;
    let result = run(&claude, &work, photos, cancel);
    let _ = fs::remove_dir_all(&work);
    result
}

fn run(claude: &Path, work: &Path, photos: &[Photo], cancel: &Cancel) -> Result<Structure, String> {
    let mut images: Vec<PathBuf> = Vec::new();
    for (i, p) in photos.iter().enumerate() {
        let path = work.join(format!("photo_{i}.png"));
        p.rgb.save(&path).map_err(|e| e.to_string())?;
        images.push(path);
    }
    fs::write(work.join("prompt.txt"), structure::prompt(&images)).map_err(|e| e.to_string())?;
    let mut cmd = Command::new(claude);
    cmd.args(args(work)).current_dir(work);
    let (out, err) = (work.join("out.json"), work.join("err.log"));
    cli::run(
        cmd,
        Some(&work.join("prompt.txt")),
        &out,
        &err,
        cancel,
        TIMEOUT,
    )?;
    parse_output(
        &fs::read_to_string(&out).map_err(|e| e.to_string())?,
        photos.len(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_allow_only_read_and_pass_schema() {
        let a = args(Path::new("/w"));
        let s = a.join(" ");
        assert!(s.starts_with("-p --safe-mode"));
        assert!(s.contains("--tools Read --allowed-tools Read"));
        let i = a.iter().position(|x| x == "--json-schema").unwrap();
        assert_eq!(a[i + 1], structure::SCHEMA);
        assert!(s.ends_with("--add-dir /w"));
    }

    #[test]
    fn output_structured_field_is_read() {
        let so = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/test/fixtures/structure.json"
        ))
        .unwrap();
        let out =
            format!(r#"{{"type":"result","is_error":false,"result":"","structured_output":{so}}}"#);
        let s = parse_output(&out, 3).unwrap();
        assert_eq!(s.images.len(), 3);
        let err = parse_output(r#"{"is_error":true,"result":"limit reached"}"#, 3).unwrap_err();
        assert!(err.contains("limit reached"));
        assert!(parse_output(r#"{"is_error":false,"result":"no json"}"#, 3).is_err());
    }
}
