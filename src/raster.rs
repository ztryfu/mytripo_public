//! CPU 래스터라이저. 테스트의 직교 실루엣과 앱의 3D 미리보기를 그린다.

use crate::geom::V3;
use crate::mask::Mask;
use crate::mesh::Mesh;
use crate::pipeline::Frame;

fn edge(a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> f32 {
    (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
}

/// 픽셀 중심이 삼각형 안에 드는 픽셀마다 f(x, y, 무게중심 좌표)를 부른다.
fn fill_tri(p: [(f32, f32); 3], w: usize, h: usize, mut f: impl FnMut(usize, usize, [f32; 3])) {
    let area = edge(p[0], p[1], p[2]);
    if area.abs() < 1e-9 || w == 0 || h == 0 {
        return;
    }
    let min_x = p
        .iter()
        .map(|q| q.0)
        .fold(f32::MAX, f32::min)
        .floor()
        .max(0.0) as usize;
    let max_x = p
        .iter()
        .map(|q| q.0)
        .fold(f32::MIN, f32::max)
        .ceil()
        .min(w as f32 - 1.0);
    let min_y = p
        .iter()
        .map(|q| q.1)
        .fold(f32::MAX, f32::min)
        .floor()
        .max(0.0) as usize;
    let max_y = p
        .iter()
        .map(|q| q.1)
        .fold(f32::MIN, f32::max)
        .ceil()
        .min(h as f32 - 1.0);
    if max_x < 0.0 || max_y < 0.0 {
        return;
    }
    for y in min_y..=max_y as usize {
        for x in min_x..=max_x as usize {
            let c = (x as f32 + 0.5, y as f32 + 0.5);
            let w0 = edge(p[1], p[2], c) / area;
            let w1 = edge(p[2], p[0], c) / area;
            let w2 = 1.0 - w0 - w1;
            if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 {
                f(x, y, [w0, w1, w2]);
            }
        }
    }
}

/// 메시를 사진 좌표계로 직교 투영한 실루엣.
pub fn silhouette(mesh: &Mesh, frame: &Frame, w: usize, h: usize) -> Mask {
    let mut m = Mask::new(w, h);
    let px: Vec<(f32, f32)> = mesh.positions.iter().map(|&p| frame.to_px(p)).collect();
    for t in &mesh.tris {
        fill_tri(t.map(|i| px[i as usize]), w, h, |x, y, _| m.set(x, y, true));
    }
    m
}

/// 궤도 카메라. 대상 주위를 yaw·pitch 로 돌고 distance 로 확대한다.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub target: V3,
}

const FOV_Y: f32 = 0.6;

impl Camera {
    /// 메시 전체가 보이는 정면 카메라.
    pub fn framing(mesh: &Mesh) -> Self {
        let (lo, hi) = mesh.bounds();
        let size = (hi - lo).len().max(0.01);
        Camera {
            yaw: 0.5,
            pitch: 0.15,
            distance: size * 0.6 / (FOV_Y / 2.0).tan(),
            target: lo.lerp(hi, 0.5),
        }
    }

    fn basis(&self) -> (V3, V3, V3, V3) {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        let eye = self.target + V3::new(cp * sy, sp, cp * cy) * self.distance;
        let f = (self.target - eye).norm();
        let r = f.cross(V3::new(0.0, 1.0, 0.0)).norm();
        let u = r.cross(f);
        (eye, f, r, u)
    }
}

/// 미리보기 배경(위 → 아래)과 회색 clay 색.
const SKY: [f32; 3] = [228.0, 230.0, 233.0];
const FLOOR: [f32; 3] = [188.0, 191.0, 196.0];
const CLAY: [f32; 3] = [196.0, 196.0, 192.0];

/// 원근 투영, z-buffer 로 그린 RGBA 이미지. 밝은 배경에 회색 clay 로 그린다.
/// 메시에 정점 법선이 있으면 픽셀마다 법선을 이어서 음영을 넣고, 없으면 면마다 평평하게 칠한다.
pub fn render(mesh: &Mesh, cam: &Camera, w: usize, h: usize) -> Vec<u8> {
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        let t = y as f32 / h.max(1) as f32;
        let c = [0, 1, 2].map(|k| (SKY[k] + (FLOOR[k] - SKY[k]) * t) as u8);
        for x in 0..w {
            let i = (y * w + x) * 4;
            rgba[i..i + 3].copy_from_slice(&c);
            rgba[i + 3] = 255;
        }
    }
    let (eye, f, r, u) = cam.basis();
    let focal = h as f32 / 2.0 / (FOV_Y / 2.0).tan();
    // 카메라 기준 왼쪽 위에서 오는 주광과 오른쪽에서 오는 보조광
    let key = (-f + u * 0.8 - r * 0.6).norm();
    let fill = (-f * 0.5 + r * 0.9 - u * 0.2).norm();
    let proj: Vec<Option<(f32, f32, f32)>> = mesh
        .positions
        .iter()
        .map(|&p| {
            let d = p - eye;
            let z = d.dot(f);
            (z > 1e-3).then(|| {
                (
                    w as f32 / 2.0 + d.dot(r) / z * focal,
                    h as f32 / 2.0 - d.dot(u) / z * focal,
                    1.0 / z,
                )
            })
        })
        .collect();
    let mut depth = vec![0.0f32; w * h];
    let smooth = !mesh.normals.is_empty() && mesh.normals.len() == mesh.positions.len();
    for t in 0..mesh.triangle_count() {
        let Some(p) = mesh.tris[t]
            .iter()
            .map(|&i| proj[i as usize])
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        let face = mesh.normal(t);
        // 안쪽 면이 보이면 바깥 면처럼 칠한다.
        let flip = if face.dot(f) > 0.0 { -1.0 } else { 1.0 };
        let color = |n: V3| {
            let shade = 0.28 + 0.62 * n.dot(key).max(0.0) + 0.22 * n.dot(fill).max(0.0);
            CLAY.map(|c| (c * shade).min(255.0) as u8)
        };
        let corner = smooth.then(|| mesh.tris[t].map(|i| mesh.normals[i as usize] * flip));
        let flat = color(face * flip);
        let xy = [(p[0].0, p[0].1), (p[1].0, p[1].1), (p[2].0, p[2].1)];
        fill_tri(xy, w, h, |x, y, b| {
            let inv_z = b[0] * p[0].2 + b[1] * p[1].2 + b[2] * p[2].2;
            let i = y * w + x;
            if inv_z > depth[i] {
                depth[i] = inv_z;
                let col = match &corner {
                    Some(n) => color((n[0] * b[0] + n[1] * b[1] + n[2] * b[2]).norm()),
                    None => flat,
                };
                rgba[i * 4..i * 4 + 3].copy_from_slice(&col);
            }
        });
    }
    rgba
}
