//! 로그인된 codex CLI(ChatGPT 구독)의 내장 image_gen 으로 턴어라운드 시트를 만든다. API 키를 쓰지 않는다.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use crate::cli::{self, Cancel};
use crate::sheet;

const TIMEOUT: Duration = Duration::from_secs(900);

/// 시트 생성 요청.
#[derive(Clone, Debug, PartialEq)]
pub enum SheetJob {
    /// 글 설명과 참고 사진으로 새 시트를 만든다.
    New {
        description: String,
        refs: Vec<PathBuf>,
    },
    /// 현재 시트를 지시대로 고친다.
    Edit { sheet: PathBuf, instruction: String },
    /// 턴어라운드 시트의 인물이 설명한 자세를 한 자세 시트(앞·옆·3/4)를 만든다.
    Pose { sheet: PathBuf, description: String },
}

impl SheetJob {
    /// codex 에 붙일 이미지. 수정은 고칠 시트가 첫 번째다.
    pub fn images(&self) -> Vec<PathBuf> {
        match self {
            SheetJob::New { refs, .. } => refs.clone(),
            SheetJob::Edit { sheet, .. } | SheetJob::Pose { sheet, .. } => vec![sheet.clone()],
        }
    }
}

/// codex 에 줄 전체 프롬프트.
pub fn prompt(job: &SheetJob, out_png: &Path) -> String {
    let (mode, body) = match job {
        SheetJob::New { description, refs } => (
            if refs.is_empty() {
                "MODE: generate".to_string()
            } else {
                "MODE: generate — the attached images are references for the subject.".to_string()
            },
            sheet::new_prompt(description, refs.len()),
        ),
        SheetJob::Edit { instruction, .. } => (
            "MODE: edit — the FIRST attached image is the edit target; change only what the prompt asks and preserve everything else."
                .to_string(),
            sheet::edit_prompt(instruction),
        ),
        SheetJob::Pose { description, .. } => (
            "MODE: generate — the attached image is the character's turnaround sheet; draw that same character in the new pose."
                .to_string(),
            sheet::pose_prompt(description),
        ),
    };
    format!(
        "Use your built-in image_gen tool to create exactly one image.

MODEL: GPT Image 2.6 Flare (gpt-image-2.6-flare) — fast, high-quality everyday generation.
Use the newest GPT Image 2.6 model available to the image_gen tool. If the tool exposes a model or variant choice, pick the one named above. Never fall back to an older GPT Image model, the Images API, or an API key.

{mode}

PROMPT (use verbatim, do not rewrite or embellish):
{body}
SIZE: {size}
QUALITY: high

Requirements:
- You MUST call the image_gen tool. Do NOT write Python, shell out to curl, or fabricate a PNG any other way.
- After the tool produces the image, save it to the absolute path {out} with a shell command (cp or mv).
- Do NOT use MCP tools (node_repl, js, or any other) to move or write files. They run in a different working directory and the file will be lost.
- Reply with only the absolute path of the saved file, nothing else.",
        size = sheet::SIZE,
        out = out_png.display()
    )
}

