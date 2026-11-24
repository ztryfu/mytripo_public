//! 로컬 image-to-3D 모델(TRELLIS.2)이 만든 메시를 시트의 칸에 맞춘다. decisions/0006, 0008.
//!
//! 모델은 앱 밖의 Python 환경이다(scripts/setup_model.sh). 앱은 사진 한 장(사물은 옆 칸, 사람은 앞 칸)을 넘기고 PLY 를 받는다.
//! 모델 메시의 방향과 크기는 임의다. 칸의 실루엣과 가장 많이 겹치는 방향을 고르고, 키·길이·폭을 칸에 맞춘다.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use image::RgbaImage;

use crate::cli::{self, Cancel};
use crate::geom::V3;
use crate::hull;
use crate::mask;
use crate::mesh::Mesh;
use crate::pipeline::{self, Frame, Photo, Slots};
use crate::raster;
use crate::structure::{Subject, View};

/// 모델 메시의 삼각형 예산 기본값.
pub const BUDGET: usize = 50_000;
/// 다시 맞출 때 쓰려고 보관하는 모델 메시의 삼각형 수 상한.
const KEEP: usize = 100_000;
/// 칸과의 겹침을 잴 때 쓰는 메시의 삼각형 수.
const SCORE_TRIS: usize = 8_000;
/// 모델 메시의 잔 굴곡(격자 계단)을 펴는 횟수.
const SMOOTH: usize = 3;
/// 이보다 작은 덩어리는 떠 있는 조각으로 보고 버린다(전체 겉넓이 대비).
const MIN_SHARE: f32 = 0.002;
/// 길이와 폭을 칸에 맞출 때 키의 배율에서 벗어나도 되는 범위.
const STRETCH: (f32, f32) = (0.75, 1.33);
/// 자리를 다듬을 때 한 번에 바꾸는 폭(배율은 비율, 위치는 키 대비). 큰 폭부터 쓴다.
const REFINE: [f32; 4] = [0.04, 0.02, 0.01, 0.005];
/// 피사체 둘레의 여백(픽셀)과 가장자리를 깎는 폭(픽셀).
const PAD: usize = 16;
const ERODE: i64 = 2;
/// 첫 실행은 가중치를 내려받는다.
const TIMEOUT: Duration = Duration::from_secs(90 * 60);
/// 모델 폴더 안의 TRELLIS.2 Apple Silicon 판(trellis-mac) 폴더 이름.
const REPO: &str = "trellis-mac";
const RUNNER: &str = include_str!("../scripts/trellis_run.py");

/// 모델이 낸 메시. 좌표계는 모델의 것(Z-up, 크기 임의)이다.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Raw {
    pub positions: Vec<V3>,
    pub tris: Vec<[u32; 3]>,
    /// 이 메시를 만든 seed. PLY 의 `comment seed N` 줄에 남긴다.
    pub seed: u32,
}

