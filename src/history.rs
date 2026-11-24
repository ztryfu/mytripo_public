//! 이전 작업 목록. 메시를 저장할 때 `<시각>.json` 에 입력, 구조 응답, 칸 배치를 남긴다.
//! 앱은 이 기록으로 메시를 다시 열고(구독 호출 없음), 같은 입력으로 다시 만든다(claude 1회).
//! 기록이 없는 옛 시트(`<시각>-sheet.png`)도 목록에 넣는다. 그 시트는 다시 만들기만 된다.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::pipeline::Slots;
use crate::structure::Structure;

/// 메시를 만든 입력.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Input {
    Sheet { path: PathBuf },
    Photos { paths: Vec<PathBuf> },
}

impl Input {
    pub fn paths(&self) -> Vec<&Path> {
        match self {
            Input::Sheet { path } => vec![path.as_path()],
            Input::Photos { paths } => paths.iter().map(PathBuf::as_path).collect(),
        }
    }
}

/// `<시각>.json` 의 내용.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Record {
    pub input: Input,
    #[serde(default)]
    pub description: String,
    pub structure: Structure,
    pub slots: Slots,
}

/// 목록의 한 줄.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// 파일 이름의 시각(yyyyMMdd-HHmmss).
    pub stamp: String,
    /// 저장한 메시. 옛 시트에는 없다.
    pub glb: Option<PathBuf>,
    /// 목록에 보일 그림: 메시 미리보기 또는 입력 그림.
    pub thumb: PathBuf,
    pub input: Input,
    /// 다시 열기에 쓸 기록. 옛 시트에는 없다.
    pub record: Option<Record>,
}

impl Entry {
    /// "11-22 21:41"
    pub fn time(&self) -> String {
        let s = &self.stamp;
        format!("{}-{} {}:{}", &s[4..6], &s[6..8], &s[9..11], &s[11..13])
    }

    /// 설명이 있으면 설명, 없으면 입력의 종류와 첫 파일 이름.
    pub fn title(&self) -> String {
        if let Some(r) = &self.record
            && !r.description.trim().is_empty()
        {
            return r.description.trim().to_string();
        }
        match &self.input {
            Input::Sheet { .. } => "시트".into(),
            Input::Photos { paths } => format!("사진 {}장", paths.len()),
        }
    }
}

/// 이름이 yyyyMMdd-HHmmss 로 시작하면 그 시각을 낸다.
fn stamp_of(name: &str) -> Option<&str> {
    let s = name.get(..15)?;
    let b = s.as_bytes();
    (b[8] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 8 || c.is_ascii_digit()))
    .then_some(s)
}

/// 미리보기 그림의 경로.
pub fn thumb_path(glb: &Path) -> PathBuf {
    glb.with_extension("thumb.png")
}

/// 로컬 모델이 낸 메시의 경로. glb 옆에 둔다.
pub fn model_path(glb: &Path) -> PathBuf {
    glb.with_extension("model.ply")
}

/// 뼈대를 넣은 판의 경로. glb 옆에 둔다.
pub fn rig_path(glb: &Path) -> PathBuf {
    glb.with_extension("rig.glb")
}

/// 로우폴리 판의 경로. glb 옆에 둔다.
pub fn low_path(glb: &Path) -> PathBuf {
    glb.with_extension("low.glb")
}

/// glb 옆에 기록과 미리보기 그림을 남긴다. dir 밖의 입력 파일은 dir 로 복사해서 기록한다.
/// 사용자가 원본을 옮기거나 지워도 다시 만들 수 있다.
pub fn save(
    dir: &Path,
    glb: &Path,
    input: &Input,
    description: &str,
    structure: &Structure,
    slots: Slots,
    thumb: &image::RgbaImage,
) -> Result<(), String> {
    let stem = glb
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("메시 파일 이름이 없습니다")?;
    let keep = |k: usize, p: &Path| -> Result<PathBuf, String> {
        if p.parent() == Some(dir) {
            return Ok(p.to_path_buf());
        }
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("png");
        let to = dir.join(format!("{stem}-input-{k}.{ext}"));
        std::fs::copy(p, &to).map_err(|e| format!("{}: {e}", p.display()))?;
        Ok(to)
    };
    let input = match input {
        Input::Sheet { path } => Input::Sheet {
            path: keep(0, path)?,
        },
        Input::Photos { paths } => Input::Photos {
            paths: paths
                .iter()
                .enumerate()
                .map(|(k, p)| keep(k, p))
                .collect::<Result<_, _>>()?,
        },
    };
    let rec = Record {
        input,
        description: description.to_string(),
        structure: structure.clone(),
        slots,
    };
    let json = serde_json::to_string_pretty(&rec).map_err(|e| e.to_string())?;
    let path = glb.with_extension("json");
    std::fs::write(&path, json).map_err(|e| format!("{}: {e}", path.display()))?;
    let tp = thumb_path(glb);
    thumb
        .save(&tp)
        .map_err(|e| format!("{}: {e}", tp.display()))
}

