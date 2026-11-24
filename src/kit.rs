//! 사람이 아닌 대상: 부품마다 윤곽 다각형을 밀어낸 덩어리(프리즘)로 로우폴리를 조립한다. decisions/0005.
//!
//! 덩어리 부품은 옆 사진에서 자기 몫의 윤곽을 따서 좌우(X)로 밀어낸다.
//! 가로 막대(핸들, 미러)는 앞 사진의 윤곽을 앞뒤(Z)로 밀어낸다.
//! 밀어낸 두 끝면은 평면이다. 그 평면은 앞·위 사진에서 잰 폭에 맞춘다.
//! 바퀴는 모서리를 깎은 타이어와 안으로 들어간 허브가 있는 회전체다.

use std::collections::BTreeMap;

use crate::geom::V3;
use crate::mask::Mask;
use crate::mesh::Mesh;
use crate::pipeline::Frame;
use crate::structure::{Block, Shape, View};

/// 사진 하나와 좌표 대응.
pub type View2<'a> = (&'a Mask, &'a Frame);

/// 상자를 넓히는 여유(사진 크기 대비).
const MARGIN: f32 = 0.01;
/// 앞 사진 가로 범위의 여유. 가려진 부품의 범위는 추정값이므로 넉넉히 두고 마스크로 자른다.
const X_MARGIN: f32 = 0.01;
/// 바퀴 둘레의 변 수.
const WHEEL_SIDES: usize = 16;
/// 타이어 모서리를 깎는 폭(바퀴 반폭 대비).
const WHEEL_CHAMFER: f32 = 0.35;
/// 타이어 옆면이 시작하는 반지름(바퀴 반지름 대비).
const TIRE_INNER: f32 = 0.62;
/// 타이어 옆면이 가장 불룩한 반지름(바퀴 반지름 대비).
const SIDEWALL: f32 = 0.82;
/// 림 테두리가 옆면보다 안으로 들어간 깊이(바퀴 반폭 대비).
const RIM: f32 = 0.15;
/// 허브 끝면의 반지름(바퀴 반지름 대비).
const HUB: f32 = 0.5;
/// 허브가 안으로 들어간 깊이(바퀴 반폭 대비).
const HUB_DEPTH: f32 = 0.45;
/// 위 칸에서 폭을 잴 때 앞 가로 범위를 넓히는 양(키 대비).
const WIDEN: f32 = 0.06;
/// 어느 상자에도 들지 않은 픽셀을 가장 가까운 상자에 붙이는 거리(사진 높이 대비).
const ADOPT: f32 = 0.04;
/// 이보다 작은 조각은 버린다(사진 넓이 대비).
const MIN_PIECE: f32 = 0.0004;
/// 윤곽을 줄이는 허용 오차의 시작값(사진 높이 대비). 예산을 넘으면 키운다.
const SIMPLIFY: f32 = 0.006;
/// 한 조각의 두 끝면 평면이 잰 폭에서 벗어나도 되는 평균 제곱근 오차(키 대비).
const FIT_TOL: f32 = 0.01;
/// 조각을 가르는 최대 횟수(깊이). 한 부품은 많아야 2^SPLITS 조각이다.
const SPLITS: usize = 2;
/// 밀어낸 두께의 하한(잰 두께의 중앙값 대비).
const MIN_DEPTH: f32 = 0.3;

/// 세계 좌표의 구간.
type Span = (f32, f32);

fn sort(a: f32, b: f32) -> Span {
    (a.min(b), a.max(b))
}

/// 사진의 가로 좌표 h 에 놓인 픽셀 열에서, 세계 세로 좌표가 lim 안인 마스크 픽셀의 세로 범위.
fn col_span(v: View2, h: f32, lim: Span, skip: &dyn Fn(f32, f32) -> bool) -> Option<Span> {
    let (m, f) = v;
    let x = f.to_px(world(f.view, h, 0.0)).0;
    let (ya, yb) = (
        f.to_px(world(f.view, h, lim.0)).1,
        f.to_px(world(f.view, h, lim.1)).1,
    );
    let (r0, r1) = (
        ya.min(yb).max(0.0) as i64,
        ya.max(yb).min(m.h as f32 - 1.0) as i64,
    );
    let (mut lo, mut hi) = (i64::MAX, i64::MIN);
    for r in r0..=r1 {
        if m.get(x as i64, r) && !skip(x, r as f32 + 0.5) {
            lo = lo.min(r);
            hi = hi.max(r + 1);
        }
    }
    (lo < hi).then(|| sort(f.from_px(x, lo as f32).1, f.from_px(x, hi as f32).1))
}

/// col_span 과 같되, 마스크가 끊긴 토막마다 따로 낸다.
fn col_runs(v: View2, h: f32, lim: Span) -> Vec<Span> {
    let (m, f) = v;
    let x = f.to_px(world(f.view, h, 0.0)).0;
    let (ya, yb) = (
        f.to_px(world(f.view, h, lim.0)).1,
        f.to_px(world(f.view, h, lim.1)).1,
    );
    let (r0, r1) = (
        ya.min(yb).max(0.0) as i64,
        ya.max(yb).min(m.h as f32 - 1.0) as i64,
    );
    let mut out = Vec::new();
    let mut r = r0;
    while r <= r1 {
        if !m.get(x as i64, r) {
            r += 1;
            continue;
        }
        let s = r;
        while r <= r1 && m.get(x as i64, r) {
            r += 1;
        }
        out.push(sort(f.from_px(x, s as f32).1, f.from_px(x, r as f32).1));
    }
    out
}

