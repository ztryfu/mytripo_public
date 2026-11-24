//! 사진 + 구조 읽기 → 로우폴리 메시. decisions/0001 의 단면 고리 로프트.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use image::{DynamicImage, ImageDecoder, ImageReader, RgbImage, imageops::FilterType};

use crate::geom::V3;
use crate::glb;
use crate::hull;
use crate::mask::{self, Mask};
use crate::mesh::{self, Mesh, Ring};
use crate::structure::{Facing, PartKind, Side, Structure, Subject, View};

/// 삼각형 예산 기본값.
pub const DEFAULT_BUDGET: usize = 2000;
/// 앞·옆·뒤 실루엣 IoU 합격 기준값.
pub const IOU_MIN: f32 = 0.9;
/// 사진의 긴 변을 이 크기로 줄여서 쓴다.
pub const MAX_SIDE: u32 = 1024;

pub struct Photo {
    pub name: String,
    pub rgb: RgbImage,
    pub mask: Mask,
}

/// 사진을 읽고(EXIF 방향 반영), 줄이고, 배경을 지운다.
pub fn load_photo(path: &Path) -> Result<Photo, String> {
    photo_from_image(file_name(path), decode(path)?)
}

pub fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// 이미지 파일을 읽고 EXIF 방향을 반영한다.
pub fn decode(path: &Path) -> Result<DynamicImage, String> {
    let name = file_name(path);
    let err = |e: &dyn std::fmt::Display| format!("{name}: {e}");
    let mut dec = ImageReader::open(path)
        .map_err(|e| err(&e))?
        .with_guessed_format()
        .map_err(|e| err(&e))?
        .into_decoder()
        .map_err(|e| err(&e))?;
    let orientation = dec.orientation().map_err(|e| err(&e))?;
    let mut img = DynamicImage::from_decoder(dec).map_err(|e| err(&e))?;
    img.apply_orientation(orientation);
    Ok(img)
}

pub fn photo_from_image(name: String, img: DynamicImage) -> Result<Photo, String> {
    let img = if img.width().max(img.height()) > MAX_SIDE {
        img.resize(MAX_SIDE, MAX_SIDE, FilterType::Triangle)
    } else {
        img
    };
    let rgb = img.to_rgb8();
    let (mask, _) = mask::cut_out(&rgb).map_err(|e| format!("{name}: {e}"))?;
    Ok(Photo { name, rgb, mask })
}

/// 앞·옆·뒤 칸에 놓인 사진 번호.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Slots {
    pub front: Option<usize>,
    pub side: Option<usize>,
    pub back: Option<usize>,
    /// 시트의 위 칸. 사람이 아닌 대상의 visual hull 에만 쓴다.
    pub top: Option<usize>,
}

impl Slots {
    pub fn get(&self, v: View) -> Option<usize> {
        match v {
            View::Front => self.front,
            View::Side => self.side,
            View::Back => self.back,
            View::Top => self.top,
        }
    }

    fn slot_mut(&mut self, v: View) -> &mut Option<usize> {
        match v {
            View::Front => &mut self.front,
            View::Side => &mut self.side,
            View::Back => &mut self.back,
            View::Top => &mut self.top,
        }
    }

    pub fn view_of(&self, photo: usize) -> Option<View> {
        View::ALL.into_iter().find(|&v| self.get(v) == Some(photo))
    }

    /// 사진 순서대로 빈 칸에 넣는다. AI 판별 전의 임시 배치다.
    pub fn in_order(n: usize) -> Self {
        let mut s = Slots::default();
        for (i, v) in View::ALL.into_iter().enumerate().take(n) {
            *s.slot_mut(v) = Some(i);
        }
        s
    }

    /// AI 판별대로 넣는다. 같은 판별이 겹치면 남는 사진은 빈 칸으로 간다.
    pub fn from_structure(st: &Structure, n: usize) -> Self {
        let mut s = Slots::default();
        let mut rest = Vec::new();
        for i in 0..n {
            match st.image(i).map(|im| im.view) {
                Some(v) if s.get(v).is_none() => *s.slot_mut(v) = Some(i),
                _ => rest.push(i),
            }
        }
        for i in rest {
            if let Some(v) = View::ALL.into_iter().find(|&v| s.get(v).is_none()) {
                *s.slot_mut(v) = Some(i);
            }
        }
        s
    }