/// 모델 폴더. MYTRIPO_PUBLIC_MODEL_DIR 이 없으면 ~/Documents/mytripo_public/model 이다.
pub fn dir() -> PathBuf {
    std::env::var_os("MYTRIPO_PUBLIC_MODEL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| pipeline::output_dir().join("model"))
}

fn python() -> Option<PathBuf> {
    let d = dir();
    let py = d.join(REPO).join(".venv/bin/python");
    (py.is_file() && d.join(REPO).join("TRELLIS.2/trellis2").is_dir()).then_some(py)
}

/// 모델 환경이 설치되어 있는지.
pub fn available() -> bool {
    python().is_some()
}

/// 모델에 넘길 그림: 피사체만 남긴 RGBA.
pub fn input_image(photo: &Photo) -> Result<RgbaImage, String> {
    let m = &photo.mask;
    let b = m.bbox().ok_or("사진에 피사체가 없습니다")?;
    let (x0, y0) = (b.x0.saturating_sub(PAD), b.y0.saturating_sub(PAD));
    let (x1, y1) = ((b.x1 + PAD + 1).min(m.w), (b.y1 + PAD + 1).min(m.h));
    // 마스크는 작은 구멍(바퀴살 사이)을 메운 것이다. 모델에는 구멍을 살려서 넘긴다.
    let (open, _) = mask::foreground(&photo.rgb)?;
    // 가장자리의 배경색 번짐을 빼려고 조금 깎는다.
    let inside = |x: usize, y: usize| {
        m.get(x as i64, y as i64)
            && (-ERODE..=ERODE)
                .all(|dy| (-ERODE..=ERODE).all(|dx| open.get(x as i64 + dx, y as i64 + dy)))
    };
    Ok(RgbaImage::from_fn(
        (x1 - x0) as u32,
        (y1 - y0) as u32,
        |x, y| {
            let (sx, sy) = (x as usize + x0, y as usize + y0);
            let p = photo.rgb.get_pixel(sx as u32, sy as u32);
            let a = if inside(sx, sy) { 255 } else { 0 };
            image::Rgba([p[0], p[1], p[2], a])
        },
    ))
}

/// 모델에 넣는 사진 번호. 이 값이 같으면 모델 메시를 다시 만들지 않는다.
pub type Key = Option<usize>;

/// 사물은 옆 칸을, 사람은 앞 칸을 넣는다. 사람의 앞 칸은 팔다리가 모두 보인다.
pub fn key(slots: &Slots, subject: Subject) -> Key {
    match subject {
        Subject::Human => slots.front,
        Subject::Other => slots.side,
    }
}

/// seed 의 처음 값. 사용자가 다른 형태를 고르면 1씩 올린다.
pub const SEED: u32 = 42;

/// 옆 칸 사진 한 장으로 모델 메시를 만든다. seed 가 다르면 가려진 쪽의 형태가 달라진다.
/// 취소되면 하위 프로세스를 끝낸다.
pub fn generate(photos: &[Photo], key: Key, seed: u32, cancel: &Cancel) -> Result<Raw, String> {
    let py =
        python().ok_or("로컬 모델이 설치되지 않았습니다. scripts/setup_model.sh 를 실행하세요")?;
    let photo = key
        .and_then(|i| photos.get(i))
        .ok_or("모델로 만들려면 사물은 옆 사진, 사람은 앞 사진이 필요합니다")?;
    let work = cli::work_dir("model")?;
    let result = run(&py, &work, photo, seed, cancel);
    let _ = std::fs::remove_dir_all(&work);
    result
}

fn run(py: &Path, work: &Path, photo: &Photo, seed: u32, cancel: &Cancel) -> Result<Raw, String> {
    let d = dir();
    let (img, out, script) = (
        work.join("input.png"),
        work.join("mesh.ply"),
        work.join("trellis_run.py"),
    );
    input_image(photo)?.save(&img).map_err(|e| e.to_string())?;
    std::fs::write(&script, RUNNER).map_err(|e| e.to_string())?;
    let mut cmd = Command::new(py);
    cmd.arg("-u")
        .arg(&script)
        .arg(d.join(REPO))
        .arg(&out)
        .arg(&img)
        .arg(seed.to_string())
        .env("PYTORCH_ENABLE_MPS_FALLBACK", "1");
    cli::run(
        cmd,
        None,
        &work.join("out.log"),
        &work.join("err.log"),
        cancel,
        TIMEOUT,
    )?;
    let bytes = std::fs::read(&out).map_err(|e| format!("모델 출력을 읽지 못했습니다: {e}"))?;
    let mut raw = clean(read_ply(&bytes)?)?;
    raw.seed = seed;
    Ok(raw)
}

fn scalar_size(ty: &str) -> Option<usize> {
    Some(match ty {
        "char" | "uchar" | "int8" | "uint8" => 1,
        "short" | "ushort" | "int16" | "uint16" => 2,
        "int" | "uint" | "float" | "int32" | "uint32" | "float32" => 4,
        "double" | "float64" => 8,
        _ => return None,
    })
}

/// binary little-endian PLY 를 읽는다. 정점의 x·y·z(float)와 삼각형 면만 쓴다.
pub fn read_ply(bytes: &[u8]) -> Result<Raw, String> {
    let bad = |what: &str| format!("PLY 를 읽지 못했습니다: {what}");
    let end = bytes
        .windows(11)
        .position(|w| w == b"end_header\n")
        .ok_or_else(|| bad("헤더가 끝나지 않습니다"))?;
    let header = std::str::from_utf8(&bytes[..end]).map_err(|_| bad("헤더"))?;
    if !header.contains("format binary_little_endian") {
        return Err(bad("binary little-endian 이 아닙니다"));
    }
    let (mut nv, mut nf) = (0usize, 0usize);
    let mut seed = SEED;
    // 정점 속성: (이름, 바이트 수). 면 속성: (개수 형의 크기, 번호 형의 크기).
    let mut vprops: Vec<(&str, usize)> = Vec::new();
    let mut face: Option<(usize, usize)> = None;
    let mut elem = "";
    for line in header.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        match t.as_slice() {
            ["comment", "seed", n] => seed = n.parse().unwrap_or(SEED),
            ["element", name, n] => {
                elem = name;
                let n = n.parse().map_err(|_| bad("element 수"))?;
                match *name {
                    "vertex" => nv = n,
                    "face" => nf = n,
                    _ if n > 0 => return Err(bad("모르는 element 가 있습니다")),
                    _ => {}
                }
            }
            ["property", "list", c, i, _] if elem == "face" => {
                face = scalar_size(c).zip(scalar_size(i));
            }
            ["property", ty, name] if elem == "vertex" => {
                vprops.push((name, scalar_size(ty).ok_or_else(|| bad("속성 형"))?));
            }
            _ => {}
        }
    }
    let stride: usize = vprops.iter().map(|p| p.1).sum();
    let offset = |name: &str| -> Result<usize, String> {
        let k = vprops
            .iter()
            .position(|p| p.0 == name && p.1 == 4)
            .ok_or_else(|| bad("x·y·z 가 float 이 아닙니다"))?;
        Ok(vprops[..k].iter().map(|p| p.1).sum())
    };
    let (ox, oy, oz) = (offset("x")?, offset("y")?, offset("z")?);
    let body = &bytes[end + 11..];
    if body.len() < nv * stride {
        return Err(bad("정점이 모자랍니다"));
    }
    let f32_at = |o: usize| f32::from_le_bytes(body[o..o + 4].try_into().unwrap());
    let positions: Vec<V3> = (0..nv)
        .map(|i| {
            let o = i * stride;
            V3::new(f32_at(o + ox), f32_at(o + oy), f32_at(o + oz))
        })
        .collect();
    let (cs, is) = face.ok_or_else(|| bad("면 목록이 없습니다"))?;
    if cs != 1 || is != 4 {
        return Err(bad("면 목록의 형이 uchar·int 가 아닙니다"));
    }
    let mut tris = Vec::with_capacity(nf);
    let mut o = nv * stride;
    for _ in 0..nf {
        if body.len() < o + 13 || body[o] != 3 {
            return Err(bad("삼각형이 아닌 면이 있습니다"));
        }
        let idx =
            |k: usize| u32::from_le_bytes(body[o + 1 + 4 * k..o + 5 + 4 * k].try_into().unwrap());
        let t = [idx(0), idx(1), idx(2)];
        if t.iter().any(|&i| i as usize >= nv) {
            return Err(bad("면의 정점 번호가 범위를 넘습니다"));
        }
        tris.push(t);
        o += 13;
    }
    Ok(Raw {
        positions,
        tris,
        seed,
    })
}