/// 사진의 세로 좌표 v 에 놓인 픽셀 줄에서, 세계 가로 좌표가 lim 안인 마스크 픽셀의 가로 범위.
fn row_span(v: View2, vert: f32, lim: Span, skip: &dyn Fn(f32, f32) -> bool) -> Option<Span> {
    let (m, f) = v;
    let y = f.to_px(world(f.view, 0.0, vert)).1;
    let (xa, xb) = (
        f.to_px(world(f.view, lim.0, vert)).0,
        f.to_px(world(f.view, lim.1, vert)).0,
    );
    let (c0, c1) = (
        xa.min(xb).max(0.0) as i64,
        xa.max(xb).min(m.w as f32 - 1.0) as i64,
    );
    let (mut lo, mut hi) = (i64::MAX, i64::MIN);
    for c in c0..=c1 {
        if m.get(c, y as i64) && !skip(c as f32 + 0.5, y) {
            lo = lo.min(c);
            hi = hi.max(c + 1);
        }
    }
    (lo < hi).then(|| sort(f.from_px(lo as f32, y).0, f.from_px(hi as f32, y).0))
}

/// 사진의 (가로, 세로) 세계 좌표를 3D 점으로. 나머지 축은 0 이다.
fn world(view: View, h: f32, v: f32) -> V3 {
    match view {
        View::Side => V3::new(0.0, v, h),
        View::Top => V3::new(h, 0.0, v),
        _ => V3::new(h, v, 0.0),
    }
}

/// 여러 줄(또는 열)의 범위를 합친다.
fn union(spans: impl Iterator<Item = Option<Span>>) -> Option<Span> {
    spans.flatten().reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)))
}

/// a 와 b 의 겹침. b 가 없거나 겹치지 않으면 a.
fn meet(a: Span, b: Option<Span>) -> Span {
    match b {
        Some(b) if a.0.max(b.0) < a.1.min(b.1) => (a.0.max(b.0), a.1.min(b.1)),
        _ => a,
    }
}

/// a 와 b 의 겹침. 겹치지 않으면 없다.
fn both(a: Span, b: Span) -> Option<Span> {
    let s = (a.0.max(b.0), a.1.min(b.1));
    (s.0 < s.1).then_some(s)
}

/// 구간 s 를 9 곳에서 재어 합친다.
fn spread(s: Span, f: impl Fn(f32) -> Option<Span>) -> Option<Span> {
    union((0..=8).map(|k| f(s.0 + (s.1 - s.0) * k as f32 / 8.0)))
}

/// 옆 사진의 바퀴: 상자 가운데 70% 열마다 가장 낮은 마스크 픽셀(타이어 아래 호)에 원을 맞춘다.
/// 맞춘 원에서 먼 점을 빼고 한 번 더 맞춘다. (Z 중심, Y 중심, 반지름)
fn fit_wheel(side: View2, bd: &Bounds) -> Option<(f32, f32, f32)> {
    let none = |_: f32, _: f32| false;
    let mut pts: Vec<(f32, f32)> = Vec::new();
    let (z0, z1) = bd.z;
    for k in 0..=60 {
        let z = z0 + (z1 - z0) * (0.15 + 0.7 * k as f32 / 60.0);
        if let Some(y) = col_span(side, z, bd.y, &none) {
            pts.push((z, y.0));
        }
    }
    let mut c = fit_circle(&pts)?;
    let keep: Vec<(f32, f32)> = pts
        .iter()
        .copied()
        .filter(|&(z, y)| (((z - c.0).powi(2) + (y - c.1).powi(2)).sqrt() - c.2).abs() < 0.03 * c.2)
        .collect();
    if keep.len() >= 8 {
        c = fit_circle(&keep)?;
    }
    // 상자보다 큰 원은 믿지 않는다.
    let box_r = (z1 - z0).max(bd.y.1 - bd.y.0) / 2.0;
    (c.2 > 0.0 && c.2 < 1.2 * box_r).then_some(c)
}