    /// 구조 읽기 결과와 사진 수로 칸을 정한다. 넷째 사진(시트의 위 칸)은 위 칸이다.
    pub fn for_photos(st: &Structure, n: usize) -> Self {
        let mut s = Self::from_structure(st, n.min(3));
        if n == 4 {
            s.top = Some(3);
        }
        s
    }

    /// 사진을 다른 칸으로 옮긴다. 그 칸의 사진은 옮긴 사진의 옛 칸으로 간다. 바뀌면 true.
    pub fn move_photo(&mut self, photo: usize, to: View) -> bool {
        let from = self.view_of(photo);
        if from == Some(to) {
            return false;
        }
        let other = self.get(to);
        *self.slot_mut(to) = Some(photo);
        if let Some(f) = from {
            *self.slot_mut(f) = other;
        }
        true
    }
}

/// 사진 픽셀 좌표와 세계 좌표(미터, Y-up, 발바닥 원점, 정면 +Z)의 대응.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub view: View,
    /// 픽셀당 미터.
    pub scale: f32,
    /// 세계 가로 좌표 0 의 픽셀 x.
    pub cx: f32,
    /// 세계 세로 좌표 0 의 픽셀 y. 앞·옆·뒤는 발바닥(Y=0), 위는 Z=0 이다.
    pub bottom: f32,
    /// 세계 가로 좌표가 늘 때 픽셀 x 가 느는 방향(+1/−1).
    pub sign: f32,
    /// 세계 세로 좌표가 늘 때 픽셀 y 가 느는 방향. 앞·옆·뒤는 −1(Y 가 위), 위는 +1 또는 −1(Z).
    pub vsign: f32,
}

impl Frame {
    /// 앞·뒤 사진은 X, 옆 사진은 Z 가 가로 좌표다.
    pub fn new(view: View, mask: &Mask, height_m: f32, facing: Facing) -> Option<Self> {
        let b = mask.bbox()?;
        let cx = match view {
            // 옆: 발바닥이 바닥에 닿는 띠의 가운데
            View::Side => mask.mean_x(b.y1 + 1 - (b.height() / 30).max(1), b.y1)?,
            _ => mask.mean_x(b.y0, b.y1)?,
        };
        let sign = match view {
            View::Front | View::Top => 1.0,
            View::Back => -1.0,
            View::Side if facing == Facing::Left => -1.0,
            View::Side => 1.0,
        };
        Some(Frame {
            view,
            scale: height_m / b.height() as f32,
            cx,
            bottom: (b.y1 + 1) as f32,
            sign,
            vsign: -1.0,
        })
    }

    /// 위 칸: 피사체의 Z 범위(z0..z1, 옆 칸에서 잰 값)가 마스크의 세로 범위와 맞는다.
    /// front_down 이면 피사체의 앞(+Z)이 그림의 아래쪽이다.
    pub fn top(mask: &Mask, z0: f32, z1: f32, front_down: bool) -> Option<Self> {
        let b = mask.bbox()?;
        let scale = (z1 - z0) / b.height() as f32;
        let (bottom, vsign) = if front_down {
            (b.y0 as f32 - z0 / scale, 1.0)
        } else {
            ((b.y1 + 1) as f32 + z0 / scale, -1.0)
        };
        // 가로 중심은 경계 상자의 가운데다. 한쪽에만 있는 부품(머플러)이 평균을 끌어 앞 칸과 어긋난다.
        Some(Frame {
            view: View::Top,
            scale,
            cx: (b.x0 + b.x1 + 1) as f32 / 2.0,
            bottom,
            // 위에서 내려다볼 때 앞이 그림 위쪽이면 피사체의 왼쪽(+X)이 그림의 왼쪽이다.
            sign: if front_down { 1.0 } else { -1.0 },
            vsign,
        })
    }

    pub fn to_px(&self, p: V3) -> (f32, f32) {
        let (h, v) = match self.view {
            View::Side => (p.z, p.y),
            View::Top => (p.x, p.z),
            _ => (p.x, p.y),
        };
        (
            self.cx + self.sign * h / self.scale,
            self.bottom + self.vsign * v / self.scale,
        )
    }

    /// 픽셀 → (세계 가로 좌표, 세계 세로 좌표)
    pub fn from_px(&self, x: f32, y: f32) -> (f32, f32) {
        (
            (x - self.cx) * self.sign * self.scale,
            (y - self.bottom) * self.vsign * self.scale,
        )
    }
}

