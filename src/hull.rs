//! 사람이 아닌 대상: 구조 응답에 부품 목록이 있으면 kit 으로 조립한다(decisions/0005).
//! 없으면 앞·옆·뒤 실루엣의 교집합(visual hull)을 voxel 로 깎고, 표면을 뽑아 예산까지 줄인다. decisions/0003.

use crate::geom::V3;
use crate::mesh::Mesh;
use crate::pipeline::{Frame, Model, Photo};
use crate::structure::{Structure, View};

/// 키 방향 voxel 수.
const RESOLUTION: f32 = 160.0;
/// voxel 수 상한.
const MAX_CELLS: usize = 12_000_000;

/// voxel 격자. 값은 0(밖)~1(안), 경계는 0.5.
struct Grid {
    n: [usize; 3],
    origin: V3,
    step: f32,
    v: Vec<f32>,
}

impl Grid {
    fn idx(&self, i: usize, j: usize, k: usize) -> usize {
        (k * self.n[1] + j) * self.n[0] + i
    }

    fn pos(&self, i: f32, j: f32, k: f32) -> V3 {
        self.origin + V3::new(i, j, k) * self.step
    }

    fn inside(&self, i: usize, j: usize, k: usize) -> bool {
        self.v[self.idx(i, j, k)] > 0.5
    }
}

/// 마스크의 세계 좌표 범위(가로 좌표 min, max).
fn horizontal_range(photo: &Photo, f: &Frame) -> Option<(f32, f32)> {
    let b = photo.mask.bbox()?;
    let a = f.from_px(b.x0 as f32, 0.0).0;
    let c = f.from_px((b.x1 + 1) as f32, 0.0).0;
    Some((a.min(c), a.max(c)))
}

/// 실루엣 교집합 격자. 좌우 대칭으로 평균하고 한 번 흐린다.
fn carve(views: &[(&Photo, Frame)], height: f32) -> Result<Grid, String> {
    let side = views
        .iter()
        .find(|(_, f)| f.view == View::Side)
        .ok_or("사람이 아닌 대상은 옆 시점이 필요합니다. 앞·옆·뒤 시트를 생성하세요")?;
    if !views
        .iter()
        .any(|(_, f)| matches!(f.view, View::Front | View::Back))
    {
        return Err("앞이나 뒤 시점이 필요합니다".into());
    }
    let mut xr: f32 = 0.0;
    for (ph, f) in views
        .iter()
        .filter(|(_, f)| matches!(f.view, View::Front | View::Back))
    {
        let (a, b) = horizontal_range(ph, f).ok_or("사진에 피사체가 없습니다")?;
        xr = xr.max(a.abs()).max(b.abs());
    }
    let (z0, z1) = horizontal_range(side.0, &side.1).ok_or("사진에 피사체가 없습니다")?;
    let mut step = height / RESOLUTION;
    let dims = |step: f32| {
        [
            (2.0 * xr / step).ceil() as usize + 5,
            (height / step).ceil() as usize + 5,
            ((z1 - z0) / step).ceil() as usize + 5,
        ]
    };
    while dims(step).iter().product::<usize>() > MAX_CELLS {
        step *= 1.25;
    }
    let mut n = dims(step);
    // X 를 0 에 대해 대칭으로 둔다: 칸 i 와 n-1-i 가 거울상이다.
    if n[0] % 2 == 0 {
        n[0] += 1;
    }
    let origin = V3::new(
        -(n[0] as f32 - 1.0) / 2.0 * step,
        -2.0 * step,
        z0 - 2.0 * step,
    );
    let mut g = Grid {
        n,
        origin,
        step,
        v: vec![0.0; n[0] * n[1] * n[2]],
    };
    let inside = |p: V3| {
        views.iter().all(|(ph, f)| {
            let (x, y) = f.to_px(p);
            ph.mask.at(x, y)
        })
    };
    let mut occ = vec![false; g.v.len()];
    for k in 1..n[2] - 1 {
        for j in 1..n[1] - 1 {
            for i in 1..n[0] - 1 {
                occ[g.idx(i, j, k)] = inside(g.pos(i as f32, j as f32, k as f32));
            }
        }
    }
    for k in 0..n[2] {
        for j in 0..n[1] {
            for i in 0..n[0] {
                let a = occ[g.idx(i, j, k)] as u8 as f32;
                let b = occ[g.idx(n[0] - 1 - i, j, k)] as u8 as f32;
                let id = g.idx(i, j, k);
                g.v[id] = (a + b) / 2.0;
            }
        }
    }
    blur(&mut g);
    Ok(g)
}