/// 최소제곱 원 맞춤(Kasa). (중심 a, 중심 b, 반지름)
fn fit_circle(p: &[(f32, f32)]) -> Option<(f32, f32, f32)> {
    if p.len() < 3 {
        return None;
    }
    let n = p.len() as f64;
    let (mx, my) = p
        .iter()
        .fold((0.0, 0.0), |s, &(x, y)| (s.0 + x as f64, s.1 + y as f64));
    let (mx, my) = (mx / n, my / n);
    let (mut suu, mut svv, mut suv, mut suuu, mut svvv, mut suvv, mut svuu) =
        (0.0f64, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for &(x, y) in p {
        let (u, v) = (x as f64 - mx, y as f64 - my);
        suu += u * u;
        svv += v * v;
        suv += u * v;
        suuu += u * u * u;
        svvv += v * v * v;
        suvv += u * v * v;
        svuu += v * u * u;
    }
    let det = suu * svv - suv * suv;
    if det.abs() < 1e-12 {
        return None;
    }
    let (r1, r2) = ((suuu + suvv) / 2.0, (svvv + svuu) / 2.0);
    let uc = (r1 * svv - r2 * suv) / det;
    let vc = (r2 * suu - r1 * suv) / det;
    let r = (uc * uc + vc * vc + (suu + svv) / n).sqrt();
    Some(((uc + mx) as f32, (vc + my) as f32, r as f32))
}

/// X 구간을 키의 WIDEN 배만큼 양쪽으로 넓힌다.
fn widen(s: Span, height: f32) -> Span {
    (s.0 - WIDEN * height, s.1 + WIDEN * height)
}

/// 사진 한 장 위의 픽셀 영역(경계 상자 안의 비트맵).
struct Region {
    x0: usize,
    y0: usize,
    w: usize,
    h: usize,
    px: Vec<bool>,
}

impl Region {
    fn get(&self, x: i64, y: i64) -> bool {
        x >= 0
            && y >= 0
            && (x as usize) < self.w
            && (y as usize) < self.h
            && self.px[y as usize * self.w + x as usize]
    }
}

/// 사진 크기의 라벨 지도에서 라벨 l 의 4-연결 조각들. 작은 조각은 버린다.
fn pieces(label: &[usize], w: usize, h: usize, l: usize, min_px: usize) -> Vec<Region> {
    let mut seen = vec![false; label.len()];
    let mut out = Vec::new();
    for start in 0..label.len() {
        if label[start] != l || seen[start] {
            continue;
        }
        let mut stack = vec![start];
        seen[start] = true;
        let mut cells = Vec::new();
        while let Some(i) = stack.pop() {
            cells.push(i);
            let (x, y) = (i % w, i / w);
            let mut push = |j: usize| {
                if label[j] == l && !seen[j] {
                    seen[j] = true;
                    stack.push(j);
                }
            };
            if x > 0 {
                push(i - 1);
            }
            if x + 1 < w {
                push(i + 1);
            }
            if y > 0 {
                push(i - w);
            }
            if y + 1 < h {
                push(i + w);
            }
        }
        if cells.len() < min_px {
            continue;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        for &i in &cells {
            x0 = x0.min(i % w);
            x1 = x1.max(i % w);
            y0 = y0.min(i / w);
            y1 = y1.max(i / w);
        }
        let (rw, rh) = (x1 - x0 + 1, y1 - y0 + 1);
        let mut px = vec![false; rw * rh];
        for &i in &cells {
            px[(i / w - y0) * rw + (i % w - x0)] = true;
        }
        out.push(Region {
            x0,
            y0,
            w: rw,
            h: rh,
            px,
        });
    }
    out
}

/// 영역의 바깥 윤곽. 픽셀 모서리를 잇는 닫힌 꺾은선(사진 픽셀 좌표)이다. 구멍은 메운다.
fn outline(r: &Region) -> Vec<(f32, f32)> {
    // 픽셀마다 바깥과 닿은 변을 시계 방향(그림 좌표, y 아래)으로 모은다. 영역은 진행 방향 오른쪽에 있다.
    let mut next: BTreeMap<(i64, i64), Vec<(i64, i64)>> = BTreeMap::new();
    for y in 0..r.h as i64 {
        for x in 0..r.w as i64 {
            if !r.get(x, y) {
                continue;
            }
            let mut add = |a: (i64, i64), b: (i64, i64)| next.entry(a).or_default().push(b);
            if !r.get(x, y - 1) {
                add((x, y), (x + 1, y));
            }
            if !r.get(x + 1, y) {
                add((x + 1, y), (x + 1, y + 1));
            }
            if !r.get(x, y + 1) {
                add((x + 1, y + 1), (x, y + 1));
            }
            if !r.get(x - 1, y) {
                add((x, y + 1), (x, y));
            }
        }
    }
    let mut best: Vec<(i64, i64)> = Vec::new();
    let mut best_area = 0i64;
    while let Some(&start) = next.keys().next() {
        let mut lp = vec![start];
        let mut cur = start;
        let mut dir = (0i64, 0i64);
        while let Some(outs) = next.get_mut(&cur) {
            // 갈림길(대각선으로만 닿은 픽셀)에서는 오른쪽으로 꺾는 변을 고른다.
            let k = if outs.len() > 1 {
                let right = (-dir.1, dir.0);
                outs.iter()
                    .position(|&b| (b.0 - cur.0, b.1 - cur.1) == right)
                    .unwrap_or(0)
            } else {
                0
            };
            let b = outs.swap_remove(k);
            if outs.is_empty() {
                next.remove(&cur);
            }
            dir = (b.0 - cur.0, b.1 - cur.1);
            cur = b;
            if cur == start {
                break;
            }
            lp.push(cur);
        }
        let area: i64 = (0..lp.len())
            .map(|i| {
                let (a, b) = (lp[i], lp[(i + 1) % lp.len()]);
                a.0 * b.1 - b.0 * a.1
            })
            .sum();
        if area.abs() > best_area {
            best_area = area.abs();
            best = lp;
        }
    }
    best.iter()
        .map(|&(x, y)| ((x as usize + r.x0) as f32, (y as usize + r.y0) as f32))
        .collect()
}

/// 닫힌 꺾은선을 Douglas–Peucker 로 줄인다. 남긴 점의 번호를 낸다.
fn simplify(p: &[(f32, f32)], tol: f32) -> Vec<usize> {
    let n = p.len();
    if n < 4 {
        return (0..n).collect();
    }
    let d2 = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0).powi(2) + (a.1 - b.1).powi(2);
    let far = (1..n)
        .max_by(|&i, &j| d2(p[0], p[i]).total_cmp(&d2(p[0], p[j])))
        .unwrap_or(n / 2);
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[far] = true;
    // 꺾은선 i..=j(감싸는 번호)에서 가장 먼 점을 남기고 나눈다.
    let mut stack = vec![(0usize, far), (far, n)];
    while let Some((i, j)) = stack.pop() {
        if j <= i + 1 {
            continue;
        }
        let (a, b) = (p[i], p[j % n]);
        let len = d2(a, b).sqrt().max(1e-6);
        let (mut best, mut at) = (0.0f32, i);
        for (k, &q) in p.iter().enumerate().take(j).skip(i + 1) {
            let d = ((b.0 - a.0) * (a.1 - q.1) - (a.0 - q.0) * (b.1 - a.1)).abs() / len;
            if d > best {
                best = d;
                at = k;
            }
        }
        if best > tol {
            keep[at] = true;
            stack.push((i, at));
            stack.push((at, j));
        }
    }
    let mut out: Vec<usize> = (0..n).filter(|&i| keep[i]).collect();
    // 시작점이 곧은 변 위에 있으면 남는다. 이웃을 잇는 선에서 tol 안의 점을 뺀다.
    let mut i = 0;
    while out.len() > 3 && i < out.len() {
        let m = out.len();
        let (a, q, b) = (p[out[(i + m - 1) % m]], p[out[i]], p[out[(i + 1) % m]]);
        let len = d2(a, b).sqrt().max(1e-6);
        if cross(a, b, q).abs() / len <= tol {
            out.remove(i);
        } else {
            i += 1;
        }
    }
    out
}

fn cross(o: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
}

fn signed_area(p: &[(f32, f32)]) -> f32 {
    (0..p.len())
        .map(|i| {
            let (a, b) = (p[i], p[(i + 1) % p.len()]);
            a.0 * b.1 - b.0 * a.1
        })
        .sum::<f32>()
        / 2.0
}

/// 반시계 방향 단순 다각형을 귀 자르기로 삼각형으로 나눈다.
fn triangulate(p: &[(f32, f32)]) -> Vec<[usize; 3]> {
    let mut idx: Vec<usize> = (0..p.len()).collect();
    let mut out = Vec::new();
    let eps = 1e-9 * signed_area(p).abs().max(1e-12);
    while idx.len() > 3 {
        let m = idx.len();
        let mut cut = None;
        for i in 0..m {
            let (a, b, c) = (idx[(i + m - 1) % m], idx[i], idx[(i + 1) % m]);
            let z = cross(p[a], p[b], p[c]);
            if z.abs() <= eps {
                // 한 줄에 놓인 점은 삼각형 없이 뺀다.
                cut = Some((i, None));
                break;
            }
            if z < 0.0 {
                continue;
            }
            let inside = idx.iter().any(|&j| {
                j != a
                    && j != b
                    && j != c
                    && cross(p[a], p[b], p[j]) >= 0.0
                    && cross(p[b], p[c], p[j]) >= 0.0
                    && cross(p[c], p[a], p[j]) >= 0.0
            });
            if !inside {
                cut = Some((i, Some([a, b, c])));
                break;
            }
        }
        match cut {
            Some((i, t)) => {
                out.extend(t);
                idx.remove(i);
            }
            None => {
                // 귀가 없으면(꼬인 윤곽) 남은 점을 부채꼴로 막는다.
                for i in 1..idx.len() - 1 {
                    out.push([idx[0], idx[i], idx[i + 1]]);
                }
                return out;
            }
        }
    }
    if idx.len() == 3 {
        out.push([idx[0], idx[1], idx[2]]);
    }
    out
}

/// 평면 d = a + b·u + c·v.
type Plane = [f32; 3];

fn eval(p: Plane, u: f32, v: f32) -> f32 {
    p[0] + p[1] * u + p[2] * v
}

/// (u, v, d) 표본에 평면을 최소제곱으로 맞춘다. 많이 벗어난 표본을 빼고 한 번 더 맞춘다.
fn fit_plane(s: &[(f32, f32, f32)]) -> Option<Plane> {
    let solve = |s: &[(f32, f32, f32)]| -> Option<Plane> {
        if s.is_empty() {
            return None;
        }
        let n = s.len() as f64;
        let (mu, mv, md) = s.iter().fold((0.0, 0.0, 0.0), |a, &(u, v, d)| {
            (a.0 + u as f64 / n, a.1 + v as f64 / n, a.2 + d as f64 / n)
        });
        let (mut uu, mut vv, mut uv, mut ud, mut vd) = (0.0f64, 0.0, 0.0, 0.0, 0.0);
        for &(u, v, d) in s {
            let (u, v, d) = (u as f64 - mu, v as f64 - mv, d as f64 - md);
            uu += u * u;
            vv += v * v;
            uv += u * v;
            ud += u * d;
            vd += v * d;
        }
        let det = uu * vv - uv * uv;
        let (b, c) = if s.len() >= 6 && det.abs() > 1e-12 * (uu * vv).max(1e-30) {
            ((ud * vv - vd * uv) / det, (vd * uu - ud * uv) / det)
        } else {
            (0.0, 0.0)
        };
        Some([(md - b * mu - c * mv) as f32, b as f32, c as f32])
    };
    let p = solve(s)?;
    let mut res: Vec<f32> = s
        .iter()
        .map(|&(u, v, d)| (d - eval(p, u, v)).abs())
        .collect();
    res.sort_by(f32::total_cmp);
    let lim = 2.5 * res[res.len() / 2] + 1e-6;
    let kept: Vec<_> = s
        .iter()
        .copied()
        .filter(|&(u, v, d)| (d - eval(p, u, v)).abs() <= lim)
        .collect();
    solve(&kept).or(Some(p))
}

/// 윤곽 다각형(사진의 세계 좌표 u, v)을 두 평면 lo..hi 사이로 밀어낸 닫힌 덩어리.
/// to3 은 (u, v, 깊이)를 3D 점으로 옮긴다.
fn prism(
    mesh: &mut Mesh,
    poly: &[(f32, f32)],
    lo: &[f32],
    hi: &[f32],
    to3: impl Fn(f32, f32, f32) -> V3,
) {
    let n = poly.len();
    let base = mesh.positions.len() as u32;
    for i in 0..n {
        mesh.positions.push(to3(poly[i].0, poly[i].1, lo[i]));
    }
    for i in 0..n {
        mesh.positions.push(to3(poly[i].0, poly[i].1, hi[i]));
    }
    let start = mesh.tris.len();
    let (l, h) = (
        |i: usize| base + (i % n) as u32,
        |i: usize| base + (n + i % n) as u32,
    );
    for t in triangulate(poly) {
        mesh.tris.push([h(t[0]), h(t[1]), h(t[2])]);
        mesh.tris.push([l(t[0]), l(t[2]), l(t[1])]);
    }
    for i in 0..n {
        mesh.tris.push([l(i), l(i + 1), h(i + 1)]);
        mesh.tris.push([l(i), h(i + 1), h(i)]);
    }
    orient(mesh, start);
}

/// start 이후 삼각형의 부피가 음수면 모두 뒤집는다. 법선이 바깥을 향한다.
fn orient(mesh: &mut Mesh, start: usize) {
    let vol: f32 = mesh.tris[start..]
        .iter()
        .map(|t| {
            let [p, q, r] = t.map(|i| mesh.positions[i as usize]);
            p.dot(q.cross(r))
        })
        .sum();
    if vol < 0.0 {
        for t in &mut mesh.tris[start..] {
            t.swap(1, 2);
        }
    }
    mesh.parts.resize(mesh.tris.len(), 0);
}

/// 고리들을 이어 닫힌 조각을 만든다. 양 끝은 부채꼴로 막는다. 법선은 바깥을 향한다.
fn loft(mesh: &mut Mesh, rings: &[Vec<V3>]) {
    if rings.len() < 2 {
        return;
    }
    let n = rings[0].len();
    let base = mesh.positions.len() as u32;
    for r in rings {
        mesh.positions.extend_from_slice(r);
    }
    let start = mesh.tris.len();
    let v = |l: usize, i: usize| base + (l * n + i % n) as u32;
    for i in 1..n - 1 {
        mesh.tris.push([v(0, 0), v(0, i + 1), v(0, i)]);
        let l = rings.len() - 1;
        mesh.tris.push([v(l, 0), v(l, i), v(l, i + 1)]);
    }
    for l in 0..rings.len() - 1 {
        for i in 0..n {
            mesh.tris.push([v(l, i), v(l, i + 1), v(l + 1, i + 1)]);
            mesh.tris.push([v(l, i), v(l + 1, i + 1), v(l + 1, i)]);
        }
    }
    let vol: f32 = mesh.tris[start..]
        .iter()
        .map(|t| {
            let [p, q, r] = t.map(|i| mesh.positions[i as usize]);
            p.dot(q.cross(r))
        })
        .sum();
    if vol < 0.0 {
        for t in &mut mesh.tris[start..] {
            t.swap(1, 2);
        }
    }
    mesh.parts.resize(mesh.tris.len(), 0);
}

/// 부품 하나의 세계 범위.
struct Bounds {
    x: Span,
    y: Span,
    z: Span,
}

/// 밀어낼 조각 하나: 윤곽(픽셀), 그 사진, 두 끝면 평면, 깊이의 허용 범위.
struct Piece<'a> {
    outline: Vec<(f32, f32)>,
    /// 윤곽 점마다 잰 깊이 범위. 평면이 이 범위를 넘으면 자른다.
    meas: Vec<Option<Span>>,
    view: View2<'a>,
    lo: Plane,
    hi: Plane,
    lo_lim: Span,
    hi_lim: Span,
    min_depth: f32,
}

