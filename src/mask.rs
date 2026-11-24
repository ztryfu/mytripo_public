//! 단색 배경(초록 또는 흰색) 사진에서 피사체 마스크를 만든다.

use image::RgbImage;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mask {
    pub w: usize,
    pub h: usize,
    pub px: Vec<bool>,
}

/// 포함 경계 상자. x1, y1 도 마스크 안의 픽셀이다.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bbox {
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
}

impl Bbox {
    pub fn height(&self) -> usize {
        self.y1 - self.y0 + 1
    }
}

impl Mask {
    pub fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            px: vec![false; w * h],
        }
    }

    pub fn get(&self, x: i64, y: i64) -> bool {
        x >= 0
            && y >= 0
            && (x as usize) < self.w
            && (y as usize) < self.h
            && self.px[y as usize * self.w + x as usize]
    }

    /// 실수 좌표(픽셀 가장자리 기준)가 마스크 픽셀 안에 드는지.
    pub fn at(&self, x: f32, y: f32) -> bool {
        self.get(x.floor() as i64, y.floor() as i64)
    }

    pub fn set(&mut self, x: usize, y: usize, v: bool) {
        self.px[y * self.w + x] = v;
    }

    pub fn area(&self) -> usize {
        self.px.iter().filter(|&&v| v).count()
    }

    pub fn bbox(&self) -> Option<Bbox> {
        let mut b: Option<Bbox> = None;
        for y in 0..self.h {
            for x in 0..self.w {
                if self.px[y * self.w + x] {
                    let e = b.get_or_insert(Bbox {
                        x0: x,
                        y0: y,
                        x1: x,
                        y1: y,
                    });
                    e.x0 = e.x0.min(x);
                    e.x1 = e.x1.max(x);
                    e.y0 = e.y0.min(y);
                    e.y1 = e.y1.max(y);
                }
            }
        }
        b
    }

    /// 행 y 의 마스크 구간들. (시작, 끝) 은 픽셀 가장자리 좌표다.
    pub fn runs(&self, y: i64) -> Vec<(f32, f32)> {
        let mut out = Vec::new();
        if y < 0 || y as usize >= self.h {
            return out;
        }
        let row = &self.px[y as usize * self.w..(y as usize + 1) * self.w];
        let mut start = None;
        for (x, &v) in row.iter().enumerate() {
            match (v, start) {
                (true, None) => start = Some(x),
                (false, Some(s)) => {
                    out.push((s as f32, x as f32));
                    start = None;
                }
                _ => {}
            }
        }
        if let Some(s) = start {
            out.push((s as f32, self.w as f32));
        }
        out
    }

    /// 행 y0..=y1 의 마스크 픽셀 x 중심 평균.
    pub fn mean_x(&self, y0: usize, y1: usize) -> Option<f32> {
        let (mut sum, mut n) = (0.0f64, 0usize);
        for y in y0..=y1.min(self.h.saturating_sub(1)) {
            for x in 0..self.w {
                if self.px[y * self.w + x] {
                    sum += x as f64 + 0.5;
                    n += 1;
                }
            }
        }
        (n > 0).then(|| (sum / n as f64) as f32)
    }

    /// 테두리에 닿지 않는 작은 구멍(넓이의 1/50 이하)을 메운다.
    pub fn fill_holes(&mut self) {
        let (w, h) = (self.w, self.h);
        let max_hole = self.area() / 50;
        for hole in components(self, false) {
            let touches_border = hole.iter().any(|&i| {
                let (x, y) = (i % w, i / w);
                x == 0 || y == 0 || x == w - 1 || y == h - 1
            });
            if !touches_border && hole.len() <= max_hole {
                for i in hole {
                    self.px[i] = true;
                }
            }
        }
    }

    pub fn iou(&self, other: &Mask) -> f32 {
        assert_eq!((self.w, self.h), (other.w, other.h), "mask size differs");
        let (mut inter, mut uni) = (0usize, 0usize);
        for (a, b) in self.px.iter().zip(&other.px) {
            inter += usize::from(*a && *b);
            uni += usize::from(*a || *b);
        }
        if uni == 0 {
            1.0
        } else {
            inter as f32 / uni as f32
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Background {
    Green,
    White,
}

fn greenness(p: [u8; 3]) -> i32 {
    p[1] as i32 - (p[0] as i32).max(p[2] as i32)
}

fn chroma(p: [u8; 3]) -> i32 {
    let mx = p.iter().max().copied().unwrap_or(0) as i32;
    let mn = p.iter().min().copied().unwrap_or(0) as i32;
    mx - mn
}

fn luma(p: [u8; 3]) -> i32 {
    (p[0] as i32 + p[1] as i32 + p[2] as i32) / 3
}

fn is_background(kind: Background, bg: [u8; 3], p: [u8; 3]) -> bool {
    match kind {
        Background::Green => greenness(p) * 2 > greenness(bg),
        Background::White => chroma(p) < 30 && luma(p) > luma(bg) - 50,
    }
}

/// 테두리 픽셀의 중앙값으로 배경 종류와 색을 정한다.
pub fn detect_background(img: &RgbImage) -> Result<(Background, [u8; 3]), String> {
    let (w, h) = img.dimensions();
    if w < 8 || h < 8 {
        return Err("사진이 너무 작습니다".into());
    }
    let mut border = Vec::new();
    for x in 0..w {
        border.push(img.get_pixel(x, 0).0);
        border.push(img.get_pixel(x, h - 1).0);
    }
    for y in 0..h {
        border.push(img.get_pixel(0, y).0);
        border.push(img.get_pixel(w - 1, y).0);
    }
    let mut med = [0u8; 3];
    for (c, m) in med.iter_mut().enumerate() {
        let mut v: Vec<u8> = border.iter().map(|p| p[c]).collect();
        v.sort_unstable();
        *m = v[v.len() / 2];
    }
    let kind = if greenness(med) > 40 {
        Background::Green
    } else if med.iter().all(|&c| c > 170) && chroma(med) < 40 {
        Background::White
    } else {
        return Err("배경이 초록색이나 흰색 단색이 아닙니다".into());
    };
    let bg_share = border
        .iter()
        .filter(|&&p| is_background(kind, med, p))
        .count() as f32
        / border.len() as f32;
    if bg_share < 0.75 {
        return Err("배경이 고른 단색이 아닙니다".into());
    }
    Ok((kind, med))
}

/// 배경색이 아닌 픽셀 전부. 덩어리를 고르지 않는다.
pub fn foreground(img: &RgbImage) -> Result<(Mask, Background), String> {
    let (kind, bg) = detect_background(img)?;
    let mut fg = Mask::new(img.width() as usize, img.height() as usize);
    for (x, y, p) in img.enumerate_pixels() {
        fg.set(x as usize, y as usize, !is_background(kind, bg, p.0));
    }
    Ok((fg, kind))
}

/// 배경을 지운 피사체 마스크. 가장 큰 덩어리만 남기고 작은 구멍을 메운다.
pub fn cut_out(img: &RgbImage) -> Result<(Mask, Background), String> {
    let (fg, kind) = foreground(img)?;
    let (w, h) = (fg.w, fg.h);
    let comps = components(&fg, true);
    let largest = comps
        .iter()
        .max_by_key(|c| c.len())
        .ok_or("사진에서 피사체를 찾지 못했습니다")?;
    if largest.len() < w * h / 400 {
        return Err("사진에서 피사체를 찾지 못했습니다".into());
    }
    let mut out = Mask::new(w, h);
    for &i in largest {
        out.px[i] = true;
    }
    out.fill_holes();
    Ok((out, kind))
}

/// 값이 `value` 인 픽셀의 4-연결 덩어리들.
fn components(m: &Mask, value: bool) -> Vec<Vec<usize>> {
    let mut seen = vec![false; m.px.len()];
    let mut out = Vec::new();
    for start in 0..m.px.len() {
        if seen[start] || m.px[start] != value {
            continue;
        }
        seen[start] = true;
        let mut comp = vec![start];
        let mut stack = vec![start];
        while let Some(i) = stack.pop() {
            let (x, y) = (i % m.w, i / m.w);
            let mut visit = |j: usize| {
                if !seen[j] && m.px[j] == value {
                    seen[j] = true;
                    comp.push(j);
                    stack.push(j);
                }
            };
            if x > 0 {
                visit(i - 1);
            }
            if x + 1 < m.w {
                visit(i + 1);
            }
            if y > 0 {
                visit(i - m.w);
            }
            if y + 1 < m.h {
                visit(i + m.w);
            }
        }
        out.push(comp);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    fn scene(bg: [u8; 3]) -> RgbImage {
        let mut img = RgbImage::from_pixel(60, 80, Rgb(bg));
        for y in 10..70 {
            for x in 20..40 {
                img.put_pixel(x, y, Rgb([180, 40, 40]));
            }
        }
        // 피사체 안의 배경색 점 하나(구멍)와 떨어진 잡티 하나
        img.put_pixel(30, 40, Rgb(bg));
        img.put_pixel(5, 5, Rgb([10, 10, 10]));
        img
    }

    #[test]
    fn green_background_is_removed() {
        let (m, kind) = cut_out(&scene([20, 180, 60])).unwrap();
        assert_eq!(kind, Background::Green);
        assert_eq!(m.area(), 20 * 60);
        assert_eq!(
            m.bbox(),
            Some(Bbox {
                x0: 20,
                y0: 10,
                x1: 39,
                y1: 69
            })
        );
    }

    #[test]
    fn white_background_is_removed() {
        let (m, kind) = cut_out(&scene([250, 250, 248])).unwrap();
        assert_eq!(kind, Background::White);
        assert_eq!(m.area(), 20 * 60);
    }

    #[test]
    fn busy_background_is_rejected() {
        let img = RgbImage::from_fn(60, 80, |x, y| Rgb([(x * 4) as u8, (y * 3) as u8, 90]));
        assert!(cut_out(&img).is_err());
    }
}