pub struct Model {
    pub mesh: Mesh,
    /// 메시를 만든 사진마다 (사진 번호, 좌표 대응).
    pub frames: Vec<(usize, Frame)>,
    /// 사람형 메시의 부위별 단면 고리. visual hull 은 비어 있다.
    pub rings: Vec<(PartKind, Vec<Ring>)>,
}

type P2 = (f32, f32);

fn sub(a: P2, b: P2) -> P2 {
    (a.0 - b.0, a.1 - b.1)
}

fn len(a: P2) -> f32 {
    (a.0 * a.0 + a.1 * a.1).sqrt()
}

fn unit(a: P2) -> P2 {
    let l = len(a);
    if l > 1e-9 {
        (a.0 / l, a.1 / l)
    } else {
        (0.0, -1.0)
    }
}

/// 마스크 밖의 점을 가장 가까운 마스크 픽셀로 옮긴다.
fn snap(mask: &Mask, p: P2, radius: f32) -> P2 {
    if mask.at(p.0, p.1) {
        return p;
    }
    let r = radius.ceil() as i64;
    let (px, py) = (p.0.floor() as i64, p.1.floor() as i64);
    let mut best: Option<(i64, P2)> = None;
    for dy in -r..=r {
        for dx in -r..=r {
            let d2 = dx * dx + dy * dy;
            if d2 <= r * r && mask.get(px + dx, py + dy) && best.is_none_or(|(b, _)| d2 < b) {
                best = Some((d2, ((px + dx) as f32 + 0.5, (py + dy) as f32 + 0.5)));
            }
        }
    }
    best.map_or(p, |(_, q)| q)
}

/// from 에서 dir 로 나아가 마스크를 벗어나기 직전의 점.
fn march_to_edge(mask: &Mask, from: P2, dir: P2, max: f32) -> P2 {
    let d = unit(dir);
    let mut t = 0.0;
    let mut last = from;
    while t < max {
        let q = (from.0 + d.0 * t, from.1 + d.1 * t);
        if !mask.at(q.0, q.1) {
            break;
        }
        last = q;
        t += 0.5;
    }
    last
}

/// 세계 XY 평면의 꺾은선.
#[derive(Clone, Debug)]
struct Poly {
    pts: Vec<P2>,
    cum: Vec<f32>,
}

impl Poly {
    fn new(pts: Vec<P2>) -> Self {
        let mut cum = vec![0.0];
        for w in pts.windows(2) {
            cum.push(cum.last().copied().unwrap_or(0.0) + len(sub(w[1], w[0])));
        }
        Poly { pts, cum }
    }

    fn length(&self) -> f32 {
        self.cum.last().copied().unwrap_or(0.0)
    }