/// [1,2,1]/4 필터를 세 축으로 한 번씩.
fn blur(g: &mut Grid) {
    let n = g.n;
    for axis in 0..3 {
        let src = g.v.clone();
        let stride = match axis {
            0 => 1,
            1 => n[0],
            _ => n[0] * n[1],
        };
        for k in 0..n[2] {
            for j in 0..n[1] {
                for i in 0..n[0] {
                    let c = [i, j, k][axis];
                    if c == 0 || c + 1 >= n[axis] {
                        continue;
                    }
                    let id = g.idx(i, j, k);
                    g.v[id] = (src[id - stride] + 2.0 * src[id] + src[id + stride]) / 4.0;
                }
            }
        }
    }
}

/// surface nets: 경계가 지나는 칸마다 정점 하나, 경계를 가로지르는 격자 변마다 사각형 하나.
fn surface_nets(g: &Grid) -> (Vec<V3>, Vec<[u32; 3]>) {
    let n = g.n;
    let cell = |i: usize, j: usize, k: usize| (k * (n[1] - 1) + j) * (n[0] - 1) + i;
    let mut vid = vec![u32::MAX; (n[0] - 1) * (n[1] - 1) * (n[2] - 1)];
    let mut verts = Vec::new();
    const CORNERS: [[usize; 3]; 8] = [
        [0, 0, 0],
        [1, 0, 0],
        [0, 1, 0],
        [1, 1, 0],
        [0, 0, 1],
        [1, 0, 1],
        [0, 1, 1],
        [1, 1, 1],
    ];
    const EDGES: [[usize; 2]; 12] = [
        [0, 1],
        [2, 3],
        [4, 5],
        [6, 7],
        [0, 2],
        [1, 3],
        [4, 6],
        [5, 7],
        [0, 4],
        [1, 5],
        [2, 6],
        [3, 7],
    ];
    for k in 0..n[2] - 1 {
        for j in 0..n[1] - 1 {
            for i in 0..n[0] - 1 {
                let val: Vec<f32> = CORNERS
                    .iter()
                    .map(|c| g.v[g.idx(i + c[0], j + c[1], k + c[2])])
                    .collect();
                let ins = val.iter().filter(|&&x| x > 0.5).count();
                if ins == 0 || ins == 8 {
                    continue;
                }
                let mut sum = V3::default();
                let mut cnt = 0.0;
                for e in EDGES {
                    let (a, b) = (val[e[0]], val[e[1]]);
                    if (a > 0.5) != (b > 0.5) {
                        let t = (0.5 - a) / (b - a);
                        let ca = CORNERS[e[0]].map(|x| x as f32);
                        let cb = CORNERS[e[1]].map(|x| x as f32);
                        sum = sum
                            + V3::new(
                                ca[0] + (cb[0] - ca[0]) * t,
                                ca[1] + (cb[1] - ca[1]) * t,
                                ca[2] + (cb[2] - ca[2]) * t,
                            );
                        cnt += 1.0;
                    }
                }
                let local = sum * (1.0 / cnt);
                vid[cell(i, j, k)] = verts.len() as u32;
                verts.push(g.pos(i as f32 + local.x, j as f32 + local.y, k as f32 + local.z));
            }
        }
    }
    let mut tris = Vec::new();
    let mut quad = |q: [u32; 4], flip: bool| {
        if q.contains(&u32::MAX) {
            return;
        }
        let [a, b, c, d] = if flip { [q[0], q[3], q[2], q[1]] } else { q };
        tris.push([a, b, c]);
        tris.push([a, c, d]);
    };
    for k in 1..n[2] - 1 {
        for j in 1..n[1] - 1 {
            for i in 1..n[0] - 1 {
                let here = g.inside(i, j, k);
                // x 변: (y,z) 평면의 네 칸. e_y × e_z = e_x
                if here != g.inside(i + 1, j, k) {
                    quad(
                        [
                            vid[cell(i, j - 1, k - 1)],
                            vid[cell(i, j, k - 1)],
                            vid[cell(i, j, k)],
                            vid[cell(i, j - 1, k)],
                        ],
                        !here,
                    );
                }
                // y 변: (z,x) 평면. e_z × e_x = e_y
                if here != g.inside(i, j + 1, k) {
                    quad(
                        [
                            vid[cell(i - 1, j, k - 1)],
                            vid[cell(i - 1, j, k)],
                            vid[cell(i, j, k)],
                            vid[cell(i, j, k - 1)],
                        ],
                        !here,
                    );
                }
                // z 변: (x,y) 평면. e_x × e_y = e_z
                if here != g.inside(i, j, k + 1) {
                    quad(
                        [
                            vid[cell(i - 1, j - 1, k)],
                            vid[cell(i, j - 1, k)],
                            vid[cell(i, j, k)],
                            vid[cell(i - 1, j, k)],
                        ],
                        !here,
                    );
                }
            }
        }
    }
    (verts, tris)
}