/// 영역에서 고른 깊이 표본 (u, v, lo, hi)과 그 픽셀 위치.
fn samples(
    r: &Region,
    f: &Frame,
    depth: &dyn Fn(f32, f32) -> Option<Span>,
) -> Vec<(usize, usize, f32, f32, f32, f32)> {
    let stride = (r.w.max(r.h) / 32).max(1);
    let mut out = Vec::new();
    for y in (0..r.h).step_by(stride) {
        for x in (0..r.w).step_by(stride) {
            if !r.px[y * r.w + x] {
                continue;
            }
            let (u, v) = f.from_px((x + r.x0) as f32 + 0.5, (y + r.y0) as f32 + 0.5);
            if let Some(d) = depth(u, v) {
                out.push((x, y, u, v, d.0, d.1));
            }
        }
    }
    out
}

/// 표본에 두 평면을 맞추고 평균 제곱근 오차 중 큰 값을 낸다.
fn fit(s: &[(usize, usize, f32, f32, f32, f32)]) -> Option<(Plane, Plane, f32)> {
    let lo: Vec<_> = s.iter().map(|p| (p.2, p.3, p.4)).collect();
    let hi: Vec<_> = s.iter().map(|p| (p.2, p.3, p.5)).collect();
    let (a, b) = (fit_plane(&lo)?, fit_plane(&hi)?);
    let rms = |q: &[(f32, f32, f32)], p: Plane| {
        (q.iter()
            .map(|&(u, v, d)| (d - eval(p, u, v)).powi(2))
            .sum::<f32>()
            / q.len() as f32)
            .sqrt()
    };
    Some((a, b, rms(&lo, a).max(rms(&hi, b))))
}