    /// s 위치의 (점, 방향, 구간 번호)
    fn at(&self, s: f32) -> (P2, P2, usize) {
        let n = self.pts.len() - 1;
        let mut i = 0;
        while i + 1 < n && s > self.cum[i + 1] {
            i += 1;
        }
        let seg = self.cum[i + 1] - self.cum[i];
        let t = if seg > 1e-9 {
            ((s - self.cum[i]) / seg).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (a, b) = (self.pts[i], self.pts[i + 1]);
        (
            (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t),
            unit(sub(b, a)),
            i,
        )
    }
}

struct PartPlan {
    kind: PartKind,
    /// −1: 세계 X<0 쪽 팔다리, +1: X>0 쪽, 0: 가운데 부위
    side: i8,
    poly: Poly,
}

struct Ctx<'a> {
    height: f32,
    primary: &'a Photo,
    pf: Frame,
    side: Option<(&'a Photo, Frame)>,
    /// 팔의 뼈대(세계 XY). 몸통 폭의 상한을 정한다.
    arms: Vec<Poly>,
}

/// 높이 y 에서 팔 축의 |X|. 팔이 그 높이를 지나지 않으면 None.
fn arm_x_at(arm: &Poly, y: f32) -> Option<f32> {
    arm.pts.windows(2).find_map(|w| {
        let (a, b) = (w[0], w[1]);
        let (lo, hi) = (a.1.min(b.1), a.1.max(b.1));
        if y < lo || y > hi || hi - lo < 1e-6 {
            return None;
        }
        let t = (y - a.1) / (b.1 - a.1);
        Some((a.0 + (b.0 - a.0) * t).abs())
    })
}

/// 반폭 상한. 측정이 이웃 부위로 새는 것을 막는다.
fn max_half(kind: PartKind, ctx: &Ctx, y: f32) -> f32 {
    let h = ctx.height;
    match kind {
        // 머리는 measure_part 가 형태에서 상한을 구한다.
        PartKind::Head | PartKind::Prop => 0.5 * h,
        // 팔이 붙은 높이에서는 팔 축 안쪽까지만 몸통이다.
        PartKind::Torso => ctx
            .arms
            .iter()
            .filter_map(|a| arm_x_at(a, y))
            .reduce(f32::min)
            .unwrap_or(0.5 * h),
        PartKind::Arm => 0.05 * h,
        PartKind::Leg => 0.075 * h,
    }
}

/// 부위의 고리를 모두 잰다. 머리는 위쪽 70% 의 최대 반폭을 목 쪽 고리의 상한으로 다시 잰다.
fn measure_part(ctx: &Ctx, part: &PartPlan, s: &[f32]) -> Vec<Ring> {
    let run = |cap: &dyn Fn(usize) -> Option<f32>| {
        let mut prev = None;
        s.iter()
            .enumerate()
            .map(|(i, &si)| {
                let r = measure(ctx, part, si, prev, cap(i));
                prev = Some(r.center.z);
                r
            })
            .collect::<Vec<Ring>>()
    };
    let rings = run(&|_| None);
    if part.kind == PartKind::Arm {
        return trim_tip(part, s, rings);
    }
    if part.kind != PartKind::Head {
        return rings;
    }
    let total = part.poly.length().max(1e-6);
    let upper = |i: usize| s[i] / total >= 0.3;
    let head = (0..rings.len())
        .filter(|&i| upper(i))
        .map(|i| rings[i].ru.0.max(rings[i].ru.1))
        .reduce(f32::max);
    match head {
        Some(m) => run(&|i| (!upper(i)).then_some(m)),
        None => rings,
    }
}

/// 팔 끝(손)에서 손 폭의 절반보다 가는 고리를 잘라낸다. 편 손가락 하나를 따라 가늘게 튀어나오지 않게 한다.
fn trim_tip(part: &PartPlan, s: &[f32], mut rings: Vec<Ring>) -> Vec<Ring> {
    let n = part.poly.pts.len();
    let hand_start = part.poly.cum[n.saturating_sub(2)];
    let half = |r: &Ring| (r.ru.0 + r.ru.1) / 2.0;
    let widest = rings
        .iter()
        .zip(s)
        .filter(|(_, si)| **si >= hand_start)
        .map(|(r, _)| half(r))
        .fold(0.0, f32::max);
    while rings.len() > 2
        && s[rings.len() - 1] > hand_start
        && half(&rings[rings.len() - 1]) < 0.5 * widest
    {
        rings.pop();
    }
    rings
}

/// s 위치의 단면 고리를 사진에서 잰다. cap 이 있으면 max_half 대신 쓴다.
fn measure(ctx: &Ctx, part: &PartPlan, s: f32, prev_z: Option<f32>, cap: Option<f32>) -> Ring {
    let (pos, dir, seg) = part.poly.at(s);
    let last_seg = seg + 2 == part.poly.pts.len();
    let center_part = part.side == 0;
    // 폭 방향: 가운데 부위는 세계 X, 팔다리는 축에 수직
    let u = if center_part {
        (1.0, 0.0)
    } else {
        (-dir.1, dir.0)
    };
    let pf = ctx.pf;
    let mask = &ctx.primary.mask;
    let p = pf.to_px(V3::new(pos.0, pos.1, 0.0));
    let n = unit((pf.sign * u.0, -u.1));
    let inside = |q: P2| {
        mask.at(q.0, q.1)
            && match part.side {
                0 => true,
                sd => pf.from_px(q.0, q.1).0 * sd as f32 >= 0.0,
            }
    };
    let min_half = 0.006 * ctx.height;
    let limit = 0.6 * ctx.height / pf.scale;
    let start = (0..(0.08 * ctx.height / pf.scale) as i32)
        .flat_map(|t| [t as f32, -(t as f32)])
        .map(|t| (p.0 + n.0 * t, p.1 + n.1 * t))
        .find(|&q| inside(q));
    let (a, b) = match start {
        Some(q) => {
            let mut tp = 0.0;
            while tp < limit && inside((q.0 + n.0 * tp, q.1 + n.1 * tp)) {
                tp += 0.5;
            }
            let mut tn = 0.0;
            while tn < limit && inside((q.0 - n.0 * tn, q.1 - n.1 * tn)) {
                tn += 0.5;
            }
            let off = (q.0 - p.0) * n.0 + (q.1 - p.1) * n.1;
            (
                (tn - off).max(0.0) * pf.scale,
                (tp + off).max(0.0) * pf.scale,
            )
        }
        None => (min_half, min_half),
    };
    let cap = cap.unwrap_or_else(|| max_half(part.kind, ctx, pos.1));
    let (shift, half) = if center_part {
        (0.0, ((a + b) / 2.0).min(cap))
    } else if (a + b) / 2.0 <= cap {
        ((b - a) / 2.0, (a + b) / 2.0)
    } else {
        (0.0, a.min(b).min(cap))
    };
    let half = half.max(min_half);
    let cx = if center_part {
        0.0
    } else {
        pos.0 + u.0 * shift
    };
    let cy = pos.1 + u.1 * shift;

    // 깊이: 옆 사진의 같은 높이 행에서 잰다.
    let depth_k = match part.kind {
        PartKind::Leg if last_seg => 3.0,
        PartKind::Arm | PartKind::Leg => 1.25,
        _ => f32::MAX,
    };
    let fallback = match part.kind {
        PartKind::Head => 0.95,
        PartKind::Torso => 0.6,
        PartKind::Prop => 0.8,
        _ => 1.0,
    };
    let (zc, hz) = ctx
        .side
        .and_then(|(ph, sf)| {
            let row = (sf.bottom - cy / sf.scale).floor() as i64;
            let runs = ph.mask.runs(row);
            let pick = match prev_z {
                Some(z) => {
                    let zp = sf.cx + sf.sign * z / sf.scale;
                    runs.iter()
                        .min_by(|r1, r2| dist_run(**r1, zp).total_cmp(&dist_run(**r2, zp)))
                        .copied()
                }
                None => runs
                    .iter()
                    .max_by(|r1, r2| (r1.1 - r1.0).total_cmp(&(r2.1 - r2.0)))
                    .copied(),
            }?;
            let mid = (pick.0 + pick.1) / 2.0;
            Some((
                (mid - sf.cx) * sf.sign * sf.scale,
                (pick.1 - pick.0) / 2.0 * sf.scale,
            ))
        })
        .map(|(z, h)| (z, h.min(depth_k * half).max(min_half)))
        .unwrap_or((0.0, half * fallback));

    Ring {
        center: V3::new(cx, cy, zc),
        u: V3::new(u.0, u.1, 0.0).norm(),
        v: V3::new(0.0, 0.0, 1.0),
        ru: (half, half),
        rv: (hz, hz),
    }
}

fn dist_run(r: (f32, f32), x: f32) -> f32 {
    if x < r.0 {
        r.0 - x
    } else if x > r.1 {
        x - r.1
    } else {
        0.0
    }
}

/// 두 고리의 평균. 축 방향이 반대인 고리는 먼저 뒤집어 맞춘다.
fn avg_ring(a: &Ring, b: &Ring) -> Ring {
    let flip_u = a.u.dot(b.u) < 0.0;
    let flip_v = a.v.dot(b.v) < 0.0;
    let (bu, bru) = if flip_u {
        (-b.u, (b.ru.1, b.ru.0))
    } else {
        (b.u, b.ru)
    };
    let (bv, brv) = if flip_v {
        (-b.v, (b.rv.1, b.rv.0))
    } else {
        (b.v, b.rv)
    };
    Ring {
        center: a.center.lerp(b.center, 0.5),
        u: (a.u + bu).norm(),
        v: (a.v + bv).norm(),
        ru: ((a.ru.0 + bru.0) / 2.0, (a.ru.1 + bru.1) / 2.0),
        rv: ((a.rv.0 + brv.0) / 2.0, (a.rv.1 + brv.1) / 2.0),
    }
}

/// 꺾은선 위 고리 위치(미터). 양 끝과 내부 관절 앞뒤 δ 는 반드시 둔다.
fn mandatory_rings(poly: &Poly) -> Vec<f32> {
    let total = poly.length();
    let mut s = vec![0.0, total];
    for j in 1..poly.pts.len() - 1 {
        let before = poly.cum[j] - poly.cum[j - 1];
        let after = poly.cum[j + 1] - poly.cum[j];
        let d = 0.12 * before.min(after);
        s.push(poly.cum[j] - d);
        s.push(poly.cum[j] + d);
    }
    s.sort_by(f32::total_cmp);
    s.dedup_by(|a, b| (*a - *b).abs() < 1e-5);
    s
}

/// 칸마다 (사진 번호, 좌표 대응).
pub(crate) fn frames(
    photos: &[Photo],
    slots: &Slots,
    st: &Structure,
) -> Result<Vec<(usize, Frame)>, String> {
    let mut frames = Vec::new();
    for v in View::ALL {
        if let Some(i) = slots.get(v) {
            let ph = photos.get(i).ok_or("칸의 사진 번호가 잘못되었습니다")?;
            let f = Frame::new(v, &ph.mask, st.height_m, st.side_facing)
                .ok_or("사진에 피사체가 없습니다")?;
            frames.push((i, f));
        }
    }
    Ok(frames)
}

/// 사진, 칸 배치, 구조 읽기로 메시를 만든다. 사람형은 단면 고리 로프트(0001), 그 밖은 부품 조립(0005)이다.
pub fn build(
    photos: &[Photo],
    slots: &Slots,
    st: &Structure,
    budget: usize,
) -> Result<Model, String> {
    build_with(photos, slots, st, budget, None)
}

/// build 와 같다. 로컬 모델의 메시(raw)가 있으면 그것을 칸에 맞춘다(0006, 0008).
pub fn build_with(
    photos: &[Photo],
    slots: &Slots,
    st: &Structure,
    budget: usize,
    raw: Option<&crate::model::Raw>,
) -> Result<Model, String> {
    let height = st.height_m;
    let frames = frames(photos, slots, st)?;
    // 사람이 아닌 대상은 관절 사슬이 없다. 실루엣 교집합(visual hull)으로 만든다.
    if st.subject == Subject::Other {
        return hull::build(photos, frames, slots.top, st, budget, raw);
    }
    // 사람형도 모델 메시가 있으면 그것을 칸에 맞춘다(0008). 위 칸은 A포즈를 위에서 본 그림이라 쓰지 않는다.
    if raw.is_some() {
        return hull::build(photos, frames, None, st, budget, raw);
    }
    let frame_of = |v: View| frames.iter().find(|(_, f)| f.view == v).copied();
    let (pi, pf) = frame_of(View::Front)
        .or_else(|| frame_of(View::Back))
        .ok_or("앞이나 뒤 사진이 필요합니다")?;
    let primary = &photos[pi];

    // 관절점(세계 XY). 앞 사진의 관절을 먼저 쓰고 없으면 뒤 사진의 관절을 쓴다.
    let mut joints: HashMap<String, P2> = HashMap::new();
    for v in [View::Front, View::Back] {
        let Some((i, f)) = frame_of(v) else { continue };
        let Some(info) = st.image(i) else { continue };
        if !joints.is_empty() {
            break;
        }
        let (w, h) = (photos[i].mask.w as f32, photos[i].mask.h as f32);
        for j in &info.joints {
            joints.insert(j.name.clone(), f.from_px(j.x * w, j.y * h));
        }
    }
    if joints.is_empty() {
        return Err("구조 응답에 앞·뒤 사진의 관절점이 없습니다".into());
    }

    let snap_r = 0.08 * height / pf.scale;
    let mut plans: Vec<PartPlan> = Vec::new();
    for part in &st.parts {
        let world: Vec<P2> = part
            .chain
            .iter()
            .filter_map(|n| joints.get(n).copied())
            .collect();
        if world.len() < 2 {
            continue;
        }
        // 사진 좌표에서 마스크에 맞춘다.
        let mut px: Vec<P2> = world
            .iter()
            .map(|&(x, y)| snap(&primary.mask, pf.to_px(V3::new(x, y, 0.0)), snap_r))
            .collect();
        // 자유단(머리 꼭대기, 손끝, 발바닥)은 실루엣 끝까지 맞춘다. 몸통의 끝(목)은 자유단이 아니다.
        if part.kind != PartKind::Torso {
            let k = px.len();
            let tip_dir = sub(px[k - 1], px[k - 2]);
            let max = 2.0 * len(tip_dir) + 0.1 * height / pf.scale;
            px[k - 1] = march_to_edge(&primary.mask, px[k - 2], tip_dir, max);
        }
        let mut world: Vec<P2> = px.iter().map(|&(x, y)| pf.from_px(x, y)).collect();
        if part.kind == PartKind::Torso {
            let d = sub(world[0], world[1]);
            world.insert(0, (world[0].0 + d.0 * 0.1, world[0].1 + d.1 * 0.1));
        }
        let mean_x = world.iter().map(|p| p.0).sum::<f32>() / world.len() as f32;
        let side = match part.kind {
            PartKind::Head | PartKind::Torso => 0,
            _ if part.side == Side::Center && mean_x.abs() < 0.02 * height => 0,
            _ if mean_x >= 0.0 => 1,
            _ => -1,
        };
        if side == 0 {
            for p in &mut world {
                p.0 = 0.0;
            }
        }
        plans.push(PartPlan {
            kind: part.kind,
            side,
            poly: Poly::new(world),
        });
    }
    if plans.is_empty() {
        return Err("관절점으로 만들 수 있는 부위가 없습니다".into());
    }

    // 좌우 대칭: 같은 종류, 반대쪽, 같은 관절 수인 부위를 짝짓고 뼈대를 평균한다.
    let mut pair: Vec<Option<usize>> = vec![None; plans.len()];
    for i in 0..plans.len() {
        if plans[i].side != 1 || pair[i].is_some() {
            continue;
        }
        if let Some(j) = (0..plans.len()).find(|&j| {
            pair[j].is_none()
                && plans[j].side == -1
                && plans[j].kind == plans[i].kind
                && plans[j].poly.pts.len() == plans[i].poly.pts.len()
        }) {
            pair[i] = Some(j);
            pair[j] = Some(i);
            let avg: Vec<P2> = plans[i]
                .poly
                .pts
                .iter()
                .zip(&plans[j].poly.pts)
                .map(|(a, b)| ((a.0 - b.0) / 2.0, (a.1 + b.1) / 2.0))
                .collect();
            plans[j].poly = Poly::new(avg.iter().map(|p| (-p.0, p.1)).collect());
            plans[i].poly = Poly::new(avg);
        }
    }

    let ctx = Ctx {
        height,
        primary,
        pf,
        side: frame_of(View::Side).map(|(i, f)| (&photos[i], f)),
        arms: plans
            .iter()
            .filter(|p| p.kind == PartKind::Arm)
            .map(|p| p.poly.clone())
            .collect(),
    };

    // 고리 배치: 짝은 한 단위로 센다. 예산 안에서 가장 큰 간격을 반복해서 나눈다.
    let units: Vec<usize> = (0..plans.len())
        .filter(|&i| pair[i].is_none_or(|j| j > i))
        .collect();
    let mult = |i: usize| if pair[i].is_some() { 2 } else { 1 };
    let mut ring_s: Vec<Vec<f32>> = units
        .iter()
        .map(|&i| mandatory_rings(&plans[i].poly))
        .collect();
    let cost = |ring_s: &[Vec<f32>], n: usize| -> usize {
        units
            .iter()
            .zip(ring_s)
            .map(|(&i, s)| mult(i) * mesh::tube_triangles(s.len(), n))
            .sum()
    };
    let n = if cost(&ring_s, 8) <= budget { 8 } else { 6 };
    if cost(&ring_s, n) > budget {
        return Err(format!(
            "삼각형 예산 {budget} 이 부위 수에 비해 너무 작습니다"
        ));
    }
    // 머리는 옆 윤곽(이마·코·턱)이 드러나게 고리 간격을 더 좁게 둔다.
    let min_gap = |i: usize| match plans[i].kind {
        PartKind::Head => 0.012 * height,
        _ => 0.025 * height,
    };
    loop {
        let used = cost(&ring_s, n);
        let best = units
            .iter()
            .enumerate()
            .filter(|&(_, &i)| used + mult(i) * 2 * n <= budget)
            .filter_map(|(u, &i)| {
                ring_s[u]
                    .windows(2)
                    .enumerate()
                    .map(|(k, w)| (u, k, w[1] - w[0]))
                    .filter(|&(_, _, gap)| gap > min_gap(i))
                    .max_by(|a, b| a.2.total_cmp(&b.2))
            })
            .max_by(|a, b| a.2.total_cmp(&b.2));
        match best {
            Some((u, k, _)) => {
                let mid = (ring_s[u][k] + ring_s[u][k + 1]) / 2.0;
                ring_s[u].insert(k + 1, mid);
            }
            _ => break,
        }
    }

    // 고리를 재고, 짝은 평균해서 거울상으로 맞춘다.
    let mut rings: Vec<Vec<Ring>> = vec![Vec::new(); plans.len()];
    for (u, &i) in units.iter().enumerate() {
        let ri = measure_part(&ctx, &plans[i], &ring_s[u]);
        match pair[i] {
            Some(j) => {
                let rj = measure_part(&ctx, &plans[j], &ring_s[u]);
                let avg: Vec<Ring> = ri
                    .iter()
                    .zip(&rj)
                    .map(|(a, b)| avg_ring(a, &b.mirror_x()))
                    .collect();
                rings[j] = avg.iter().map(Ring::mirror_x).collect();
                rings[i] = avg;
            }
            None => rings[i] = ri,
        }
    }

    let mut out = Mesh::default();
    for (i, r) in rings.iter().enumerate() {
        mesh::add_tube(&mut out, r, n, i);
    }
    if out.triangle_count() > budget {
        return Err(format!(
            "삼각형 {} 개가 예산 {budget} 을 넘었습니다",
            out.triangle_count()
        ));
    }
    Ok(Model {
        mesh: out,
        frames,
        rings: plans.iter().map(|p| p.kind).zip(rings).collect(),
    })
}

/// ~/Documents/mytripo_public
pub fn output_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join("Documents").join("mytripo_public")
}

