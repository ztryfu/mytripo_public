//! 사람 메시에 기준 마네킹의 뼈대를 넣는다. Blender 를 창 없이 돌린다. decisions/0008.
//!
//! 뼈대는 Universal Animation Library(Quaternius, CC0)의 마네킹과 이름, 부모-자식 구조, rest 방향이 같다.
//! 뼈의 자리만 이 인물의 관절점에 맞춘다. 그래서 라이브러리의 동작 클립이 그대로 재생된다.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::cli::{self, Cancel};
use crate::lowpoly;
use crate::mesh::Mesh;
use crate::model::{self, Raw};
use crate::pipeline::{Frame, Photo};
use crate::structure::{Structure, View};

/// 기준 마네킹의 뼈대. scripts/make_skeleton.py 가 만든다.
pub const SKELETON: &str = include_str!("../schema/mannequin_skeleton.json");
const SCRIPT: &str = include_str!("../scripts/blender_rig.py");
const TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// 관절점: 이름과 세계 좌표 (X, Y).
pub type Joint = (String, f32, f32);

/// 앞(없으면 뒤) 사진의 관절점을 세계 좌표로 옮긴다.
pub fn joints(photos: &[Photo], frames: &[(usize, Frame)], st: &Structure) -> Vec<Joint> {
    for v in [View::Front, View::Back] {
        let Some((i, f)) = frames.iter().find(|(_, f)| f.view == v) else {
            continue;
        };
        let Some(info) = st.image(*i).filter(|im| !im.joints.is_empty()) else {
            continue;
        };
        let (w, h) = (photos[*i].mask.w as f32, photos[*i].mask.h as f32);
        return info
            .joints
            .iter()
            .map(|j| {
                let (x, y) = f.from_px(j.x * w, j.y * h);
                (j.name.clone(), x, y)
            })
            .collect();
    }
    Vec::new()
}

/// 메시에 뼈대를 넣은 skinned glb(T포즈, Y-up, 발바닥 원점). 취소되면 하위 프로세스를 끝낸다.
pub fn rig(mesh: &Mesh, joints: &[Joint], cancel: &Cancel) -> Result<Vec<u8>, String> {
    let blender =
        lowpoly::blender().ok_or("Blender 를 찾지 못했습니다 (brew install --cask blender)")?;
    if joints.is_empty() {
        return Err("구조 응답에 앞·뒤 사진의 관절점이 없습니다".into());
    }
    let work = cli::work_dir("rig")?;
    let result = run(&blender, &work, mesh, joints, cancel);
    let _ = std::fs::remove_dir_all(&work);
    result
}

fn run(
    blender: &Path,
    work: &Path,
    mesh: &Mesh,
    joints: &[Joint],
    cancel: &Cancel,
) -> Result<Vec<u8>, String> {
    let (src, list, skeleton, script, dst) = (
        work.join("in.ply"),
        work.join("joints.txt"),
        work.join("skeleton.json"),
        work.join("blender_rig.py"),
        work.join("out.glb"),
    );
    let raw = Raw {
        positions: mesh.positions.clone(),
        tris: mesh.tris.clone(),
        seed: model::SEED,
    };
    let text: String = joints
        .iter()
        .map(|(n, x, y)| format!("{n} {x} {y}\n"))
        .collect();
    let write = |p: &Path, b: &[u8]| std::fs::write(p, b).map_err(|e| e.to_string());
    write(&src, &model::write_ply(&raw))?;
    write(&list, text.as_bytes())?;
    write(&skeleton, SKELETON.as_bytes())?;
    write(&script, SCRIPT.as_bytes())?;
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
    .arg(&list)
    .arg(&skeleton)
    .arg(&dst);
    cli::run(
        cmd,
        None,
        &work.join("out.log"),
        &work.join("err.log"),
        cancel,
        TIMEOUT,
    )?;
    let glb = std::fs::read(&dst).map_err(|e| format!("Blender 출력을 읽지 못했습니다: {e}"))?;
    // 뼈의 이름이 기준 마네킹과 같아야 한다. skin 안의 순서는 달라도 된다.
    let names = |b: Vec<(String, [f32; 4])>| {
        let mut n: Vec<String> = b.into_iter().map(|x| x.0).collect();
        n.sort();
        n
    };
    let (got, want) = (names(bones(&glb)?), names(skeleton_bones()?));
    if got != want {
        let extra: Vec<&String> = got.iter().filter(|n| !want.contains(n)).collect();
        let lost: Vec<&String> = want.iter().filter(|n| !got.contains(n)).collect();
        return Err(format!(
            "뼈대가 기준 마네킹과 다릅니다 (없는 뼈 {lost:?}, 남는 뼈 {extra:?})"
        ));
    }
    Ok(glb)
}

/// glb 의 JSON 부분.
fn document(glb: &[u8]) -> Result<serde_json::Value, String> {
    let bad = || "glb 를 읽지 못했습니다".to_string();
    if glb.len() < 20 || &glb[0..4] != b"glTF" {
        return Err(bad());
    }
    let n = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
    serde_json::from_slice(glb.get(20..20 + n).ok_or_else(bad)?).map_err(|_| bad())
}

/// skinned glb 의 뼈: 이름과 local rotation(x, y, z, w). skin 의 joints 순서다.
pub fn bones(glb: &[u8]) -> Result<Vec<(String, [f32; 4])>, String> {
    let doc = document(glb)?;
    let joints = doc["skins"][0]["joints"]
        .as_array()
        .ok_or("glb 에 skin 이 없습니다")?;
    Ok(joints
        .iter()
        .filter_map(|j| doc["nodes"].get(j.as_u64()? as usize))
        .map(|n| {
            let r = &n["rotation"];
            let q = [0, 1, 2, 3].map(|k| r[k].as_f64().unwrap_or(if k == 3 { 1.0 } else { 0.0 }));
            (
                n["name"].as_str().unwrap_or_default().to_string(),
                q.map(|v| v as f32),
            )
        })
        .collect())
}

/// glb 에 든 메시 수와 재질 수.
pub fn counts(glb: &[u8]) -> Result<(usize, usize), String> {
    let doc = document(glb)?;
    let len = |k: &str| doc[k].as_array().map_or(0, Vec::len);
    Ok((len("meshes"), len("materials")))
}

/// 기준 마네킹의 뼈: 이름과 local rotation.
pub fn skeleton_bones() -> Result<Vec<(String, [f32; 4])>, String> {
    let doc: serde_json::Value = serde_json::from_str(SKELETON).map_err(|e| e.to_string())?;
    Ok(doc["bones"]
        .as_array()
        .ok_or("뼈대 파일에 bones 가 없습니다")?
        .iter()
        .map(|b| {
            let q = [0, 1, 2, 3].map(|k| b["rotation"][k].as_f64().unwrap_or(0.0) as f32);
            (b["name"].as_str().unwrap_or_default().to_string(), q)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skeleton_has_the_mannequin_bones() {
        let b = skeleton_bones().unwrap();
        assert_eq!(b.len(), 65);
        assert_eq!(b[0].0, "root");
        for name in [
            "pelvis",
            "spine_03",
            "Head",
            "upperarm_l",
            "calf_r",
            "ball_l",
        ] {
            assert!(b.iter().any(|x| x.0 == name), "{name}");
        }
    }
}
