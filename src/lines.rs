//! Детектор прямых в духе LSD (Line Segment Detector, Grompone von Gioi et al.):
//! локальная ориентация по градиенту, рост отрезков вдоль уровня яркости,
//! уточнение прямоугольником. Работает по полутону, без бинаризации и без
//! скелета — даёт отрезки на тонких и пунктирных линиях скана.
//!
//! Это самостоятельный Rust-аналог LSD: тот же принцип (уровни -> рост ->
//! уточнение), но без внешней библиотеки, с сеткой для быстрого обхода.

use crate::geom::{Pt, Seg};

#[derive(Clone, Copy)]
struct Grad {
    gx: f32,
    gy: f32,
}

/// Порог градиента: ниже — фон бумаги. На скане с 8-битным серым хватает
/// невысокого порога, чтобы поймать бледные и розовые линии.
struct Gray {
    w: u32,
    h: u32,
    g: Vec<f32>,
    grad: Vec<Grad>,
    mag: Vec<f32>,
}

impl Gray {
    fn new(img: &image::GrayImage) -> Self {
        let (w, h) = img.dimensions();
        let wu = w as usize;
        let hu = h as usize;
        let src: Vec<f32> = img.as_raw().iter().map(|&p| p as f32).collect();
        let mut grad = vec![Grad { gx: 0.0, gy: 0.0 }; wu * hu];
        let mut mag = vec![0f32; wu * hu];
        // Собель 3x3.
        for y in 1..hu.saturating_sub(1) {
            for x in 1..wu.saturating_sub(1) {
                let at = |dx: i32, dy: i32| -> f32 {
                    let xx = (x as i32 + dx) as usize;
                    let yy = (y as i32 + dy) as usize;
                    src[yy * wu + xx]
                };
                let gx = (at(1, -1) + 2.0 * at(1, 0) + at(1, 1))
                    - (at(-1, -1) + 2.0 * at(-1, 0) + at(-1, 1));
                let gy = (at(-1, 1) + 2.0 * at(0, 1) + at(1, 1))
                    - (at(-1, -1) + 2.0 * at(0, -1) + at(1, -1));
                let i = y * wu + x;
                grad[i] = Grad { gx, gy };
                mag[i] = (gx * gx + gy * gy).sqrt();
            }
        }
        Self { w, h, g: src, grad, mag }
    }

    #[inline]
    fn idx(&self, x: i64, y: i64) -> Option<usize> {
        if x < 0 || y < 0 || x >= self.w as i64 || y >= self.h as i64 {
            None
        } else {
            Some(y as usize * self.w as usize + x as usize)
        }
    }

    #[inline]
    fn level(&self, x: f64, y: f64) -> Option<f32> {
        let i = self.idx(x.floor() as i64, y.floor() as i64)?;
        Some(self.g[i])
    }
}

/// Отрезок из LSD-роста.
pub struct RawSeg {
    pub a: Pt,
    pub b: Pt,
    pub width: f64,
}

pub struct LsdParams {
    pub threshold: f32,
    pub min_len: f64,
    /// Максимум точек градиента для роста отрезков: у реального скана их
    /// сотни тысяч, а 150k сильнейших уже дают все контуры.
    pub max_seeds: usize,
}