/// read_ply 가 읽는 형식으로 쓴다.
pub fn write_ply(raw: &Raw) -> Vec<u8> {
    let mut out = format!(
        "ply\nformat binary_little_endian 1.0\ncomment seed {}\nelement vertex {}\nproperty float x\nproperty float y\nproperty float z\nelement face {}\nproperty list uchar int vertex_indices\nend_header\n",
        raw.seed,
        raw.positions.len(),
        raw.tris.len()
    )
    .into_bytes();
    for p in &raw.positions {
        for c in [p.x, p.y, p.z] {
            out.extend_from_slice(&c.to_le_bytes());
        }
    }
    for t in &raw.tris {
        out.push(3);
        for &i in t {
            out.extend_from_slice(&i.to_le_bytes());
        }
    }
    out
}

/// 떠 있는 작은 조각을 버리고 보관할 크기로 줄인다.
pub fn clean(raw: Raw) -> Result<Raw, String> {
    let n = raw.positions.len();
    // 정점을 삼각형으로 이어진 덩어리로 묶는다(union-find).
    let mut parent: Vec<u32> = (0..n as u32).collect();
    fn root(parent: &mut [u32], mut i: u32) -> u32 {
        while parent[i as usize] != i {
            parent[i as usize] = parent[parent[i as usize] as usize];
            i = parent[i as usize];
        }
        i
    }
    for t in &raw.tris {
        let a = root(&mut parent, t[0]);
        for &v in &t[1..] {
            let b = root(&mut parent, v);
            parent[b as usize] = a;
        }
    }
    let area = |t: &[u32; 3]| {
        let [a, b, c] = t.map(|i| raw.positions[i as usize]);
        (b - a).cross(c - a).len() / 2.0
    };
    let mut share = vec![0.0f32; n];
    let mut total = 0.0f32;
    for t in &raw.tris {
        let r = root(&mut parent, t[0]) as usize;
        let a = area(t);
        share[r] += a;
        total += a;
    }
    if total <= 0.0 {
        return Err("모델이 빈 메시를 냈습니다".into());
    }
    let tris: Vec<[u32; 3]> = raw
        .tris
        .iter()
        .filter(|t| share[root(&mut parent, t[0]) as usize] >= MIN_SHARE * total)
        .copied()
        .collect();
    let (mut positions, tris) = hull::simplify(&raw.positions, &tris, KEEP);
    smooth(&mut positions, &tris, SMOOTH);
    Ok(Raw {
        positions,
        tris,
        seed: raw.seed,
    })
}

