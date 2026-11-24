//! 구조 읽기 응답(JSON). 스키마는 schema/structure.schema.json 이다.

use serde::{Deserialize, Serialize};

pub const SCHEMA: &str = include_str!("../schema/structure.schema.json");
const PROMPT: &str = include_str!("../schema/prompt.txt");

/// 사진 파일들에 대한 구조 읽기 프롬프트. 사진 번호는 목록 순서다.
pub fn prompt(images: &[std::path::PathBuf]) -> String {
    let n = images.len();
    let list: Vec<String> = images
        .iter()
        .enumerate()
        .map(|(i, p)| format!("- index {i}: {}", p.display()))
        .collect();
    PROMPT
        .replace("{N}", &n.to_string())
        .replace("{LAST}", &n.saturating_sub(1).to_string())
        .replace("{IMAGES}", &list.join("\n"))
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum View {
    Front,
    Side,
    Back,
    /// 시트의 넷째 칸. 구조 읽기에는 넣지 않고 시트 위치로 정한다.
    Top,
}

impl View {
    pub const ALL: [View; 3] = [View::Front, View::Side, View::Back];

    pub fn label(self) -> &'static str {
        match self {
            View::Front => "앞",
            View::Side => "옆",
            View::Back => "뒤",
            View::Top => "위",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Subject {
    Human,
    Other,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Facing {
    Left,
    Right,
    None,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PartKind {
    Head,
    Torso,
    Arm,
    Leg,
    Prop,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Left,
    Right,
    Center,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Joint {
    pub name: String,
    pub x: f32,
    pub y: f32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ImageInfo {
    pub index: usize,
    pub view: View,
    pub joints: Vec<Joint>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Part {
    pub name: String,
    pub kind: PartKind,
    pub side: Side,
    pub chain: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Shape {
    /// 축이 옆(X)을 향한 원판: 바퀴.
    Wheel,
    /// 옆 윤곽을 좌우로 밀어낸 덩어리.
    Block,
    /// 앞 윤곽을 앞뒤로 밀어낸 가로 막대: 핸들, 미러, 가로로 놓인 짐.
    Bar,
}

/// 사물의 부품 하나. 옆 사진에서 부품을 감싸는 상자와 앞 사진에서 부품의 가로 범위(0..1, 왼쪽 위 원점).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Block {
    pub name: String,
    pub shape: Shape,
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub front_x0: f32,
    pub front_x1: f32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Structure {
    pub subject: Subject,
    pub height_m: f32,
    pub side_facing: Facing,
    pub images: Vec<ImageInfo>,
    pub parts: Vec<Part>,
    /// 사람이 아닌 대상의 부품. 옛 응답에는 없다.
    #[serde(default)]
    pub blocks: Vec<Block>,
}

impl Structure {
    /// 응답을 읽고 사진 수에 맞는지 검사한다. 범위를 벗어난 값은 고친다.
    pub fn parse(json: &str, n_images: usize) -> Result<Self, String> {
        let mut s: Structure =
            serde_json::from_str(json).map_err(|e| format!("구조 응답을 읽지 못했습니다: {e}"))?;
        s.images.retain(|i| i.index < n_images);
        let mut seen = vec![false; n_images];
        s.images
            .retain(|i| !std::mem::replace(&mut seen[i.index], true));
        if !(0.05..=20.0).contains(&s.height_m) || !s.height_m.is_finite() {
            s.height_m = 1.7;
        }
        for img in &mut s.images {
            for j in &mut img.joints {
                j.x = if j.x.is_finite() {
                    j.x.clamp(0.0, 1.0)
                } else {
                    0.5
                };
                j.y = if j.y.is_finite() {
                    j.y.clamp(0.0, 1.0)
                } else {
                    0.5
                };
            }
        }
        s.parts.retain(|p| p.chain.len() >= 2);
        if s.parts.is_empty() {
            return Err("구조 응답에 부위가 없습니다".into());
        }
        Ok(s)
    }

    pub fn image(&self, index: usize) -> Option<&ImageInfo> {
        self.images.iter().find(|i| i.index == index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_is_strict_json_schema() {
        let v: serde_json::Value = serde_json::from_str(SCHEMA).unwrap();
        assert_eq!(v["additionalProperties"], false);
        assert_eq!(v["required"].as_array().unwrap().len(), 6);
    }

    #[test]
    fn prompt_lists_images_in_order() {
        let p = prompt(&["/w/a.png".into(), "/w/b.png".into()]);
        assert!(p.contains("You are given 2 photo(s)"));
        assert!(p.contains("indices are 0 to 1"));
        assert!(p.contains("- index 0: /w/a.png\n- index 1: /w/b.png"));
        assert!(!p.contains('{'));
    }

    #[test]
    fn parse_drops_bad_values() {
        let json = r##"{"subject":"human","height_m":-3,"side_facing":"none",
            "images":[{"index":0,"view":"front","joints":[{"name":"neck","x":1.4,"y":0.2}]},
                      {"index":0,"view":"back","joints":[]},{"index":7,"view":"side","joints":[]}],
            "parts":[{"name":"torso","kind":"torso","side":"center","chain":["pelvis","neck"],"colors":["#ff0000"]},
                     {"name":"x","kind":"prop","side":"center","chain":["a"],"colors":[]}]}"##;
        let s = Structure::parse(json, 1).unwrap();
        assert_eq!(s.height_m, 1.7);
        assert_eq!(s.images.len(), 1);
        assert_eq!(s.images[0].view, View::Front);
        assert_eq!(s.images[0].joints[0].x, 1.0);
        assert_eq!(s.parts.len(), 1);
    }
}