/// ~/Documents/mytripo_public/<yyyyMMdd-HHmmss><suffix>
pub fn output_path(suffix: &str) -> PathBuf {
    output_dir().join(format!(
        "{}{suffix}",
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    ))
}

/// ~/Documents/mytripo_public/<yyyyMMdd-HHmmss>.glb 에 저장하고 경로를 낸다.
pub fn save(mesh: &Mesh) -> Result<PathBuf, String> {
    let dir = output_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = output_path(".glb");
    std::fs::write(&path, glb::encode(mesh)).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_a_photo_swaps_slots() {
        let mut s = Slots {
            front: Some(0),
            side: Some(1),
            back: Some(2),
            top: None,
        };
        assert!(s.move_photo(2, View::Front));
        assert_eq!(
            s,
            Slots {
                front: Some(2),
                side: Some(1),
                back: Some(0),
                top: None,
            }
        );
        assert!(!s.move_photo(1, View::Side));
        let mut t = Slots {
            front: Some(0),
            side: None,
            back: None,
            top: None,
        };
        assert!(t.move_photo(0, View::Back));
        assert_eq!(
            t,
            Slots {
                front: None,
                side: None,
                back: Some(0),
                top: None,
            }
        );
    }

    #[test]
    fn frame_round_trips() {
        let mut m = Mask::new(40, 100);
        for y in 10..90 {
            for x in 10..30 {
                m.set(x, y, true);
            }
        }
        let f = Frame::new(View::Back, &m, 1.6, Facing::None).unwrap();
        assert!((f.scale - 0.02).abs() < 1e-6);
        let (x, y) = f.to_px(V3::new(0.1, 0.4, 0.0));
        let (wx, wy) = f.from_px(x, y);
        assert!((wx - 0.1).abs() < 1e-5 && (wy - 0.4).abs() < 1e-5);
        // 뒤 사진에서 세계 +X(피사체의 왼쪽)는 사진의 왼쪽이다.
        assert!(x < f.cx);
    }

    #[test]
    fn top_frame_mirrors_x_when_the_front_is_up() {
        // 위에서 내려다볼 때 앞(+Z)이 그림 위쪽이면 피사체의 왼쪽(+X)은 그림의 왼쪽이다.
        let mut m = Mask::new(40, 100);
        for y in 10..90 {
            for x in 10..30 {
                m.set(x, y, true);
            }
        }
        let p = V3::new(0.1, 0.0, 0.8);
        let up = Frame::top(&m, 0.0, 1.6, false).unwrap();
        assert!(up.to_px(p).0 < up.cx, "앞이 위: +X 가 그림 왼쪽");
        let down = Frame::top(&m, 0.0, 1.6, true).unwrap();
        assert!(down.to_px(p).0 > down.cx, "앞이 아래: +X 가 그림 오른쪽");
    }
}