pub fn args(work: &Path, images: &[PathBuf], prompt: &str) -> Vec<String> {
    let mut a: Vec<String> = [
        "exec",
        "--skip-git-repo-check",
        "-s",
        "workspace-write",
        "--color",
        "never",
        "-o",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    a.push(work.join("last.txt").display().to_string());
    a.push("-C".into());
    a.push(work.display().to_string());
    for img in images {
        a.push("-i".into());
        a.push(img.display().to_string());
    }
    a.push("--".into());
    a.push(prompt.to_string());
    a
}

/// PNG 가 없을 때의 오류 문장. codex 의 마지막 답과 로그에서 이유를 찾는다.
pub fn no_png_reason(last_message: &str, log: &str) -> String {
    let text = format!("{last_message}\n{log}");
    let answer: String = match last_message.trim() {
        "" => cli::tail(log, 3),
        m => m.chars().take(300).collect(),
    };
    let safety = [
        "safety system",
        "safety_violations",
        "moderation_blocked",
        "안전 검토",
    ]
    .iter()
    .any(|k| text.contains(k));
    if safety {
        let violations = text
            .find("safety_violations=")
            .and_then(|i| {
                text[i..]
                    .find(']')
                    .map(|j| format!(" ({})", &text[i..=i + j]))
            })
            .unwrap_or_default();
        format!(
            "OpenAI 이미지 안전 검토가 이 시트 요청을 거부했습니다{violations}. 설명이나 참고 사진을 바꿔 다시 생성하세요. codex 의 답: {answer}"
        )
    } else {
        format!("codex 가 끝났지만 시트 PNG 가 없습니다. codex 의 답: {answer}")
    }
}

/// codex 로그 머리의 session id.
pub fn session_id(log: &str) -> Option<String> {
    log.lines()
        .find_map(|l| l.trim().strip_prefix("session id:"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 시트를 만들어 `out` 에 저장한다.
pub fn generate(job: &SheetJob, out: &Path, cancel: &Cancel) -> Result<PathBuf, String> {
    let codex = cli::find("codex")
        .ok_or("codex CLI 를 찾지 못했습니다. codex 를 설치하고 codex login 으로 로그인하세요")?;
    for img in job.images() {
        if !img.is_file() {
            return Err(format!("이미지가 없습니다: {}", img.display()));
        }
    }
    let work = cli::work_dir("codex")?;
    let result = run(&codex, &work, job, out, cancel);
    let _ = fs::remove_dir_all(&work);
    result
}

fn run(
    codex: &Path,
    work: &Path,
    job: &SheetJob,
    out: &Path,
    cancel: &Cancel,
) -> Result<PathBuf, String> {
    let started = SystemTime::now();
    let out_png = work.join("out.png");
    let mut cmd = Command::new(codex);
    cmd.args(args(work, &job.images(), &prompt(job, &out_png)));
    let (log, err) = (work.join("codex.log"), work.join("codex.err"));
    cli::run(cmd, None, &log, &err, cancel, TIMEOUT)?;
    let found = if out_png.is_file() {
        Some(out_png)
    } else {
        let mut text = fs::read_to_string(&log).unwrap_or_default();
        text.push_str(&fs::read_to_string(&err).unwrap_or_default());
        newest_png(work, started).or_else(|| {
            // image_gen 은 $CODEX_HOME/generated_images/<session id>/ 에도 남긴다.
            let home = std::env::var_os("CODEX_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".codex")))?;
            let sid = session_id(&text)?;
            newest_png(&home.join("generated_images").join(sid), started)
        })
    };
    let Some(src) = found else {
        let last = fs::read_to_string(work.join("last.txt")).unwrap_or_default();
        let mut text = fs::read_to_string(&log).unwrap_or_default();
        text.push_str(&fs::read_to_string(&err).unwrap_or_default());
        let mut reason = no_png_reason(&last, &text);
        // 작업 폴더는 지워지므로 로그를 남긴다.
        let kept = crate::pipeline::output_dir().join("logs").join(format!(
            "{}-codex.log",
            chrono::Local::now().format("%Y%m%d-%H%M%S")
        ));
        if kept.parent().is_some_and(|d| fs::create_dir_all(d).is_ok())
            && fs::write(&kept, &text).is_ok()
        {
            reason.push_str(&format!(" (로그: {})", kept.display()));
        }
        return Err(reason);
    };
    if let Some(dir) = out.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::copy(&src, out).map_err(|e| format!("{}: {e}", out.display()))?;
    Ok(out.to_path_buf())
}

fn newest_png(dir: &Path, after: SystemTime) -> Option<PathBuf> {
    fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("png")))
        .filter_map(|p| Some((fs::metadata(&p).ok()?.modified().ok()?, p)))
        .filter(|(t, _)| *t >= after)
        .max_by_key(|(t, _)| *t)
        .map(|(_, p)| p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_attaches_the_sheet_first_and_keeps_rules() {
        let job = SheetJob::Edit {
            sheet: "/s/sheet.png".into(),
            instruction: "make the shirt blue".into(),
        };
        let p = prompt(&job, Path::new("/w/out.png"));
        assert!(p.contains("MODE: edit"));
        assert!(p.contains("make the shirt blue"));
        assert!(p.contains("orthographic"));
        assert!(p.contains("/w/out.png"));
        let a = args(Path::new("/w"), &job.images(), &p);
        let s = a.join(" ");
        assert!(s.contains("-C /w -i /s/sheet.png -- "));
        assert_eq!(a.last().unwrap(), &p);
    }

    #[test]
    fn new_job_uses_description_and_refs() {
        let job = SheetJob::New {
            description: "a knight in silver armor".into(),
            refs: vec!["/r/a.jpg".into()],
        };
        let p = prompt(&job, Path::new("/w/out.png"));
        assert!(p.contains("a knight in silver armor"));
        assert!(p.contains("references for the subject"));
        assert!(p.contains(&format!("SIZE: {}", sheet::SIZE)));
    }

    #[test]
    fn pose_job_attaches_the_turnaround_sheet() {
        let job = SheetJob::Pose {
            sheet: "/s/man.png".into(),
            description: "crouched guard".into(),
        };
        let p = prompt(&job, Path::new("/w/out.png"));
        assert!(
            p.contains("MODE: generate — the attached image is the character's turnaround sheet")
        );
        assert!(p.contains("Pose: crouched guard"));
        assert!(p.contains("THREE-QUARTER view"));
        assert_eq!(job.images(), vec![PathBuf::from("/s/man.png")]);
    }

    #[test]
    fn safety_rejection_is_reported() {
        let log = r#"Script error: image generation failed: http 400 Bad Request: "Your request was rejected by the safety system. safety_violations=[illicit].""#;
        let msg = no_png_reason(
            "image_gen의 자동 안전 검토가 산탄총 이미지 출력을 차단했습니다(`moderation_blocked`).",
            log,
        );
        assert!(msg.contains("안전 검토"), "{msg}");
        assert!(msg.contains("illicit"), "{msg}");
        assert!(msg.contains("산탄총"), "{msg}");
        let other = no_png_reason("파일을 저장하지 못했습니다", "");
        assert!(other.contains("파일을 저장하지 못했습니다"), "{other}");
    }

    #[test]
    fn session_id_is_read_from_log() {
        let log = "OpenAI Codex v0.171.0\n--------\nmodel: x\nsession id: 01a1ff1c-658e-74a2\n";
        assert_eq!(session_id(log).as_deref(), Some("01a1ff1c-658e-74a2"));
        assert_eq!(session_id("nothing"), None);
    }
}
