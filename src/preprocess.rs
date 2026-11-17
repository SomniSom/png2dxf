//! Предобработка растра: grayscale, шумоподавление, адаптивная бинаризация,
//! морфологическая чистка и скелетизация.

use image::{GrayImage, Luma};
use imageproc::distance_transform::Norm;
use imageproc::morphology;

#[inline]
fn to_gray(p: image::Rgba<u8>) -> u8 {
    let [r, g, b, a] = p.0;
    let lum = (r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000;
    ((lum * a as u32 + 255 * (255 - a as u32)) / 255) as u8
}

/// Полутон с усилением цветных линий: тусклая розовая граница участка на
/// скане по яркости почти фон, но заметно цветная. Затемняем по насыщенности,
/// чтобы такая линия попала в бинаризацию.
pub fn grayscale_colored(src: &image::RgbaImage) -> GrayImage {
    let (w, h) = src.dimensions();
    let mut out = GrayImage::new(w, h);
    for (x, y, px) in src.enumerate_pixels() {
        let [r, g, b, a] = px.0;
        let max = r.max(g).max(b) as f32;
        let min = r.min(g).min(b) as f32;
        let sat = if max > 1.0 { (max - min) / max } else { 0.0 };
        let base = to_gray(*px) as f32;
        // Насыщенный тон -> вычитаем долю яркости, оставляя линию тёмной.
        let boost = 150.0 * sat;
        let v = (base - boost).clamp(0.0, 255.0) as u8;
        out.put_pixel(x, y, Luma([v]));
    }
    out
}

pub fn grayscale(src: &image::RgbaImage) -> GrayImage {
    let (w, h) = src.dimensions();
    let mut out = GrayImage::new(w, h);
    for (x, y, px) in src.enumerate_pixels() {
        out.put_pixel(x, y, Luma([to_gray(*px)]));
    }
    out
}

fn gaussian_kernel(sigma: f32) -> (Vec<f32>, i32) {
    let radius = (sigma * 3.0).ceil() as i32;
    let mut k = Vec::with_capacity((2 * radius + 1) as usize);
    let mut sum = 0.0f32;
    for i in -radius..=radius {
        let v = (-((i * i) as f32) / (2.0 * sigma * sigma)).exp();
        k.push(v);
        sum += v;
    }
    for v in &mut k {
        *v /= sum;
    }
    (k, radius)
}

/// Разделяемый гауссов фильтр по строке (fast path при малом радиусе).
pub fn gaussian_blur(src: &GrayImage, sigma: f32) -> GrayImage {
    if sigma <= 0.1 {
        return src.clone();
    }
    let (w, h) = src.dimensions();
    let (k, radius) = gaussian_kernel(sigma);
    let r = radius as usize;
    let mut tmp = vec![0f32; (w * h) as usize];
    let mut out = GrayImage::new(w, h);

    for y in 0..h {
        let row = y * w;
        for x in 0..w {
            let mut acc = 0f32;
            for (i, kw) in k.iter().enumerate() {
                let sx = (x as i64 + i as i64 - r as i64).clamp(0, w as i64 - 1) as u32;
                acc += src.get_pixel(sx, y)[0] as f32 * kw;
            }
            tmp[(row + x) as usize] = acc;
        }
    }
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0f32;
            for (i, kw) in k.iter().enumerate() {
                let sy = (y as i64 + i as i64 - r as i64).clamp(0, h as i64 - 1) as u32;
                acc += tmp[(sy * w + x) as usize] * kw;
            }
            out.put_pixel(x, y, Luma([acc.clamp(0.0, 255.0) as u8]));
        }
    }
    out
}

/// Адаптивный порог (Gaussian C, THRESH_BINARY_INV): линия = 255.
pub fn adaptive_binarize(src: &GrayImage, radius: u32, offset: i32) -> GrayImage {
    let radius = radius.max(2);
    let (w, h) = src.dimensions();
    let size = (2 * radius + 1) as usize;

    let mut integral = vec![0u64; ((w + 1) * (h + 1)) as usize];
    let stride = (w + 1) as usize;
    for y in 0..h {
        let mut row_sum = 0u64;
        for x in 0..w {
            row_sum += src.get_pixel(x, y)[0] as u64;
            integral[(y as usize + 1) * stride + x as usize + 1] =
                integral[y as usize * stride + x as usize + 1] + row_sum;
        }
    }
    let mut out = GrayImage::new(w, h);
    for y in 0..h {
        let y0 = y.saturating_sub(radius) as usize;
        let y1 = (y + radius).min(h - 1) as usize + 1;
        for x in 0..w {
            let x0 = x.saturating_sub(radius) as usize;
            let x1 = (x + radius).min(w - 1) as usize + 1;
            let sum = integral[y1 * stride + x1] + integral[y0 * stride + x0]
                - integral[y0 * stride + x1]
                - integral[y1 * stride + x0];
            let count = ((y1 - y0) * (x1 - x0)) as u64;
            debug_assert!(count <= (size * size) as u64);
            let mean = (sum as f32) / (count as f32);
            let v = src.get_pixel(x, y)[0] as f32;
            let ink = v < mean - offset as f32;
            out.put_pixel(x, y, Luma([if ink { 255 } else { 0 }]));
        }
    }
    out
}