/// 삼각형 수를 예산 안으로 줄이고 쓰지 않는 정점을 뺀다.
pub(crate) fn simplify(verts: &[V3], tris: &[[u32; 3]], budget: usize) -> (Vec<V3>, Vec<[u32; 3]>) {
    let pos: Vec<[f32; 3]> = verts.iter().map(|p| [p.x, p.y, p.z]).collect();
    let idx: Vec<u32> = tris.iter().flatten().copied().collect();
    // 이미 예산 안이면 삼각형은 그대로 두고 쓰지 않는 정점만 뺀다.
    let mut out = if tris.len() <= budget {
        idx.clone()
    } else {
        meshopt::simplify_decoder(
            &idx,
            &pos,
            budget * 3,
            1.0,
            meshopt::SimplifyOptions::None,
            None,
        )
    };
    if out.len() > budget * 3 {
        out = meshopt::simplify_sloppy_decoder(&idx, &pos, budget * 3, 1.0, None);
    }
    let mut remap = vec![u32::MAX; verts.len()];
    let mut nv = Vec::new();
    let mut nt = Vec::new();
    for t in out.as_chunks::<3>().0 {
        let tri = t.map(|i| {
            if remap[i as usize] == u32::MAX {
                remap[i as usize] = nv.len() as u32;
                nv.push(verts[i as usize]);
            }
            remap[i as usize]
        });
        nt.push(tri);
    }
    (nv, nt)
}

/// 위 칸의 좌표 대응. 옆 칸의 Z 범위에 맞추고, 앞이 그림의 아래·위 중 어느 쪽인지는
/// 앞·옆·뒤로 깎은 부피와 더 많이 겹치는 쪽으로 고른다.
fn top_frame(top: &Photo, views: &[(&Photo, Frame)], height: f32) -> Option<Frame> {
    let side = views.iter().find(|(_, f)| f.view == View::Side)?;
    let (z0, z1) = horizontal_range(side.0, &side.1)?;
    let grid = carve(views, height).ok()?;
    let score = |f: &Frame| {
        let mut hit = 0usize;
        for k in 0..grid.n[2] {
            for j in 0..grid.n[1] {
                for i in 0..grid.n[0] {
                    if grid.inside(i, j, k) {
                        let (x, y) = f.to_px(grid.pos(i as f32, j as f32, k as f32));
                        hit += usize::from(top.mask.at(x, y));
                    }
                }
            }
        }
        hit
    };
    let down = Frame::top(&top.mask, z0, z1, true)?;
    let up = Frame::top(&top.mask, z0, z1, false)?;
    Some(if score(&down) >= score(&up) { down } else { up })
}

