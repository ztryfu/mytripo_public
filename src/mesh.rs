//! 단면 고리를 쌓아 닫힌 튜브를 만든다. decisions/0001.

use crate::geom::V3;

/// 삼각형 메시. 색은 없다(회색 재질 하나).
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub positions: Vec<V3>,
    pub tris: Vec<[u32; 3]>,
    /// 삼각형마다 부위 번호.
    pub parts: Vec<usize>,
    /// 정점마다 법선. 비어 있으면 면마다 평평하게 그린다.
    pub normals: Vec<V3>,
}

impl Mesh {
    pub fn triangle_count(&self) -> usize {
        self.tris.len()
    }

    pub fn bounds(&self) -> (V3, V3) {
        let mut lo = V3::new(f32::MAX, f32::MAX, f32::MAX);
        let mut hi = V3::new(f32::MIN, f32::MIN, f32::MIN);
        for p in &self.positions {
            lo = V3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
            hi = V3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
        }
        (lo, hi)
    }

    pub fn corners(&self, t: usize) -> [V3; 3] {
        self.tris[t].map(|i| self.positions[i as usize])
    }

    /// 정점에 모인 면의 법선을 넓이로 가중해 더한 정점 법선.
    pub fn smooth_normals(&self) -> Vec<V3> {
        let mut n = vec![V3::default(); self.positions.len()];
        for t in 0..self.tris.len() {
            let [a, b, c] = self.corners(t);
            let w = (b - a).cross(c - a);
            for &i in &self.tris[t] {
                n[i as usize] = n[i as usize] + w;
            }
        }
        n.into_iter().map(V3::norm).collect()
    }

    /// 바깥을 향하는 면 법선.
    pub fn normal(&self, t: usize) -> V3 {
        let [a, b, c] = self.corners(t);
        (b - a).cross(c - a).norm()
    }
}

/// 단면 고리. 정점 k 는 center + u·cosθ·(u 쪽 반경) + v·sinθ·(v 쪽 반경) 이다.
/// θ = 2πk/n 이므로 n 이 4의 배수이면 정점이 네 극점(±u, ±v)에 놓인다.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ring {
    pub center: V3,
    /// 폭 방향 단위 벡터(정면 사진의 평면 안).
    pub u: V3,
    /// 깊이 방향 단위 벡터.
    pub v: V3,
    /// (−u 쪽, +u 쪽) 반경.
    pub ru: (f32, f32),
    /// (−v 쪽, +v 쪽) 반경.
    pub rv: (f32, f32),
}

impl Ring {
    pub fn points(&self, n: usize) -> Vec<V3> {
        (0..n)
            .map(|k| {
                let t = std::f32::consts::TAU * k as f32 / n as f32;
                let (s, c) = t.sin_cos();
                let a = if c >= 0.0 { self.ru.1 } else { self.ru.0 };
                let b = if s >= 0.0 { self.rv.1 } else { self.rv.0 };
                self.center + self.u * (c * a) + self.v * (s * b)
            })
            .collect()
    }

    /// X=0 평면에 대해 비춘 고리.
    pub fn mirror_x(&self) -> Ring {
        Ring {
            center: self.center.mirror_x(),
            u: self.u.mirror_x(),
            v: self.v.mirror_x(),
            ..*self
        }
    }
}

/// 튜브 하나의 삼각형 수. 옆면 2n(m−1) + 양 끝 뚜껑 2n = 2nm.
pub fn tube_triangles(rings: usize, n: usize) -> usize {
    2 * n * rings
}

/// 고리들을 이어 닫힌 튜브를 붙인다.
pub fn add_tube(mesh: &mut Mesh, rings: &[Ring], n: usize, part: usize) {
    if rings.len() < 2 {
        return;
    }
    let base = mesh.positions.len() as u32;
    for r in rings {
        mesh.positions.extend(r.points(n));
    }
    let first_cap = mesh.positions.len() as u32;
    mesh.positions.push(rings[0].center);
    let last_cap = first_cap + 1;
    mesh.positions.push(rings[rings.len() - 1].center);

    let idx = |ring: usize, k: usize| base + (ring * n + k % n) as u32;
    let pos = |mesh: &Mesh, i: u32| mesh.positions[i as usize];
    // 면(삼각형 여러 개)을 바깥 방향에 맞춰 붙인다.
    let face = |mesh: &mut Mesh, tris: &[[u32; 3]], outward: V3| {
        for &t in tris {
            let mut t = t;
            let [a, b, c] = t.map(|i| pos(mesh, i));
            let nrm = (b - a).cross(c - a);
            if nrm.len() < 1e-12 {
                continue;
            }
            if nrm.dot(outward) < 0.0 {
                t.swap(1, 2);
            }
            mesh.tris.push(t);
            mesh.parts.push(part);
        }
    };
    for i in 0..rings.len() - 1 {
        let axis_mid = rings[i].center.lerp(rings[i + 1].center, 0.5);
        for k in 0..n {
            let (a, b, c, d) = (idx(i, k), idx(i, k + 1), idx(i + 1, k + 1), idx(i + 1, k));
            let quad_mid = (pos(mesh, a) + pos(mesh, b) + pos(mesh, c) + pos(mesh, d)) * 0.25;
            face(mesh, &[[a, b, c], [a, c, d]], quad_mid - axis_mid);
        }
    }
    let m = rings.len();
    let out0 = rings[0].center - rings[1].center;
    let out1 = rings[m - 1].center - rings[m - 2].center;
    let cap0: Vec<[u32; 3]> = (0..n)
        .map(|k| [first_cap, idx(0, k), idx(0, k + 1)])
        .collect();
    let cap1: Vec<[u32; 3]> = (0..n)
        .map(|k| [last_cap, idx(m - 1, k), idx(m - 1, k + 1)])
        .collect();
    face(mesh, &cap0, out0);
    face(mesh, &cap1, out1);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(y: f32) -> Ring {
        Ring {
            center: V3::new(0.0, y, 0.0),
            u: V3::new(1.0, 0.0, 0.0),
            v: V3::new(0.0, 0.0, 1.0),
            ru: (0.1, 0.2),
            rv: (0.05, 0.05),
        }
    }

    #[test]
    fn ring_vertices_hit_extremes() {
        let p = ring(0.0).points(8);
        assert!((p[0].x - 0.2).abs() < 1e-6);
        assert!((p[4].x + 0.1).abs() < 1e-6);
        assert!((p[2].z - 0.05).abs() < 1e-6);
    }

    #[test]
    fn tube_is_closed_and_outward() {
        let mut m = Mesh::default();
        let rings = [ring(0.0), ring(0.5), ring(1.0)];
        add_tube(&mut m, &rings, 8, 0);
        assert_eq!(m.triangle_count(), tube_triangles(3, 8));
        // 닫힌 메시: 모든 변을 두 삼각형이 반대 방향으로 공유한다.
        let mut edges = std::collections::HashMap::new();
        for t in &m.tris {
            for e in 0..3 {
                *edges.entry((t[e], t[(e + 1) % 3])).or_insert(0) += 1;
            }
        }
        for (&(a, b), &c) in &edges {
            assert_eq!(c, 1);
            assert_eq!(edges.get(&(b, a)), Some(&1));
        }
        // 부호 있는 부피가 양수면 법선이 바깥을 향한다.
        let vol: f32 = (0..m.triangle_count())
            .map(|t| {
                let [a, b, c] = m.corners(t);
                a.dot(b.cross(c)) / 6.0
            })
            .sum();
        assert!(vol > 0.0);
    }
}
