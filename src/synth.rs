//! Генерация синтетических чертежей для регрессионных тестов.
//!
//! Лицензированные сканы в репозиторий класть нельзя, поэтому эталоны
//! рисуются процедурно: рамка, сетка прямых, дуга, окружность и подпись.
//! Такой лист детерминирован (фиксированное зерно) и позволяет в CI
//! проверять recall/precision на известной геометрии.
//!
//! Использование: `png2dxf gen-test-image out.png [ширина] [высота]`

use crate::geom::{Pt, Seg};
use image::{GrayImage, Luma};

/// Геометрия эталонного листа в пикселях: что должно быть найдено.
#[derive(Clone, Debug, Default)]
pub struct GroundTruth {
    pub segs: Vec<Seg>,
    pub circles: Vec<(Pt, f64)>,
}

/// Рисует эталонный лист и возвращает изображение вместе с эталонной геометрией.
pub fn make_sheet(w: u32, h: u32) -> (GrayImage, GroundTruth) {
    let mut img = GrayImage::from_pixel(w, h, Luma([252]));
    let mut gt = GroundTruth::default();
    let (fw, fh) = (w as f64, h as f64);

    let line = |img: &mut GrayImage, a: Pt, b: Pt, thickness: u32, ink: u8| {
        // Растеризация линии Брезенхема с толщиной.
        let (mut x0, mut y0) = (a.x.round() as i64, a.y.round() as i64);
        let (x1, y1) = (b.x.round() as i64, b.y.round() as i64);
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        loop {
            for oy in 0..thickness as i64 {
                for ox in 0..thickness as i64 {
                    let x = x0 + ox - thickness as i64 / 2;
                    let y = y0 + oy - thickness as i64 / 2;
                    if x >= 0 && y >= 0 && (x as u32) < img.width() && (y as u32) < img.height() {
                        img.put_pixel(x as u32, y as u32, Luma([ink]));
                    }
                }
            }
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
    };

    // Рамка листа.
    let m = fw * 0.06;
    let frame = [
        (Pt::new(m, m), Pt::new(fw - m, m)),
        (Pt::new(fw - m, m), Pt::new(fw - m, fh - m)),
        (Pt::new(fw - m, fh - m), Pt::new(m, fh - m)),
        (Pt::new(m, fh - m), Pt::new(m, m)),
    ];
    for (a, b) in frame {
        line(&mut img, a, b, 2, 20);
        gt.segs.push(Seg { a, b, votes: 1 });
    }

    // Внутренний прямоугольник «здания».
    let (bx0, by0) = (fw * 0.18, fh * 0.22);
    let (bx1, by1) = (fw * 0.52, fh * 0.62);
    let building = [
        (Pt::new(bx0, by0), Pt::new(bx1, by0)),
        (Pt::new(bx1, by0), Pt::new(bx1, by1)),
        (Pt::new(bx1, by1), Pt::new(bx0, by1)),
        (Pt::new(bx0, by1), Pt::new(bx0, by0)),
    ];
    for (a, b) in building {
        line(&mut img, a, b, 2, 25);
        gt.segs.push(Seg { a, b, votes: 1 });
    }

    // Две внутренние перегородки и диагональ.
    let parts = [
        (Pt::new(fw * 0.35, by0), Pt::new(fw * 0.35, by1)),
        (Pt::new(bx0, fh * 0.42), Pt::new(bx1, fh * 0.42)),
        (Pt::new(fw * 0.60, fh * 0.70), Pt::new(fw * 0.86, fh * 0.86)),
    ];
    for (a, b) in parts {
        line(&mut img, a, b, 2, 30);
        gt.segs.push(Seg { a, b, votes: 1 });
    }

    // Окружность справа и дуга снизу.
    let (cx, cy, r) = (fw * 0.74, fh * 0.30, fw * 0.09);
    gt.circles.push((Pt::new(cx, cy), r));
    for k in 0..720 {
        let t = k as f64 / 720.0 * std::f64::consts::TAU;
        let p = Pt::new(cx + r * t.cos(), cy + r * t.sin());
        line(&mut img, p, Pt::new(p.x + 0.8, p.y), 2, 30);
    }
    let (ax, ay, ar) = (fw * 0.70, fh * 0.80, fw * 0.12);
    for k in 0..360 {
        let t = std::f64::consts::PI + k as f64 / 360.0 * std::f64::consts::PI;
        let p = Pt::new(ax + ar * t.cos(), ay + ar * t.sin());
        line(&mut img, p, Pt::new(p.x + 0.8, p.y), 2, 30);
    }

    // Пунктирная линия по низу: штрихи по 10 px с зазором 8 px.
    let (py, px0, px1) = (fh * 0.90, fw * 0.10, fw * 0.60);
    let mut x = px0;
    while x + 10.0 < px1 {
        line(&mut img, Pt::new(x, py), Pt::new(x + 10.0, py), 2, 40);
        x += 18.0;
    }

    // Псевдо-подпись: набор мелких блобов, который не должен стать линиями.
    let (tx, ty) = (fw * 0.62, fh * 0.10);
    for row in 0..2 {
        for col in 0..14 {
            let ox = tx + col as f64 * 9.0;
            let oy = ty + row as f64 * 14.0;
            line(&mut img, Pt::new(ox, oy), Pt::new(ox + 6.0, oy), 2, 60);
            line(&mut img, Pt::new(ox, oy), Pt::new(ox, oy + 9.0), 2, 60);
        }
    }

    // Лёгкий шум скана, чтобы пороги не были идеальными.
    let mut seed = 0x1234_5678u64;
    for y in 0..h {
        for x in 0..w {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            if (seed >> 33) % 97 == 0 {
                let v = img.get_pixel(x, y)[0].saturating_sub(18);
                img.put_pixel(x, y, Luma([v]));
            }
        }
    }
    (img, gt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sheet_has_expected_geometry() {
        let (img, gt) = make_sheet(1200, 900);
        assert_eq!(img.dimensions(), (1200, 900));
        // Рамка + здание + перегородки.
        assert!(gt.segs.len() >= 11, "segs={}", gt.segs.len());
        assert_eq!(gt.circles.len(), 1);
        let ink = img.as_raw().iter().filter(|&&p| p < 128).count();
        assert!(ink > 2000, "ink={ink}");
    }
}