/// 영역을 픽셀 세로선(vertical) 또는 가로선에서 둘로 가른다.
fn cut(r: &Region, vertical: bool, at: usize, min_px: usize) -> Vec<Region> {
    let label: Vec<usize> = (0..r.w * r.h)
        .map(|i| {
            if !r.px[i] {
                usize::MAX
            } else if (if vertical { i % r.w } else { i / r.w }) < at {
                0
            } else {
                1
            }
        })
        .collect();
    let mut out = pieces(&label, r.w, r.h, 0, min_px);
    out.extend(pieces(&label, r.w, r.h, 1, min_px));
    for p in &mut out {
        p.x0 += r.x0;
        p.y0 += r.y0;
    }
    out
}

/// 영역을 밀어낼 조각으로 만든다. 한 평면 쌍이 폭을 tol 안으로 맞추지 못하면 영역을 가로 또는 세로로 갈라
/// 더 잘 맞는 쪽을 고른다. 갈라 나누기는 level 번까지다.
fn piece<'a>(
    r: Region,
    view: View2<'a>,
    depth: &dyn Fn(f32, f32) -> Option<Span>,
    tol: f32,
    level: usize,
    min_px: usize,
    out: &mut Vec<Piece<'a>>,
) {
    let (_, f) = view;
    let s = samples(&r, f, depth);
    let Some((lo, hi, err)) = fit(&s) else {
        return;
    };
    if err > tol && level > 0 && s.len() >= 24 {
        // 표본의 가운데 픽셀에서 가르는 두 방법 중 오차가 작은 쪽.
        let mut best: Option<(f32, Vec<Region>)> = None;
        for vertical in [true, false] {
            let mut k: Vec<usize> = s.iter().map(|p| if vertical { p.0 } else { p.1 }).collect();
            k.sort_unstable();
            let parts = cut(&r, vertical, k[k.len() / 2], min_px);
            if parts.len() < 2 {
                continue;
            }
            let e = parts
                .iter()
                .filter_map(|p| fit(&samples(p, f, depth)))
                .map(|x| x.2)
                .fold(0.0f32, f32::max);
            if best.as_ref().is_none_or(|b| e < b.0) {
                best = Some((e, parts));
            }
        }
        if let Some((e, parts)) = best
            && e < 0.8 * err
        {
            for p in parts {
                piece(p, view, depth, tol, level - 1, min_px, out);
            }
            return;
        }
    }
    let lim = |k: usize| {
        s.iter().fold((f32::MAX, f32::MIN), |a, p| {
            let d = if k == 0 { p.4 } else { p.5 };
            (a.0.min(d), a.1.max(d))
        })
    };
    let mut d: Vec<f32> = s.iter().map(|p| p.5 - p.4).collect();
    d.sort_by(f32::total_cmp);
    let line = outline(&r);
    // 윤곽 점은 픽셀 모서리다. 영역 안쪽으로 반 픽셀 들어간 곳에서 잰다.
    let (cx, cy) = line.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
    let (cx, cy) = (cx / line.len() as f32, cy / line.len() as f32);
    let meas = line
        .iter()
        .map(|&(x, y)| {
            let l = ((cx - x).powi(2) + (cy - y).powi(2)).sqrt().max(1e-6);
            let (u, v) = f.from_px(x + 0.5 * (cx - x) / l, y + 0.5 * (cy - y) / l);
            depth(u, v)
        })
        .collect();
    out.push(Piece {
        outline: line,
        meas,
        view,
        lo,
        hi,
        lo_lim: lim(0),
        hi_lim: lim(1),
        min_depth: MIN_DEPTH * d[d.len() / 2],
    });
}

