//! 로컬 모델의 촘촘한 메시에서 게임에 쓸 로우폴리 판을 만든다. Blender 를 창 없이 돌린다. decisions/0007.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use crate::cli::{self, Cancel};
use crate::mesh::Mesh;
use crate::model::{self, Raw};

/// 로우폴리 판의 삼각형 예산.
pub const BUDGET: usize = 5_000;
const TIMEOUT: Duration = Duration::from_secs(5 * 60);
const SCRIPT: &str = include_str!("../scripts/blender_low.py");
const APP: &str = "/Applications/Blender.app/Contents/MacOS/Blender";

/// Blender 실행 파일.
pub fn blender() -> Option<PathBuf> {
    cli::find("blender").or_else(|| Some(PathBuf::from(APP)).filter(|p| p.is_file()))
}

pub fn available() -> bool {
    blender().is_some()
}

/// 메시를 닫힌 한 겹으로 다시 만들고 budget 까지 줄인다. 좌표계는 그대로다. 결과는 법선이 없는 평평한 면이다.
pub fn reduce(mesh: &Mesh, budget: usize, cancel: &Cancel) -> Result<Mesh, String> {
    let blender = blender().ok_or("Blender 를 찾지 못했습니다 (brew install --cask blender)")?;
    let work = cli::work_dir("lowpoly")?;
    let result = run(&blender, &work, mesh, budget, cancel);
    let _ = std::fs::remove_dir_all(&work);
    result
}

fn run(
    blender: &std::path::Path,
    work: &std::path::Path,
    mesh: &Mesh,
    budget: usize,
    cancel: &Cancel,
) -> Result<Mesh, String> {
    let (src, dst, script) = (
        work.join("in.ply"),
        work.join("out.ply"),
        work.join("blender_low.py"),
    );
    let raw = Raw {
        positions: mesh.positions.clone(),
        tris: mesh.tris.clone(),
        seed: model::SEED,
    };
    std::fs::write(&src, model::write_ply(&raw)).map_err(|e| e.to_string())?;
    std::fs::write(&script, SCRIPT).map_err(|e| e.to_string())?;
    let mut cmd = Command::new(blender);
    cmd.args([
        "-b",
        "--factory-startup",
        "--python-exit-code",
        "1",
        "--python",
    ])
    .arg(&script)
    .arg("--")
    .arg(&src)
    .arg(&dst)
    .arg(budget.to_string());
    cli::run(
        cmd,
        None,
        &work.join("out.log"),
        &work.join("err.log"),
        cancel,
        TIMEOUT,
    )?;
    let bytes = std::fs::read(&dst).map_err(|e| format!("Blender 출력을 읽지 못했습니다: {e}"))?;
    let low = model::read_ply(&bytes)?;
    let n = low.tris.len();
    if n == 0 || n > budget {
        return Err(format!(
            "로우폴리 삼각형 {n} 개가 예산 {budget} 을 벗어났습니다"
        ));
    }
    let mut out = Mesh {
        positions: low.positions,
        parts: vec![0; n],
        tris: low.tris,
        normals: Vec::new(),
    };
    // 법선이 바깥을 향하게 한다.
    let volume: f32 = (0..n)
        .map(|t| {
            let [a, b, c] = out.corners(t);
            a.dot(b.cross(c))
        })
        .sum();
    if volume < 0.0 {
        for t in &mut out.tris {
            t.swap(1, 2);
        }
    }
    Ok(out)
}