pub fn build(
    photos: &[Photo],
    mut frames: Vec<(usize, Frame)>,
    top: Option<usize>,
    st: &Structure,
    budget: usize,
    raw: Option<&crate::model::Raw>,
) -> Result<Model, String> {
    if let Some(t) = top {
        let views: Vec<(&Photo, Frame)> = frames.iter().map(|&(i, f)| (&photos[i], f)).collect();
        if let Some(f) = top_frame(&photos[t], &views, st.height_m) {
            frames.push((t, f));
        }
    }
    let views: Vec<(&Photo, Frame)> = frames.iter().map(|&(i, f)| (&photos[i], f)).collect();
    // 로컬 모델이 만든 메시가 있으면 그것을 칸에 맞춘다(decisions/0006).
    if let Some(raw) = raw {
        let mesh = crate::model::fit(raw, &views, st.height_m, budget)?;
        return Ok(Model {
            mesh,
            frames,
            rings: Vec::new(),
        });
    }
    if !st.blocks.is_empty() {
        let side = views
            .iter()
            .find(|(_, f)| f.view == View::Side)
            .ok_or("사람이 아닌 대상은 옆 시점이 필요합니다")?;
        let front = views
            .iter()
            .find(|(_, f)| f.view == View::Front)
            .or_else(|| views.iter().find(|(_, f)| f.view == View::Back));
        let top = views.iter().find(|(_, f)| f.view == View::Top);
        let mesh = crate::kit::assemble(
            (&side.0.mask, &side.1),
            front.map(|(p, f)| (&p.mask, f)),
            top.map(|(p, f)| (&p.mask, f)),
            &st.blocks,
            st.height_m,
            budget,
        )?;
        return Ok(Model {
            mesh,
            frames,
            rings: Vec::new(),
        });
    }
    let grid = carve(&views, st.height_m)?;
    let (verts, tris) = surface_nets(&grid);
    if tris.is_empty() {
        return Err("실루엣이 겹치는 부피가 없습니다. 앞·옆·뒤 배치를 확인하세요".into());
    }
    let (positions, tris) = simplify(&verts, &tris, budget);
    let parts = vec![0; tris.len()];
    Ok(Model {
        mesh: Mesh {
            positions,
            tris,
            parts,
            normals: Vec::new(),
        },
        frames,
        rings: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mask::Mask;

    fn rect(w: usize, h: usize, x0: usize, x1: usize, y0: usize, y1: usize) -> Mask {
        let mut m = Mask::new(w, h);
        for y in y0..y1 {
            for x in x0..x1 {
                m.set(x, y, true);
            }
        }
        m
    }

    #[test]
    fn box_silhouettes_carve_a_closed_outward_box() {
        // 앞: 폭 40px, 옆: 깊이 80px, 키 100px. 1px = 0.01m
        let photo = |m: Mask| Photo {
            name: String::new(),
            rgb: image::RgbImage::new(m.w as u32, m.h as u32),
            mask: m,
        };
        let front = photo(rect(100, 120, 30, 70, 10, 110));
        let side = photo(rect(120, 120, 20, 100, 10, 110));
        let ff = Frame::new(
            View::Front,
            &front.mask,
            1.0,
            crate::structure::Facing::Right,
        )
        .unwrap();
        let sf = Frame::new(View::Side, &side.mask, 1.0, crate::structure::Facing::Right).unwrap();
        let views = vec![(&front, ff), (&side, sf)];
        let g = carve(&views, 1.0).unwrap();
        let (v, t) = surface_nets(&g);
        let (v, t) = simplify(&v, &t, 500);
        assert!(!t.is_empty() && t.len() <= 500);
        let vol: f32 = t
            .iter()
            .map(|tri| {
                let [a, b, c] = tri.map(|i| v[i as usize]);
                a.dot(b.cross(c)) / 6.0
            })
            .sum();
        // 0.4 × 1.0 × 0.8 = 0.32 m³
        assert!((vol - 0.32).abs() < 0.03, "volume {vol}");
        let ymin = v.iter().map(|p| p.y).fold(f32::MAX, f32::min);
        assert!(ymin.abs() < 0.02, "ymin {ymin}");
    }
}