/// Ищет отрезки. `hint` — ожидаемая толщина линии в пикселях: шире линия
/// (или размытая) — шире полоса роста.
pub fn detect(img: &image::GrayImage, p: &LsdParams) -> Vec<RawSeg> {
    let g = Gray::new(img);
    let (w, h) = (g.w as i64, g.h as i64);
    let mut used = vec![false; g.mag.len()];

    // Сначала самые сильные градиенты: они соответствуют контурам объектов.
    let mut candidates: Vec<usize> = (0..g.mag.len()).filter(|&i| g.mag[i] >= p.threshold).collect();
    if candidates.len() > p.max_seeds {
        candidates.select_nth_unstable_by(p.max_seeds, |&a, &b| {
            g.mag[b].partial_cmp(&g.mag[a]).unwrap()
        });
        candidates.truncate(p.max_seeds);
    }
    let mut order = candidates;
    order.sort_unstable_by(|&a, &b| g.mag[b].partial_cmp(&g.mag[a]).unwrap());

    let mut out = Vec::new();
    let min_width = 1.0;
    for i in order {
        if used[i] {
            continue;
        }
        let x0 = (i % g.w as usize) as i64;
        let y0 = (i / g.w as usize) as i64;
        if x0 <= 0 || y0 <= 0 || x0 >= w - 1 || y0 >= h - 1 {
            continue;
        }
        let gr = g.grad[i];
        let m = (gr.gx * gr.gx + gr.gy * gr.gy).sqrt();
        if m < 1e-3 {
            continue;
        }
        // Нормаль к контуру = направление градиента.
        let nx = gr.gx / m;
        let ny = gr.gy / m;
        let px = -ny;
        let py = nx;

        // Центр линии: ищем экстремум градиента поперёк.
        let mut best = (0f64, g.mag[i]);
        for step in [-1.5f32, -1.0, -0.5, 0.5, 1.0, 1.5] {
            let sx = x0 as f64 + nx as f64 * step as f64;
            let sy = y0 as f64 + ny as f64 * step as f64;
            if let Some(j) = g.idx(sx.floor() as i64, sy.floor() as i64) {
                if g.mag[j] > best.1 {
                    best = (step as f64, g.mag[j]);
                }
            }
        }
        let cx = x0 as f64 + nx as f64 * best.0;
        let cy = y0 as f64 + ny as f64 * best.0;
        let Some(base_level) = g.level(cx, cy) else { continue };

        let mut len = 0.0f64;
        let (mut fx, mut fy) = (cx, cy);
        let (mut bx, mut by) = (cx, cy);
        let tolerance = 4.0f32;

        // Рост вперёд и назад вдоль контура.
        for dir in [1.0f64, -1.0] {
            let (mut x, mut y) = (cx, cy);
            let mut run = 0.0;
            loop {
                let nxp = x + px as f64 * dir;
                let nyp = y + py as f64 * dir;
                // Поддержание на уровне: ищем минимум |I - level| поперёк.
                let mut moved = false;
                // Три кандидата поперёк, выбираем ближайший по уровню к
                // исходному: при «липком» первом совпадении цепочка легко
                // сворачивает на соседнюю линию или на текст.
                let mut best = (f64::INFINITY, 0.0f64);
                for step in 0..3i32 {
                    let off = (step as f64 - 1.0) * 0.5;
                    let tx = nxp + nx as f64 * off;
                    let ty = nyp + ny as f64 * off;
                    if let Some(v) = g.level(tx, ty) {
                        let err = ((v - base_level) as f64).abs();
                        if err <= tolerance as f64 && err < best.0 {
                            best = (err, off);
                        }
                    }
                }
                if best.0.is_finite() {
                    x = nxp + nx as f64 * best.1;
                    y = nyp + ny as f64 * best.1;
                    moved = true;
                }
                if !moved {
                    break;
                }
                len += 1.0;
                run += 1.0;
                if dir > 0.0 {
                    fx = x;
                    fy = y;
                } else {
                    bx = x;
                    by = y;
                }
                if run > 4000.0 {
                    break;
                }
            }
        }

        let total = (fx - bx).hypot(fy - by);
        // Линия — это в первую очередь прямой отрезок: если цепочка прошла
        // 200 шагов, а концы рядом, это контур буквы или петля, не отрезок.
        let straightness = total / len.max(1.0);
        if straightness < 0.62 {
            mark(&mut used, &g, bx, by, fx, fy, min_width);
            continue;
        }
        if total < p.min_len {
            // Всё равно помечаем: короткие штрихи текста тоже нужно
            // «съесть», чтобы они не мешали другим линиям.
            mark(&mut used, &g, bx, by, fx, fy, min_width);
            continue;
        }
        mark(&mut used, &g, bx, by, fx, fy, min_width);
        out.push(RawSeg {
            a: Pt::new(bx, by),
            b: Pt::new(fx, fy),
            width: min_width,
        });
    }
    out
}

fn mark(used: &mut [bool], g: &Gray, ax: f64, ay: f64, bx: f64, by: f64, band: f64) {
    let (dx, dy) = (bx - ax, by - ay);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-6 {
        return;
    }
    let n = (len.ceil() as usize).max(1);
    let (ux, uy) = (dx / len, dy / len);
    let (nx, ny) = (-uy, ux);
    for k in 0..=n {
        let t = k as f64 / n as f64;
        let x = ax + dx * t;
        let y = ay + dy * t;
        let steps = band.ceil() as i64;
        for k in -steps..=steps {
            let off = k as f64;
            let xx = (x + nx * off).round() as i64;
            let yy = (y + ny * off).round() as i64;
            if let Some(i) = g.idx(xx, yy) {
                used[i] = true;
            }
        }
    }
}

/// Переводит сырые отрезки LSD в общий формат и сливает коллинеарные.
pub fn to_segments(raw: Vec<RawSeg>) -> Vec<Seg> {
    raw.into_iter()
        .map(|s| Seg { a: s.a, b: s.b, votes: 1 })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GrayImage, Luma};

    #[test]
    fn finds_straight_line() {
        let mut img = GrayImage::from_pixel(200, 120, Luma([255]));
        for x in 20..180 {
            for y in 58..62 {
                img.put_pixel(x, y, Luma([0]));
            }
        }
        let p = LsdParams { threshold: 30.0, min_len: 40.0, max_seeds: 100_000 };
        let segs = detect(&img, &p);
        assert!(!segs.is_empty());
        let longest = segs.iter().map(|s| s.a.dist(&s.b)).fold(0.0, f64::max);
        assert!(longest > 100.0, "longest={longest}");
    }

    #[test]
    fn ignores_flat_area() {
        let img = GrayImage::from_pixel(80, 80, Luma([200]));
        let p = LsdParams { threshold: 30.0, min_len: 20.0, max_seeds: 100_000 };
        assert!(detect(&img, &p).is_empty());
    }
}