/// Расширение маски на `r` пикселей (квадратом 2r+1): склеивает отдельные
/// пиксели маски в блобы — нужно для группировки символов в строки.
pub fn dilate(src: &GrayImage, r: u32) -> GrayImage {
    let (w, h) = src.dimensions();
    let mut out = GrayImage::new(w, h);
    let r = r as i64;
    for y in 0..h {
        for x in 0..w {
            let mut ink = 0u8;
            'outer: for dy in -r..=r {
                for dx in -r..=r {
                    let nx = x as i64 + dx;
                    let ny = y as i64 + dy;
                    if nx >= 0 && ny >= 0 && nx < w as i64 && ny < h as i64
                        && src.get_pixel(nx as u32, ny as u32)[0] > 0
                    {
                        ink = 255;
                        break 'outer;
                    }
                }
            }
            out.put_pixel(x, y, Luma([ink]));
        }
    }
    out
}

/// Убирает одиночные пиксели и тонкие белые разрывы.
pub fn clean(bin: &GrayImage) -> GrayImage {
    let opened = morphology::open(bin, Norm::LInf, 1);
    morphology::close(&opened, Norm::LInf, 1)
}

/// Скелетизация (Zhang-Suen). Возвращает растровую маску толщиной 1px.
pub fn thin(bin: &GrayImage) -> GrayImage {
    zhang_suen(bin)
}