/// side: 옆 사진, front: 앞(없으면 뒤) 사진, top: 위 사진. height 는 피사체 키(미터).
pub fn assemble(
    side: View2,
    front: Option<View2>,
    top: Option<View2>,
    blocks: &[Block],
    height: f32,
    budget: usize,
) -> Result<Mesh, String> {
    let (sm, sf) = side;
    let (w, h) = (sm.w as f32, sm.h as f32);
    let none = |_: f32, _: f32| false;
    let fr = |y: f32, lim: Span| front.and_then(|fv| row_span(fv, y, lim, &none));
    // 옆 상자와 앞 가로 범위를 세계 범위로 옮긴다.
    let bounds = |b: &Block| -> Bounds {
        let (za, ya) = sf.from_px((b.x0.min(b.x1) - MARGIN) * w, (b.y0.min(b.y1) - MARGIN) * h);
        let (zb, yb) = sf.from_px((b.x0.max(b.x1) + MARGIN) * w, (b.y0.max(b.y1) + MARGIN) * h);
        let x = match front {
            Some((fm, ff)) => {
                let fw = fm.w as f32;
                sort(
                    ff.from_px((b.front_x0.min(b.front_x1) - X_MARGIN) * fw, 0.0)
                        .0,
                    ff.from_px((b.front_x0.max(b.front_x1) + X_MARGIN) * fw, 0.0)
                        .0,
                )
            }
            None => (-height, height),
        };
        Bounds {
            x,
            y: sort(ya, yb),
            z: sort(za, zb),
        }
    };
    // 바퀴: 상자 안 옆 마스크의 범위로 중심과 반지름을 잰다.
    let mut wheels: Vec<(f32, f32, f32, Span)> = Vec::new(); // (z, y, 반지름, X 범위)
    // 바퀴 폭은 타이어 아래쪽 줄에서만 잰다. 위쪽 줄에는 포크와 펜더가 겹친다.
    for b in blocks.iter().filter(|b| b.shape == Shape::Wheel) {
        let bd = bounds(b);
        let Some((zc, yc, r)) = fit_wheel(side, &bd) else {
            continue;
        };
        let x = meet(bd.x, spread((yc - r, yc - 0.6 * r), |y| fr(y, bd.x)));
        wheels.push((zc, yc, r, x));
    }
    // 옆 사진에서 바퀴 원 안의 픽셀은 다른 부품이 쓰지 않는다.
    let wheel_px: Vec<(f32, f32, f32)> = wheels
        .iter()
        .map(|&(z, y, r, _)| {
            let (px, py) = sf.to_px(V3::new(0.0, y, z));
            (px, py, r / sf.scale)
        })
        .collect();
    let wheel_at = |x: f32, y: f32| {
        wheel_px
            .iter()
            .position(|&(cx, cy, r)| (x - cx).powi(2) + (y - cy).powi(2) < r * r)
    };
    let in_wheel = |x: f32, y: f32| wheel_at(x, y).is_some();

    // 옆 사진의 픽셀을 부품에 나눈다. 여러 상자에 들면 가장 작은 상자, 어디에도 없으면 가까운 상자다.
    let parts: Vec<(&Block, Bounds)> = blocks
        .iter()
        .filter(|b| b.shape != Shape::Wheel)
        .map(|b| (b, bounds(b)))
        .collect();
    let boxes: Vec<(f32, f32, f32, f32)> = parts
        .iter()
        .map(|(_, bd)| {
            let (a, b) = (
                sf.to_px(V3::new(0.0, bd.y.0, bd.z.0)),
                sf.to_px(V3::new(0.0, bd.y.1, bd.z.1)),
            );
            (a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1))
        })
        .collect();
    // 옆 상자가 같은 부품(왼쪽·오른쪽 사이드백)은 옆 픽셀을 함께 쓴다. 대표는 가장 앞 번호다.
    let tie = 0.01 * w.max(h);
    let rep: Vec<usize> = (0..boxes.len())
        .map(|i| {
            (0..i)
                .find(|&j| {
                    let (a, b) = (boxes[i], boxes[j]);
                    (a.0 - b.0).abs() < tie
                        && (a.1 - b.1).abs() < tie
                        && (a.2 - b.2).abs() < tie
                        && (a.3 - b.3).abs() < tie
                })
                .unwrap_or(i)
        })
        .collect();
    let (sw, sh) = (sm.w, sm.h);
    let unset = usize::MAX;
    let mut label = vec![unset; sw * sh];
    let adopt = ADOPT * h;
    for y in 0..sh {
        for x in 0..sw {
            if !sm.px[y * sw + x] {
                continue;
            }
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            // 바퀴 원 안의 픽셀은 바퀴 옆에 비켜 있는 부품(배기구, 사이드백)만 가진다.
            let wheel = wheel_at(px, py).map(|k| wheels[k].3);
            let beside =
                |i: usize| wheel.is_none_or(|wx| parts[i].1.x.1 < wx.0 || parts[i].1.x.0 > wx.1);
            let dist = |b: &(f32, f32, f32, f32)| {
                let dx = (b.0 - px).max(px - b.2).max(0.0);
                let dy = (b.1 - py).max(py - b.3).max(0.0);
                (dx * dx + dy * dy).sqrt()
            };
            let area = |b: &(f32, f32, f32, f32)| (b.2 - b.0) * (b.3 - b.1);
            let pick = boxes
                .iter()
                .enumerate()
                .filter(|&(i, b)| dist(b) == 0.0 && beside(i))
                .min_by(|a, b| area(a.1).total_cmp(&area(b.1)))
                .or_else(|| {
                    boxes
                        .iter()
                        .enumerate()
                        .filter(|&(i, b)| dist(b) < adopt && beside(i))
                        .min_by(|a, b| dist(a.1).total_cmp(&dist(b.1)))
                });
            if let Some((i, _)) = pick {
                label[y * sw + x] = rep[i];
            }
        }
    }

    let mut pcs: Vec<Piece> = Vec::new();
    let fit_tol = FIT_TOL * height;
    let min_px = |m: &Mask| ((MIN_PIECE * (m.w * m.h) as f32) as usize).max(12);
    for (i, (b, bd)) in parts.iter().enumerate() {
        let wide = widen(bd.x, height);
        match b.shape {
            Shape::Bar => {
                // 앞 사진에서 상자 높이와 가로 범위 안의 픽셀을 앞뒤로 밀어낸다.
                let Some(fv) = front else { continue };
                let (fm, ff) = fv;
                let (a, c) = (
                    ff.to_px(V3::new(bd.x.0, bd.y.0, 0.0)),
                    ff.to_px(V3::new(bd.x.1, bd.y.1, 0.0)),
                );
                let (x0, x1) = (
                    a.0.min(c.0).max(0.0) as usize,
                    (a.0.max(c.0) as usize).min(fm.w),
                );
                let (y0, y1) = (
                    a.1.min(c.1).max(0.0) as usize,
                    (a.1.max(c.1) as usize).min(fm.h),
                );
                let mut fl = vec![unset; fm.w * fm.h];
                for y in y0..y1 {
                    for x in x0..x1 {
                        if fm.px[y * fm.w + x] {
                            fl[y * fm.w + x] = 0;
                        }
                    }
                }
                for r in pieces(&fl, fm.w, fm.h, 0, min_px(fm)) {
                    let depth = |x: f32, y: f32| {
                        let from_side = row_span(side, y, bd.z, &in_wheel);
                        // 위 칸이 있으면 그 열에 막대가 보이는 범위만 쓴다. 안 보이면 깊이를 재지 않는다.
                        match (from_side, top) {
                            (s, Some(tv)) => {
                                // 위 칸의 열에서 옆 칸의 범위와 겹치는 토막만 쓴다. 미러와 레버 사이의 빈 곳을 채우지 않는다.
                                // 겹치는 토막이 없으면(시트의 위 칸이 막대를 조금 다른 자리에 그렸으면) 가장 가까운 토막을 쓴다.
                                let runs = col_runs(tv, x, widen(bd.z, height));
                                let c = s.map_or((bd.z.0 + bd.z.1) / 2.0, |s| (s.0 + s.1) / 2.0);
                                s.and_then(|s| union(runs.iter().map(|&t| both(s, t))))
                                    .or_else(|| {
                                        runs.iter().copied().min_by(|a, b| {
                                            ((a.0 + a.1) / 2.0 - c)
                                                .abs()
                                                .total_cmp(&((b.0 + b.1) / 2.0 - c).abs())
                                        })
                                    })
                            }
                            (s, None) => s,
                        }
                    };
                    piece(r, fv, &depth, fit_tol, SPLITS, min_px(fm), &mut pcs);
                }
            }
            _ => {
                for r in pieces(&label, sw, sh, rep[i], min_px(sm)) {
                    // 폭: 그 높이의 앞 줄과 그 길이의 위 줄이 겹치는 범위. 없으면 claude 의 앞 가로 범위.
                    let depth = |z: f32, y: f32| {
                        let from_front = fr(y, wide);
                        let from_top = top.and_then(|tv| row_span(tv, z, wide, &none));
                        Some(match (from_front, from_top) {
                            (Some(f), t) => meet(f, t),
                            (None, Some(t)) => t,
                            (None, None) => bd.x,
                        })
                    };
                    piece(r, side, &depth, fit_tol, SPLITS, min_px(sm), &mut pcs);
                }
            }
        }
    }
    if pcs.is_empty() && wheels.is_empty() {
        return Err("부품의 윤곽을 재지 못했습니다".into());
    }

    // 예산 안에 들 때까지 윤곽을 줄인다.
    let mut tol = SIMPLIFY * h;
    loop {
        let mut mesh = Mesh::default();
        for p in &pcs {
            let (_, f) = p.view;
            let mut keep = simplify(&p.outline, tol);
            if keep.len() < 3 {
                continue;
            }
            let mut poly: Vec<(f32, f32)> = keep
                .iter()
                .map(|&i| f.from_px(p.outline[i].0, p.outline[i].1))
                .collect();
            if signed_area(&poly) < 0.0 {
                poly.reverse();
                keep.reverse();
            }
            let (mut lo, mut hi) = (Vec::new(), Vec::new());
            for (&(u, v), &i) in poly.iter().zip(&keep) {
                let mut a = eval(p.lo, u, v).clamp(p.lo_lim.0, p.lo_lim.1);
                let mut b = eval(p.hi, u, v).clamp(p.hi_lim.0, p.hi_lim.1);
                // 그 자리에서 잰 범위 밖으로 나가지 않는다. 넘는 점만 끝면이 평면에서 벗어난다.
                if let Some(m) = p.meas[i] {
                    a = a.clamp(m.0, m.1);
                    b = b.clamp(m.0, m.1);
                }
                let (mid, half) = ((a + b) / 2.0, ((b - a) / 2.0).max(p.min_depth / 2.0));
                lo.push(mid - half);
                hi.push(mid + half);
            }
            match f.view {
                View::Side => prism(&mut mesh, &poly, &lo, &hi, |z, y, x| V3::new(x, y, z)),
                _ => prism(&mut mesh, &poly, &lo, &hi, V3::new),
            }
        }
        for &(zc, yc, r, x) in &wheels {
            wheel(&mut mesh, zc, yc, r, x);
        }
        if mesh.triangle_count() <= budget {
            return Ok(mesh);
        }
        if tol > 0.1 * h {
            return Err(format!(
                "삼각형 {} 개가 예산 {budget} 을 넘었습니다",
                mesh.triangle_count()
            ));
        }
        tol *= 1.3;
    }
}