/// Taubin smoothing: 이웃 평균 쪽으로 당겼다가(λ) 조금 더 밀어낸다(μ). 부피를 지키면서 잔 굴곡만 편다.
fn smooth(pos: &mut [V3], tris: &[[u32; 3]], rounds: usize) {
    let mut near: Vec<Vec<u32>> = vec![Vec::new(); pos.len()];
    for t in tris {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            near[a as usize].push(b);
            near[b as usize].push(a);
        }
    }
    for n in &mut near {
        n.sort_unstable();
        n.dedup();
    }
    for _ in 0..rounds {
        for f in [0.5f32, -0.53] {
            let next: Vec<V3> = pos
                .iter()
                .zip(&near)
                .map(|(&p, n)| {
                    if n.is_empty() {
                        return p;
                    }
                    let sum = n.iter().fold(V3::default(), |a, &i| a + pos[i as usize]);
                    p + (sum * (1.0 / n.len() as f32) - p) * f
                })
                .collect();
            pos.copy_from_slice(&next);
        }
    }
}

/// 사진 칸에서 피사체가 차지하는 세계 가로 범위.
fn h_range(photo: &Photo, f: &Frame) -> Option<(f32, f32)> {
    let b = photo.mask.bbox()?;
    let (a, c) = (
        f.from_px(b.x0 as f32, 0.0).0,
        f.from_px((b.x1 + 1) as f32, 0.0).0,
    );
    Some((a.min(c), a.max(c)))
}

/// 메시의 실루엣과 사진 마스크의 IoU. 마스크는 작은 구멍(바퀴살 사이)을 메운 것이므로 실루엣도 같게 메운다.
pub fn iou(mesh: &Mesh, photo: &Photo, frame: &Frame) -> f32 {
    let mut sil = raster::silhouette(mesh, frame, photo.mask.w, photo.mask.h);
    sil.fill_holes();
    photo.mask.iou(&sil)
}

/// 방향을 정한 모델 메시를 세계 좌표로 옮기는 배율과 이동. 바닥(가장 낮은 점)은 Y=0 에 둔다.
#[derive(Clone, Copy)]
struct Pose {
    /// X, Y, Z 배율.
    s: [f32; 3],
    /// 경계 상자 가운데가 놓일 X 와 Z.
    c: [f32; 2],
}

/// 모델의 Z-up 을 Y-up 으로 눕히고 수직축 둘레로 돌린 점. mirror 면 좌우를 뒤집는다.
fn turn(p: V3, quarter: usize, mirror: bool) -> V3 {
    let (x, z) = match quarter {
        0 => (p.x, -p.y),
        1 => (-p.y, -p.x),
        2 => (-p.x, p.y),
        _ => (p.y, p.x),
    };
    V3::new(if mirror { -x } else { x }, p.z, z)
}

