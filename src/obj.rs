//! Экспорт в Wavefront OBJ — формат, который Blender импортирует нативно.
//!
//! Блендер не читает DXF, а `.blend` из Rust не пишется (бинарный и
//! версионный формат). OBJ — практичный мост: линии пишутся элементами `l`,
//! объекты разделены по слоям, координаты в миллиметрах. В Blender:
//! `File → Import → Wavefront (.obj)` и включить «Lines» в импорте.

use crate::geom::Pt;
use crate::params::Params;
use crate::pipeline::TextItem;
use crate::vectorize::VectorData;
use std::fmt::Write as _;

fn f(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    if s.is_empty() || s == "-0" {
        "0".to_string()
    } else {
        s
    }
}

/// Собирает OBJ. Все координаты в мм, ось Y перевёрнута как в CAD.
pub fn to_obj(
    data: &VectorData,
    px: &Params,
    height_px: u32,
    scale: f64,
    texts: &[TextItem],
) -> String {
    let h = height_px as f64;
    let tr = |p: Pt| -> (f64, f64) {
        let y = if px.flip_y { h - p.y } else { p.y };
        (p.x * scale, y * scale)
    };

    let mut out = String::with_capacity(1 << 16);
    let _ = writeln!(out, "# png2dxf OBJ export, units: mm");
    let _ = writeln!(out, "# Blender: File > Import > Wavefront (.obj)");
    let mut vcount = 0usize;

    let mut obj = |out: &mut String, name: &str| {
        let _ = writeln!(out, "o {name}");
    };

    // Линии.
    if !data.segs.is_empty() {
        obj(&mut out, "lines");
        let mut idx = Vec::with_capacity(data.segs.len() * 2);
        for s in &data.segs {
            if s.len() * scale < px.min_line_mm.max(0.0) {
                continue;
            }
            let (x1, y1) = tr(s.a);
            let (x2, y2) = tr(s.b);
            let _ = writeln!(out, "v {} {} 0", f(x1), f(y1));
            let _ = writeln!(out, "v {} {} 0", f(x2), f(y2));
            vcount += 2;
            idx.push((vcount - 1, vcount));
        }
        for (a, b) in idx {
            let _ = writeln!(out, "l {a} {b}");
        }
    }

    // Окружности и дуги — замкнутые/открытые полилинии из точек.
    if !data.circles.is_empty() || !data.arcs.is_empty() {
        obj(&mut out, "arcs");
        for c in &data.circles {
            let (cx, cy) = tr(c.center);
            let r = c.radius * scale;
            let n = (r * 4.0).clamp(24.0, 512.0) as usize;
            let start = vcount + 1;
            for k in 0..n {
                let t = k as f64 / n as f64 * std::f64::consts::TAU;
                let _ = writeln!(out, "v {} {} 0", f(cx + r * t.cos()), f(cy + r * t.sin()));
                vcount += 1;
            }
            let _ = write!(out, "l");
            for k in 0..n {
                let _ = write!(out, " {}", start + k);
            }
            let _ = writeln!(out, " {}", start);
        }
        for a in &data.arcs {
            let (cx, cy) = tr(a.center);
            let r = a.radius * scale;
            let mut delta = a.end_rad - a.start_rad;
            if px.flip_y {
                // При перевороте Y направление обхода меняется.
                delta = -delta;
            }
            while delta < 0.0 {
                delta += std::f64::consts::TAU;
            }
            let n = ((r * delta * 2.0).max(8.0)).min(512.0) as usize;
            let start = vcount + 1;
            for k in 0..=n {
                let t = a.start_rad + delta * (k as f64 / n as f64);
                let ty = if px.flip_y { -t } else { t };
                let _ = writeln!(out, "v {} {} 0", f(cx + r * ty.cos()), f(cy + r * ty.sin()));
                vcount += 1;
            }
            let _ = write!(out, "l");
            for k in 0..=n {
                let _ = write!(out, " {}", start + k);
            }
            out.push('\n');
        }
    }

    // Полилинии (цепочки скелета, не ставшие прямыми).
    if !data.polylines.is_empty() {
        obj(&mut out, "polylines");
        for p in &data.polylines {
            if p.len() < 2 {
                continue;
            }
            let start = vcount + 1;
            for q in p {
                let (x, y) = tr(*q);
                let _ = writeln!(out, "v {} {} 0", f(x), f(y));
                vcount += 1;
            }
            let _ = write!(out, "l");
            for k in 0..p.len() {
                let _ = write!(out, " {}", start + k);
            }
            out.push('\n');
        }
    }

    // Текст в OBJ не переносится: в нём нет текстовых объектов. Надписи
    // выгружаем комментарием, чтобы их не потерять при переносе в Blender.
    if !texts.is_empty() {
        let _ = writeln!(out, "# text annotations ({}):", texts.len());
        for t in texts.iter().take(200) {
            let (x, y) = tr(Pt::new(t.x, t.y + t.h));
            let _ = writeln!(out, "#   [{:.1}, {:.1}] {}", x, y, t.text.replace('\n', " "));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Seg;

    #[test]
    fn writes_lines_and_units() {
        let data = VectorData {
            segs: vec![Seg { a: Pt::new(0.0, 0.0), b: Pt::new(10.0, 0.0), votes: 1 }],
            ..Default::default()
        };
        let px = Params::default();
        let obj = to_obj(&data, &px, 100, 1.0, &[]);
        assert!(obj.contains("v 0 100 0"), "{obj}");
        assert!(obj.contains("l 1 2"), "{obj}");
        assert!(obj.contains("o lines"));
    }
}