/// 바퀴: 모서리를 깎은 타이어와 양쪽에서 안으로 들어간 허브.
fn wheel(mesh: &mut Mesh, zc: f32, yc: f32, r: f32, x: Span) {
    let half = (x.1 - x.0) / 2.0;
    let ch = WHEEL_CHAMFER * half;
    let dep = HUB_DEPTH * half;
    let circle = |xx: f32, s: f32| -> Vec<V3> {
        (0..WHEEL_SIDES)
            .map(|i| {
                let a = i as f32 / WHEEL_SIDES as f32 * std::f32::consts::TAU;
                V3::new(xx, yc + r * s * a.sin(), zc + r * s * a.cos())
            })
            .collect()
    };
    loft(
        mesh,
        &[
            circle(x.0 + dep, HUB),
            circle(x.0 + RIM * half, TIRE_INNER),
            circle(x.0, SIDEWALL),
            circle(x.0 + ch, 1.0),
            circle(x.1 - ch, 1.0),
            circle(x.1, SIDEWALL),
            circle(x.1 - RIM * half, TIRE_INNER),
            circle(x.1 - dep, HUB),
        ],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volume(m: &Mesh) -> f32 {
        m.tris
            .iter()
            .map(|t| {
                let [p, q, r] = t.map(|i| m.positions[i as usize]);
                p.dot(q.cross(r)) / 6.0
            })
            .sum()
    }

    #[test]
    fn l_shaped_prism_is_closed_and_outward() {
        // L 자 윤곽(넓이 3)을 두께 2 로 밀면 부피 6 이다.
        let poly = [
            (0.0, 0.0),
            (2.0, 0.0),
            (2.0, 1.0),
            (1.0, 1.0),
            (1.0, 2.0),
            (0.0, 2.0),
        ];
        let mut m = Mesh::default();
        prism(&mut m, &poly, &[-1.0; 6], &[1.0; 6], |z, y, x| {
            V3::new(x, y, z)
        });
        assert_eq!(m.triangle_count(), 4 * 6 - 4);
        assert!((volume(&m) - 6.0).abs() < 1e-4, "volume {}", volume(&m));
    }

    #[test]
    fn outline_of_a_square_simplifies_to_four_corners() {
        let r = Region {
            x0: 3,
            y0: 5,
            w: 10,
            h: 10,
            px: vec![true; 100],
        };
        let o = outline(&r);
        let p: Vec<(f32, f32)> = simplify(&o, 0.5).iter().map(|&i| o[i]).collect();
        assert_eq!(p.len(), 4, "{p:?}");
        assert!((signed_area(&p).abs() - 100.0).abs() < 1e-3);
        assert!(p.contains(&(3.0, 5.0)) && p.contains(&(13.0, 15.0)));
    }

    #[test]
    fn wheel_is_closed_and_outward() {
        let mut m = Mesh::default();
        wheel(&mut m, 0.0, 0.3, 0.3, (-0.06, 0.06));
        assert!(volume(&m) > 0.0);
        assert!(volume(&m) < std::f32::consts::PI * 0.09 * 0.12);
    }
}