/// 한 방향으로 돌린 메시: 전체 점, 겹침을 잴 때 쓰는 줄인 점, 전체 점의 경계 상자.
struct Turned {
    full: Vec<V3>,
    low: Vec<V3>,
    lo: V3,
    hi: V3,
}

impl Turned {
    fn new(full: &[V3], low: &[V3], quarter: usize, mirror: bool) -> Self {
        let full: Vec<V3> = full.iter().map(|&p| turn(p, quarter, mirror)).collect();
        let low = low.iter().map(|&p| turn(p, quarter, mirror)).collect();
        let (mut lo, mut hi) = (full[0], full[0]);
        for p in &full {
            lo = V3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
            hi = V3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
        }
        Turned { full, low, lo, hi }
    }

    fn place(&self, pts: &[V3], p: &Pose) -> Vec<V3> {
        let (lo, hi) = (self.lo, self.hi);
        pts.iter()
            .map(|v| {
                V3::new(
                    (v.x - (lo.x + hi.x) / 2.0) * p.s[0] + p.c[0],
                    (v.y - lo.y) * p.s[1],
                    (v.z - (lo.z + hi.z) / 2.0) * p.s[2] + p.c[1],
                )
            })
            .collect()
    }
}

/// 모델 메시를 세계 좌표(Y-up, 미터, 발바닥 원점, 정면 +Z)로 옮기고 예산까지 줄인다.
/// 수직축 둘레의 네 방향과 좌우 뒤집기 가운데 칸의 실루엣과 가장 많이 겹치는 것을 고른다.
/// 시트의 옆 칸이 어느 쪽 옆면을 그렸는지는 정해져 있지 않아서 좌우 뒤집기도 본다.
/// 그다음 배율과 위치를 조금씩 바꿔 가며 겹침이 가장 큰 자리를 찾는다.
pub fn fit(
    raw: &Raw,
    views: &[(&Photo, Frame)],
    height: f32,
    budget: usize,
) -> Result<Mesh, String> {
    let (pos, tris) = hull::simplify(&raw.positions, &raw.tris, budget);
    if tris.is_empty() || views.is_empty() {
        return Err("모델 메시를 맞출 사진이 없습니다".into());
    }
    // 겹침은 더 줄인 메시로 잰다. 실루엣은 거의 같고 계산이 빠르다.
    let (low, low_tris) = hull::simplify(&pos, &tris, SCORE_TRIS);
    let range = |v: View| {
        views
            .iter()
            .find(|(_, f)| f.view == v)
            .and_then(|(p, f)| h_range(p, f))
    };
    let length = range(View::Side);
    let width = range(View::Front)
        .or_else(|| range(View::Back))
        .or_else(|| range(View::Top));
    let mid = |t: Option<(f32, f32)>| t.map_or(0.0, |(a, b)| (a + b) / 2.0);
    // 처음 자리: 키를 맞추고, 길이와 폭을 칸의 범위에 맞춘다.
    let first = |t: &Turned| {
        let sy = height / (t.hi.y - t.lo.y).max(1e-6);
        let scale = |ext: f32, target: Option<(f32, f32)>| {
            target.map_or(sy, |(a, b)| {
                ((b - a) / ext.max(1e-6)).clamp(sy * STRETCH.0, sy * STRETCH.1)
            })
        };
        Pose {
            s: [
                scale(t.hi.x - t.lo.x, width),
                sy,
                scale(t.hi.z - t.lo.z, length),
            ],
            c: [mid(width), mid(length)],
        }
    };
    let mut probe = Mesh {
        positions: Vec::new(),
        tris: low_tris,
        parts: Vec::new(),
        normals: Vec::new(),
    };
    let mut score = |positions: Vec<V3>| {
        probe.positions = positions;
        views.iter().map(|(p, f)| iou(&probe, p, f)).sum::<f32>() / views.len() as f32
    };
    let mut best: Option<(f32, Turned, Pose)> = None;
    for quarter in 0..4 {
        for mirror in [false, true] {
            let t = Turned::new(&pos, &low, quarter, mirror);
            let pose = first(&t);
            let s = score(t.place(&t.low, &pose));
            if best.as_ref().is_none_or(|b| s > b.0) {
                best = Some((s, t, pose));
            }
        }
    }
    let (mut top, turned, mut pose) = best.ok_or("모델 메시를 맞추지 못했습니다")?;
    // 배율 셋과 가로 위치 둘을 한 번에 하나씩 바꿔 본다. 더 겹치면 받아들인다.
    for step in REFINE {
        loop {
            let mut better = false;
            for k in 0..5 {
                for dir in [1.0f32, -1.0] {
                    let mut p = pose;
                    match k {
                        0..3 => p.s[k] *= 1.0 + dir * step,
                        _ => p.c[k - 3] += dir * step * height,
                    }
                    let ok = |s: f32| (STRETCH.0..=STRETCH.1).contains(&(s / p.s[1]));
                    if !ok(p.s[0]) || !ok(p.s[2]) {
                        continue;
                    }
                    let s = score(turned.place(&turned.low, &p));
                    if s > top + 1e-4 {
                        (top, pose, better) = (s, p, true);
                    }
                }
            }
            if !better {
                break;
            }
        }
    }
    let mut mesh = Mesh {
        positions: turned.place(&turned.full, &pose),
        parts: vec![0; tris.len()],
        tris,
        normals: Vec::new(),
    };
    // 법선이 바깥을 향하게 한다. 부피가 음수면 면을 뒤집는다.
    let volume: f32 = mesh
        .tris
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|i| mesh.positions[i as usize]);
            a.dot(b.cross(c))
        })
        .sum();
    if volume < 0.0 {
        for t in &mut mesh.tris {
            t.swap(1, 2);
        }
    }
    mesh.normals = mesh.smooth_normals();
    Ok(mesh)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tetra(at: V3, s: f32) -> (Vec<V3>, Vec<[u32; 3]>) {
        (
            vec![
                at,
                at + V3::new(s, 0.0, 0.0),
                at + V3::new(0.0, s, 0.0),
                at + V3::new(0.0, 0.0, s),
            ],
            vec![[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]],
        )
    }

    #[test]
    fn input_image_keeps_only_the_subject() {
        use crate::mask::Mask;
        let mut mask = Mask::new(60, 40);
        for y in 10..30 {
            for x in 20..40 {
                mask.set(x, y, true);
            }
        }
        // 초록 배경 위의 어두운 사각형. 가운데에 배경색 구멍이 하나 있다.
        let mut rgb = image::RgbImage::from_pixel(60, 40, image::Rgb([0, 177, 64]));
        for y in 10..30 {
            for x in 20..40 {
                rgb.put_pixel(x, y, image::Rgb([20, 20, 20]));
            }
        }
        rgb.put_pixel(30, 15, image::Rgb([0, 177, 64]));
        let photo = Photo {
            name: "t".into(),
            rgb,
            mask,
        };
        let img = input_image(&photo).unwrap();
        assert_eq!((img.width(), img.height()), (52, 40));
        // 피사체 가운데는 불투명하고 색이 그대로다. 바깥과 깎인 가장자리는 투명하다.
        assert_eq!(img.get_pixel(26, 22).0, [20, 20, 20, 255]);
        // 마스크가 메운 구멍은 모델에 투명하게 간다.
        assert_eq!(img.get_pixel(26, 15)[3], 0);
        assert_eq!(img.get_pixel(1, 1)[3], 0);
        assert_eq!(img.get_pixel(4, 10)[3], 0);
    }

    #[test]
    fn ply_round_trips() {
        let (positions, tris) = tetra(V3::new(0.5, -1.0, 2.0), 1.0);
        let raw = Raw {
            positions,
            tris,
            seed: 7,
        };
        assert_eq!(read_ply(&write_ply(&raw)).unwrap(), raw);
        assert!(read_ply(b"ply\nformat ascii 1.0\nend_header\n").is_err());
    }

    #[test]
    fn clean_drops_small_floating_pieces() {
        let (mut positions, mut tris) = tetra(V3::default(), 1.0);
        let (p2, t2) = tetra(V3::new(5.0, 0.0, 0.0), 0.01);
        tris.extend(t2.iter().map(|t| t.map(|i| i + positions.len() as u32)));
        positions.extend(p2);
        let out = clean(Raw {
            positions,
            tris,
            seed: SEED,
        })
        .unwrap();
        assert_eq!(out.tris.len(), 4);
        assert!(out.positions.iter().all(|p| p.x <= 1.0));
    }
}