/// dir 의 이전 작업, 새것부터.
pub fn list(dir: &Path) -> Vec<Entry> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let names: Vec<String> = rd
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(String::from))
        .collect();
    let mut out: Vec<Entry> = Vec::new();
    for n in &names {
        let Some(stamp) = stamp_of(n) else { continue };
        if n.len() != 20 || !n.ends_with(".json") {
            continue;
        }
        let Some(rec) = std::fs::read_to_string(dir.join(n))
            .ok()
            .and_then(|s| serde_json::from_str::<Record>(&s).ok())
        else {
            continue;
        };
        let glb = dir.join(format!("{stamp}.glb"));
        let thumb = thumb_path(&glb);
        let thumb = if thumb.exists() {
            thumb
        } else {
            rec.input.paths()[0].to_path_buf()
        };
        out.push(Entry {
            stamp: stamp.to_string(),
            glb: glb.exists().then_some(glb),
            thumb,
            input: rec.input.clone(),
            record: Some(rec),
        });
    }
    // 기록이 쓰지 않는 옛 시트.
    for n in &names {
        let Some(stamp) = stamp_of(n) else { continue };
        if n.len() != 25 || !n.ends_with("-sheet.png") {
            continue;
        }
        let path = dir.join(n);
        if out
            .iter()
            .any(|e| e.input.paths().contains(&path.as_path()))
        {
            continue;
        }
        out.push(Entry {
            stamp: stamp.to_string(),
            glb: None,
            thumb: path.clone(),
            input: Input::Sheet { path },
            record: None,
        });
    }
    out.sort_by(|a, b| b.stamp.cmp(&a.stamp));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structure::{Facing, Subject};

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "mytripo_public-history-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn structure() -> Structure {
        Structure {
            subject: Subject::Other,
            height_m: 1.1,
            side_facing: Facing::Left,
            images: Vec::new(),
            parts: Vec::new(),
            blocks: Vec::new(),
        }
    }

    #[test]
    fn saved_record_lists_newest_first_and_keeps_old_sheets() {
        let dir = tmp("list");
        let outside = tmp("outside").join("photo.jpg");
        std::fs::write(&outside, b"jpg").unwrap();
        let thumb = image::RgbaImage::new(4, 4);
        // 옛 시트(기록 없음), 시트로 만든 메시, 사진으로 만든 메시
        let old = dir.join("20261122-200000-sheet.png");
        std::fs::write(&old, b"png").unwrap();
        let sheet = dir.join("20261122-210000-sheet.png");
        std::fs::write(&sheet, b"png").unwrap();
        let glb1 = dir.join("20261122-210100.glb");
        std::fs::write(&glb1, b"glb").unwrap();
        let slots = Slots {
            front: Some(0),
            side: Some(1),
            back: Some(2),
            top: Some(3),
        };
        save(
            &dir,
            &glb1,
            &Input::Sheet {
                path: sheet.clone(),
            },
            "오토바이",
            &structure(),
            slots,
            &thumb,
        )
        .unwrap();
        let glb2 = dir.join("20261122-220000.glb");
        std::fs::write(&glb2, b"glb").unwrap();
        save(
            &dir,
            &glb2,
            &Input::Photos {
                paths: vec![outside.clone()],
            },
            "",
            &structure(),
            Slots::default(),
            &thumb,
        )
        .unwrap();

        let l = list(&dir);
        let stamps: Vec<&str> = l.iter().map(|e| e.stamp.as_str()).collect();
        assert_eq!(
            stamps,
            ["20261122-220000", "20261122-210100", "20261122-200000"]
        );
        // 사진은 dir 로 복사되어 기록된다.
        let copied = dir.join("20261122-220000-input-0.jpg");
        assert_eq!(
            l[0].input,
            Input::Photos {
                paths: vec![copied.clone()]
            }
        );
        assert_eq!(std::fs::read(&copied).unwrap(), b"jpg");
        assert_eq!(l[0].title(), "사진 1장");
        assert_eq!(l[0].thumb, dir.join("20261122-220000.thumb.png"));
        // dir 안의 시트는 그대로 쓰고, 기록이 쓰는 시트는 옛 시트로 따로 나오지 않는다.
        assert_eq!(l[1].input, Input::Sheet { path: sheet });
        assert_eq!(l[1].record.as_ref().unwrap().slots, slots);
        assert_eq!(l[1].title(), "오토바이");
        assert_eq!(l[1].time(), "11-22 21:01");
        assert!(l[2].record.is_none() && l[2].glb.is_none());
        assert_eq!(l[2].thumb, old);
    }

    #[test]
    fn missing_dir_lists_nothing() {
        assert!(list(Path::new("/nonexistent/mytripo_public-history")).is_empty());
    }
}