fn zhang_suen(img: &GrayImage) -> GrayImage {
    let (w, h) = img.dimensions();
    let wu = w as usize;
    let hu = h as usize;
    let mut cur: Vec<u8> = img.as_raw().iter().map(|&p| (p > 0) as u8).collect();

    if wu < 3 || hu < 3 {
        return img.clone();
    }

    let mut changed = true;
    let mut guard = 0usize;
    while changed && guard < 512 {
        changed = false;
        guard += 1;
        for step in 0..2 {
            let mut del: Vec<usize> = Vec::new();
            for y in 1..hu - 1 {
                for x in 1..wu - 1 {
                    let i = y * wu + x;
                    if cur[i] == 0 {
                        continue;
                    }
                    let p2 = cur[i - wu] as u32;
                    let p3 = cur[i - wu + 1] as u32;
                    let p4 = cur[i + 1] as u32;
                    let p5 = cur[i + wu + 1] as u32;
                    let p6 = cur[i + wu] as u32;
                    let p7 = cur[i + wu - 1] as u32;
                    let p8 = cur[i - 1] as u32;
                    let p9 = cur[i - wu - 1] as u32;
                    let b = p2 + p3 + p4 + p5 + p6 + p7 + p8 + p9;
                    if (2..=6).contains(&b) {
                        let nb = [
                            (p2, p3),
                            (p3, p4),
                            (p4, p5),
                            (p5, p6),
                            (p6, p7),
                            (p7, p8),
                            (p8, p9),
                            (p9, p2),
                        ];
                        let a = nb.iter().filter(|(a, b)| *a == 0 && *b == 1).count() as u32;
                        if a == 1 {
                            let keep = if step == 0 {
                                p2 * p4 * p6 == 0 && p4 * p6 * p8 == 0
                            } else {
                                p2 * p4 * p8 == 0 && p2 * p6 * p8 == 0
                            };
                            if keep {
                                del.push(i);
                            }
                        }
                    }
                }
            }
            if !del.is_empty() {
                changed = true;
                for i in del {
                    cur[i] = 0;
                }
            }
        }
    }

    let mut out = GrayImage::new(w, h);
    for (i, &v) in cur.iter().enumerate() {
        let x = (i % wu) as u32;
        let y = (i / wu) as u32;
        out.put_pixel(x, y, Luma([v * 255]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GrayImage, Luma};

    #[test]
    fn binarize_finds_line() {
        let mut img = GrayImage::from_pixel(40, 40, Luma([250]));
        for x in 5..35 {
            img.put_pixel(x, 20, Luma([20]));
        }
        let b = adaptive_binarize(&gaussian_blur(&img, 1.0), 10, 6);
        assert_eq!(b.get_pixel(20, 20)[0], 255);
        assert_eq!(b.get_pixel(20, 5)[0], 0);
    }

    #[test]
    fn thinning_produces_one_pixel() {
        let mut img = GrayImage::from_pixel(50, 50, Luma([0]));
        for x in 5..45 {
            for y in 23..28 {
                img.put_pixel(x, y, Luma([255]));
            }
        }
        let t = thin(&img);
        let ink = t.as_raw().iter().filter(|&&p| p > 0).count();
        assert!(ink <= 42, "ink={ink}");
        assert!(ink >= 30, "ink={ink}");
    }
}

/// Отделяет мелкие «символьные» компоненты (подписи, отметки, номера) от
/// геометрии чертежа. Возвращает маску текста: её пиксели попадают в OCR и
/// вычищаются из маски для детекции линий.
///
/// Критерий — размер bbox, а не площадь: символ 8×14 px на скане 300 dpi —
/// это шрифт ~3 мм, он в любом случае не часть контура. Длинные тонкие
/// штрихи (пунктир, выноски, рамки) остаются геометрией.
pub fn text_mask(bin: &GrayImage) -> GrayImage {
    let (w, h) = bin.dimensions();
    let mut out = GrayImage::new(w, h);
    let raw = bin.as_raw();
    let mut visited = vec![false; raw.len()];
    let ink = |x: usize, y: usize| raw[y * w as usize + x] > 0;

    for y in 0..h as usize {
        for x in 0..w as usize {
            let i = y * w as usize + x;
            if visited[i] || !ink(x, y) {
                continue;
            }
            // Заливка компоненты 8-связности с ограничением по bbox: если
            // компонента крупная, дальше не исследуем.
            let mut stack = vec![(x, y)];
            visited[i] = true;
            let mut pix: Vec<(usize, usize)> = Vec::new();
            let (mut x0, mut y0, mut x1, mut y1) = (x, y, x, y);
            let mut too_big = false;
            while let Some((cx, cy)) = stack.pop() {
                pix.push((cx, cy));
                if cx < x0 { x0 = cx; }
                if cy < y0 { y0 = cy; }
                if cx > x1 { x1 = cx; }
                if cy > y1 { y1 = cy; }
                if x1 - x0 > 44 || y1 - y0 > 30 {
                    too_big = true;
                }
                for dy in 0..3 {
                    for dx in 0..3 {
                        if dx == 1 && dy == 1 {
                            continue;
                        }
                        let nx = cx as i64 + dx as i64 - 1;
                        let ny = cy as i64 + dy as i64 - 1;
                        if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                            continue;
                        }
                        let (nx, ny) = (nx as usize, ny as usize);
                        let ni = ny * w as usize + nx;
                        if !visited[ni] && ink(nx, ny) {
                            visited[ni] = true;
                            stack.push((nx, ny));
                        }
                    }
                }
            }
            let bw = x1 - x0 + 1;
            let bh = y1 - y0 + 1;
            let area = pix.len();
            // Символ: узкий bbox, умеренная площадь. Штрих 1–2 px высотой и
            // десятки px длиной в bbox не влезает (высота < 4 отсекается отдельно).
            let is_glyph = bw >= 2 && bw <= 44 && bh >= 4 && bh <= 26 && area >= 6;
            if is_glyph && !too_big {
                for (px, py) in pix {
                    out.put_pixel(px as u32, py as u32, Luma([255]));
                }
            }
        }
    }
    out
}

/// Гасит маску текста из бинаризации (линии остаются целыми).
pub fn remove_text(bin: &GrayImage, text: &GrayImage) -> GrayImage {
    let (w, h) = bin.dimensions();
    let mut out = bin.clone();
    for y in 0..h {
        for x in 0..w {
            if text.get_pixel(x, y)[0] > 0 {
                out.put_pixel(x, y, Luma([0]));
            }
        }
    }
    out
}

/// Гасит маску текста в полутоновом изображении (заменяет на белый фон),
/// чтобы градиентный детектор не цеплялся за буквы.
pub fn mask_gray(gray: &GrayImage, text: &GrayImage) -> GrayImage {
    let (w, h) = gray.dimensions();
    let mut out = gray.clone();
    for y in 0..h {
        for x in 0..w {
            if text.get_pixel(x, y)[0] > 0 {
                out.put_pixel(x, y, Luma([255]));
            }
        }
    }
    out
}
