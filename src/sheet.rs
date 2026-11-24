//! 턴어라운드 시트: 생성 프롬프트(3D 모델링 규칙)와 세 시점 자르기.

use std::path::Path;

use image::RgbImage;

use crate::mask;
use crate::pipeline::{self, Photo};

/// 시트 크기. 네 시점(세로 3:4 칸 넷)이 들어가는 가로 이미지다. 16 의 배수, 비율 3:1 이내.
pub const SIZE: &str = "3072x1024";

/// 그림의 방향. 모든 시트 프롬프트에 붙는다.
pub const STYLE: &str = include_str!("../schema/style.txt");

/// 로우폴리 모델링 레퍼런스로 쓸 수 있게 하는 규칙. 앱의 마스크와 단면 측정이 이 규칙을 가정한다.
const RULES: &str = "Character turnaround model sheet, made as the reference for building a low-poly 3D model.
- Layout: exactly four views of the same single subject in one row, left to right: FRONT view, SIDE view (profile facing the left edge of the image), BACK view, TOP view (seen from directly above, the front of the subject pointing to the bottom edge of the image). Nothing else in the image.
- Camera: orthographic projection with no perspective distortion. Front, side and back cameras are level at the subject's mid-height; the top camera looks straight down. All four views use the same scale. Front, side and back views have the same height, the top of the subject on one line and the bottom on one common baseline.
- Pose: if the subject is a person, a humanoid or an upright creature, use a neutral A-pose: arms straight and 30 to 40 degrees away from the torso, hands open and not touching the body, legs straight with a clear gap between them, feet flat, head looking straight ahead. Hands are simple closed mitten shapes: fingers together and straight, thumb along the hand, no spread or bent fingers, no gestures. The side profile shows the nose, brow and chin clearly. Left and right sides are symmetric. The front and back views have mirror-image silhouettes.
- Base body: if the subject is a person or a humanoid, draw the bare base body that equipment is fitted onto later. No clothes, coats, armor, hats, shoes, gloves, belts, bags, jewelry or weapons, even if the subject description mentions them; those are separate equipment. The body wears only plain skin-tight neutral gray underwear (simple shorts, plus a simple sports top for a woman) and has smooth mannequin-like anatomy without detail. Bare feet. Keep the subject's own hairstyle.
- Shapes: clear readable volumes, large flat color regions, simple materials, no fine texture, no loose hair strands, no thin floating accessories, no motion blur. Every part is fully visible; nothing is cropped and the views do not overlap.
- Lighting: flat even studio light. No cast shadows, no floor shadow, no reflections, no rim light.
- Background: one solid pure chroma-key green (#00B140) everywhere. Leave wide empty green gaps between the views and along every image edge, at least 5% of the image width. No floor, no gradient, no text, no labels, no arrows, no grid, no frame.";

/// 새 시트 프롬프트.
pub fn new_prompt(description: &str, refs: usize) -> String {
    let subject = description.trim();
    let mut p = String::from(STYLE);
    if !subject.is_empty() {
        p.push_str(&format!("Subject: {subject}\n"));
    }
    if refs > 0 {
        p.push_str(
            "The attached image(s) show the subject. Keep its identity, proportions and colors, but redraw it so that it follows every rule below. For a person keep the face, hair, skin and body shape, and leave out the clothing and gear (base body rule).\n",
        );
    }
    p.push_str(RULES);
    p
}

/// 시트 수정 프롬프트. 지시한 것만 바꾸고 규칙은 지킨다.
pub fn edit_prompt(instruction: &str) -> String {
    format!(
        "Edit the attached turnaround sheet. Change only this: {}\nKeep the same subject and keep every rule below.\n{STYLE}{RULES}",
        instruction.trim()
    )
}

/// 자세 시트의 규칙. 붙인 턴어라운드 시트의 인물을 한 자세로 세 시점에서 그린다. 동작을 만들 때의 참고 그림이다.
const POSE_RULES: &str = "Pose reference sheet for animating a 3D model of the character in the attached turnaround sheet.
- Character: exactly the same character as the attached sheet: same face, hairstyle, body proportions, skin and plain gray underwear. Bare feet. No clothes, gear or props.
- Layout: exactly three views of the same single moment of the same pose in one row, left to right: FRONT view, SIDE view, THREE-QUARTER view. Nothing else in the image.
- Cameras are fixed in the world, the same as the turnaround sheet: the FRONT camera stands where the turnaround FRONT camera stood, the SIDE camera stands at the character's left side, the THREE-QUARTER camera stands halfway between those two. The character may turn its body as the pose requires; never rotate the character to face each camera.
- Camera: orthographic projection, level at mid-height, the same scale in all views, feet on one common baseline. The whole body is visible in every view.
- Hands: show the hand shape the pose needs (bent, spread or clawed fingers are fine).
- Shapes: clear readable volumes, no motion blur, no speed lines, no effects.
- Lighting: flat even studio light. No cast shadows, no floor shadow.
- Background: one solid pure chroma-key green (#00B140) everywhere. Leave wide empty green gaps between the views and along every image edge, at least 5% of the image width. No floor, no text, no labels, no arrows, no grid, no frame.";

/// 자세 시트 프롬프트. 붙인 턴어라운드 시트의 인물이 설명한 자세를 한다.
pub fn pose_prompt(description: &str) -> String {
    format!(
        "{STYLE}Pose: {}
{POSE_RULES}",
        description.trim()
    )
}

/// 시트를 세로 빈 띠에서 잘라 왼쪽부터 앞·옆·뒤(·위) 칸으로 나눈다. 칸은 3개 또는 4개다.
pub fn split(img: &RgbImage) -> Result<Vec<RgbImage>, String> {
    let (fg, _) = mask::foreground(img).map_err(|e| format!("시트: {e}"))?;
    let (w, h) = (fg.w, fg.h);
    let noise = (h / 200).max(2);
    let col: Vec<usize> = (0..w)
        .map(|x| (0..h).filter(|&y| fg.px[y * w + x]).count())
        .collect();
    // 피사체가 있는 열의 구간: (시작, 끝(제외), 픽셀 수)
    let mut segs: Vec<(usize, usize, usize)> = Vec::new();
    let mut x = 0;
    while x < w {
        if col[x] <= noise {
            x += 1;
            continue;
        }
        let s = x;
        let mut sum = 0;
        while x < w && col[x] > noise {
            sum += col[x];
            x += 1;
        }
        segs.push((s, x, sum));
    }
    let biggest = segs.iter().map(|s| s.2).max().unwrap_or(0);
    segs.retain(|s| s.2 * 200 >= biggest);
    // 시점 사이 여백은 넓다(프롬프트: 폭의 5% 이상). 폭의 1/60 보다 좁은 틈은 한 시점 안의 틈(팔과 몸 사이)이다.
    let mut k = 0;
    while k + 1 < segs.len() {
        if segs[k + 1].0 - segs[k].1 < w / 60 {
            let next = segs.remove(k + 1);
            segs[k] = (segs[k].0, next.1, segs[k].2 + next.2);
        } else {
            k += 1;
        }
    }
    // 그래도 칸이 5개 이상이면 가장 좁은 틈부터 합친다.
    while segs.len() > 4 {
        let k = (0..segs.len() - 1)
            .min_by_key(|&k| segs[k + 1].0 - segs[k].1)
            .unwrap_or(0);
        let next = segs.remove(k + 1);
        segs[k] = (segs[k].0, next.1, segs[k].2 + next.2);
    }
    if segs.len() < 3 {
        return Err(format!(
            "시트에서 앞·옆·뒤 시점을 찾지 못했습니다 (찾은 시점 {}개). 시점 사이에 빈 배경이 있게 수정하세요",
            segs.len()
        ));
    }
    let mut bounds = Vec::new();
    let mut x0 = 0;
    for k in 0..segs.len() {
        let x1 = if k + 1 < segs.len() {
            (segs[k].1 + segs[k + 1].0) / 2
        } else {
            w
        };
        bounds.push((x0, x1));
        x0 = x1;
    }
    Ok(bounds
        .iter()
        .map(|&(x0, x1)| {
            image::imageops::crop_imm(img, x0 as u32, 0, (x1 - x0) as u32, h as u32).to_image()
        })
        .collect())
}

/// 시트 파일을 읽어 칸 사진(앞·옆·뒤·위 순)으로 만든다. 위 칸은 없을 수 있다.
pub fn load(path: &Path) -> Result<Vec<Photo>, String> {
    let img = pipeline::decode(path)?.to_rgb8();
    split(&img)?
        .into_iter()
        .zip(["시트 앞", "시트 옆", "시트 뒤", "시트 위"])
        .map(|(panel, name)| {
            pipeline::photo_from_image(name.into(), image::DynamicImage::ImageRgb8(panel))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    #[test]
    fn split_finds_three_views_and_ignores_specks() {
        let mut img = RgbImage::from_pixel(300, 100, Rgb([0, 177, 64]));
        for (x0, x1) in [(20, 60), (120, 150), (210, 280)] {
            for y in 10..90 {
                for x in x0..x1 {
                    img.put_pixel(x, y, Rgb([200, 50, 50]));
                }
            }
        }
        // 두 번째 시점 안의 빈 열(팔과 몸 사이)과 떨어진 잡티
        for y in 10..90 {
            img.put_pixel(135, y, Rgb([0, 177, 64]));
        }
        img.put_pixel(95, 50, Rgb([10, 10, 10]));
        let panels = split(&img).unwrap();
        assert_eq!(panels.len(), 3);
        let widths: Vec<u32> = panels.iter().map(|p| p.width()).collect();
        assert_eq!(widths.iter().sum::<u32>(), 300);
        assert_eq!(widths, vec![90, 90, 120]);
    }

    #[test]
    fn split_keeps_a_fourth_top_view() {
        let mut img = RgbImage::from_pixel(400, 100, Rgb([0, 177, 64]));
        for (x0, x1) in [(20, 60), (120, 150), (210, 260), (300, 380)] {
            for y in 10..90 {
                for x in x0..x1 {
                    img.put_pixel(x, y, Rgb([200, 50, 50]));
                }
            }
        }
        let widths: Vec<u32> = split(&img).unwrap().iter().map(|p| p.width()).collect();
        assert_eq!(widths, vec![90, 90, 100, 120]);
    }

    #[test]
    fn split_rejects_two_views() {
        let mut img = RgbImage::from_pixel(200, 100, Rgb([0, 177, 64]));
        for (x0, x1) in [(20, 60), (120, 150)] {
            for y in 10..90 {
                for x in x0..x1 {
                    img.put_pixel(x, y, Rgb([200, 50, 50]));
                }
            }
        }
        assert!(split(&img).is_err());
    }

    #[test]
    fn prompts_carry_modeling_rules() {
        let p = new_prompt("a robot", 0);
        assert!(p.starts_with("STYLE: realistic adult proportions"));
        assert!(p.contains("Subject: a robot"));
        assert!(edit_prompt("x").contains("at least 6 heads tall"));
        for rule in [
            "orthographic",
            "A-pose",
            "#00B140",
            "No cast shadows",
            "FRONT view, SIDE view",
            "TOP view",
            "mitten",
            "readable volumes",
            "base body",
            "No clothes",
        ] {
            assert!(p.contains(rule), "{rule}");
        }
        assert!(edit_prompt("bigger head").contains("Change only this: bigger head"));
    }

    #[test]
    fn pose_prompt_keeps_the_character_and_fixes_cameras() {
        let p = pose_prompt(" hunched guard, right forearm across the chest ");
        assert!(p.contains("Pose: hunched guard, right forearm across the chest\n"));
        for rule in [
            "same character",
            "FRONT view, SIDE view, THREE-QUARTER view",
            "Cameras are fixed in the world",
            "never rotate the character",
            "orthographic",
            "#00B140",
            "at least 6 heads tall",
        ] {
            assert!(p.contains(rule), "{rule}");
        }
        // 메시용 시트의 자세·손 규칙은 자세 시트에 붙지 않는다.
        assert!(!p.contains("A-pose"));
        assert!(!p.contains("mitten"));
    }
}
