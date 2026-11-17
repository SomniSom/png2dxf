//! Сборка итоговой геометрии и запись DXF: применение масштаба,
//! переворот оси Y (CAD: Y вверх, растр: Y вниз) и раскладка по слоям.

use crate::dxf::{DxfWriter, LayerDef};
use crate::geom::Pt;
use crate::params::{Layers, Params, Stats};
use crate::vectorize::{Arc, Circle, VectorData};

/// Переводит пиксельные координаты в миллиметры и, если нужно, переворачивает Y.
#[derive(Clone, Copy)]
pub struct Transform {
    pub scale: f64,
    pub height: f64,
    pub flip_y: bool,
}

impl Transform {
    pub fn apply(&self, p: Pt) -> Pt {
        let y = if self.flip_y { self.height - p.y } else { p.y };
        Pt::new(p.x * self.scale, y * self.scale)
    }

    pub fn line(&self, a: Pt, b: Pt) -> (Pt, Pt) {
        (self.apply(a), self.apply(b))
    }

    /// Углы дуги в системе CAD (радианы, CCW от +X). При перевороте Y
    /// направление обхода меняется — углы отражаются и меняются местами.
    pub fn arc_angles(&self, a0: f64, a1: f64) -> (f64, f64) {
        if self.flip_y {
            let (na0, na1) = (-a1, -a0);
            (na0, na1)
        } else {
            (a0, a1)
        }
    }
}

pub struct Built {
    pub dxf: String,
    pub svg: String,
    pub stats: Stats,
}

/// Надпись, которую нужно положить в DXF слоем TEXT.
pub struct TextOut {
    pub text: String,
    pub x_px: f64,
    pub y_px: f64,
    pub h_px: f64,
}

pub fn build(
    data: &VectorData,
    px: &Params,
    width_px: u32,
    height_px: u32,
    scale: f64,
    texts: &[TextOut],
) -> Built {
    let t = Transform { scale, height: height_px as f64, flip_y: px.flip_y };
    let layers: &Layers = &px.layers;
    let defs = [
        LayerDef { name: "0", color: 7, linetype: "CONTINUOUS" },
        LayerDef { name: "Линии", color: 7, linetype: "CONTINUOUS" },
        LayerDef { name: "Окружности", color: 1, linetype: "CONTINUOUS" },
        LayerDef { name: "Дуги", color: 3, linetype: "CONTINUOUS" },
        LayerDef { name: "Полилинии", color: 5, linetype: "CONTINUOUS" },
        LayerDef { name: "Текст", color: 2, linetype: "CONTINUOUS" },
        LayerDef { name: "Каркас", color: 8, linetype: "DASHED" },
    ];
    let mut w = DxfWriter::new();
    w.begin(&defs);

    let min_len_mm = px.min_line_mm.max(0.0);
    let mut written_lines = 0usize;
    for s in &data.segs {
        let (a, b) = t.line(s.a, s.b);
        let len = a.dist(&b);
        if len < min_len_mm {
            continue;
        }
        let layer = if layers.lines.is_empty() { "0" } else { layers.lines.as_str() };
        w.line(layer, a, b);
        written_lines += 1;
    }

    let mut written_circles = 0usize;
    for c in &data.circles {
        let layer = if layers.circles.is_empty() { "0" } else { layers.circles.as_str() };
        w.circle(
            layer,
            Circle { center: t.apply(c.center), radius: c.radius * scale, layer_circle: true },
        );
        written_circles += 1;
    }

    let mut written_arcs = 0usize;
    for a in &data.arcs {
        let layer = if layers.arcs.is_empty() { "0" } else { layers.arcs.as_str() };
        let (s0, s1) = t.arc_angles(a.start_rad, a.end_rad);
        w.arc(
            layer,
            Arc { center: t.apply(a.center), radius: a.radius * scale, start_rad: s0, end_rad: s1 },
        );
        written_arcs += 1;
    }

    let mut written_polys = 0usize;
    for p in &data.polylines {
        if p.len() < 2 {
            continue;
        }
        let layer = if layers.polylines.is_empty() { "0" } else { layers.polylines.as_str() };
        let pts: Vec<Pt> = p.iter().map(|q| t.apply(*q)).collect();
        w.polyline(layer, &pts, false);
        written_polys += 1;
    }

    // Надписи: координаты bbox от OCR переводятся в мм и переворачиваются.
    for item in texts {
        if item.text.trim().is_empty() {
            continue;
        }
        let base_px = Pt::new(item.x_px, item.y_px + item.h_px);
        let p = t.apply(base_px);
        let height = (item.h_px * scale).max(0.5);
        w.text("Текст", p.x, p.y, height, &item.text, 0.0);
    }

    if px.include_debug {
        for ch in &data.skeleton {
            let pts: Vec<Pt> = ch.iter().map(|q| t.apply(*q)).collect();
            w.skeleton_polyline("Каркас", &pts);
        }
    }

    // SVG: та же геометрия в миллиметрах, но с осью Y вниз (как в растре)
    // и шрифтом без проблем кодеков — текст читается в любом браузере.
    let svg = to_svg(data, px, &t, texts);

    let stats = Stats {
        width_px,
        height_px,
        scale_mm_per_px: scale,
        lines: written_lines,
        polylines: written_polys,
        arcs: written_arcs,
        circles: written_circles,
        dropped_short: data.dropped_short,
        chain_points: data.chain_points,
    };
    Built { dxf: w.finish(), svg, stats }
}

/// SVG-вывод: та же геометрия, но с осью Y вниз (как в растре) и TTF-шрифтом.
///
/// SVG не зависит от кодеков DXF и шрифтовых файлов CAD — открывается
/// в браузере и в Inkscape без настроек, кириллица рендерится системным шрифтом.
fn to_svg(data: &VectorData, px: &Params, t: &Transform, texts: &[TextOut]) -> String {
    use std::fmt::Write as _;
    let height_mm = if px.flip_y { t.height * t.scale } else { t.height * t.scale };
    let width_mm = pick_width(data, px, t);
    let mut out = String::with_capacity(1 << 16);
    let _ = writeln!(
        out,
        r#"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="{w}mm" height="{h}mm" viewBox="0 0 {w} {h}">
<g fill="none" stroke-linecap="round" font-family="Liberation Sans, Arial, sans-serif">"#,
        w = fmt(width_mm),
        h = fmt(height_mm)
    );

    // В SVG ось Y вниз: координаты берём без переворота (как в растре), тогда
    // картинка совпадает с исходным сканом.
    let sy = |p: Pt| -> (f64, f64) { (p.x * t.scale, p.y * t.scale) };

    let _ = writeln!(out, r##"<g stroke="#222" stroke-width="0.18">"##);
    let min_len_mm = px.min_line_mm.max(0.0);
    for s in &data.segs {
        if s.len() * t.scale < min_len_mm {
            continue;
        }
        let (x1, y1) = sy(s.a);
        let (x2, y2) = sy(s.b);
        let _ = writeln!(
            out,
            r#"<line x1="{}" y1="{}" x2="{}" y2="{}"/>"#,
            fmt(x1),
            fmt(y1),
            fmt(x2),
            fmt(y2)
        );
    }
    let _ = writeln!(out, "</g>");

    if !data.circles.is_empty() {
        let _ = writeln!(out, r##"<g stroke="#c22" stroke-width="0.18">"##);
        for c in &data.circles {
            let (cx, cy) = sy(c.center);
            let _ = writeln!(
                out,
                r#"<circle cx="{}" cy="{}" r="{}"/>"#,
                fmt(cx),
                fmt(cy),
                fmt(c.radius * t.scale)
            );
        }
        let _ = writeln!(out, "</g>");
    }

    if !data.arcs.is_empty() {
        let _ = writeln!(out, r##"<g stroke="#2a7" stroke-width="0.18">"##);
        for a in &data.arcs {
            let (cx, cy) = sy(a.center);
            let r = a.radius * t.scale;
            // SVG-углы: 0° вправо, по часовой стрелке (ось Y вниз).
            let (x1, y1) = (cx + r * a.start_rad.cos(), cy + r * a.start_rad.sin());
            let (x2, y2) = (cx + r * a.end_rad.cos(), cy + r * a.end_rad.sin());
            // Развёртка дуги без переворота Y противоположна DXF-углам.
            let mut delta = a.end_rad - a.start_rad;
            while delta > std::f64::consts::TAU {
                delta -= std::f64::consts::TAU;
            }
            while delta < 0.0 {
                delta += std::f64::consts::TAU;
            }
            let large = if delta > std::f64::consts::PI { 1 } else { 0 };
            let _ = writeln!(
                out,
                r#"<path d="M {} {} A {} {} 0 {} 1 {} {}"/>"#,
                fmt(x1),
                fmt(y1),
                fmt(r),
                fmt(r),
                large,
                fmt(x2),
                fmt(y2)
            );
        }
        let _ = writeln!(out, "</g>");
    }

    if !data.polylines.is_empty() {
        let _ = writeln!(out, r##"<g stroke="#35c" stroke-width="0.18">"##);
        for p in &data.polylines {
            if p.len() < 2 {
                continue;
            }
            let _ = write!(out, r#"<polyline points=""#);
            for (i, q) in p.iter().enumerate() {
                let (x, y) = sy(*q);
                if i > 0 {
                    out.push(' ');
                }
                let _ = write!(out, "{},{}", fmt(x), fmt(y));
            }
            let _ = writeln!(out, r#""/>"#);
        }
        let _ = writeln!(out, "</g>");
    }

    if !texts.is_empty() {
        let _ = writeln!(
            out,
            r##"<g fill="#080" stroke="none" font-size="{:.1}">"##,
            (texts.first().map(|x| x.h_px).unwrap_or(3.0) * t.scale * 0.9).max(0.8)
        );
        for item in texts {
            if item.text.trim().is_empty() {
                continue;
            }
            // В SVG ось Y вниз, поэтому базовая линия — низ bbox.
            let x = item.x_px * t.scale;
            let y = (item.y_px + item.h_px * 0.85) * t.scale;
            let _ = writeln!(
                out,
                r#"<text x="{}" y="{}">{}</text>"#,
                fmt(x),
                fmt(y),
                escape_xml(&item.text)
            );
        }
        let _ = writeln!(out, "</g>");
    }

    let _ = writeln!(out, "</g>\n</svg>");
    out
}

fn pick_width(data: &VectorData, px: &Params, t: &Transform) -> f64 {
    let mut max_x: f64 = 0.0;
    for s in &data.segs {
        max_x = max_x.max(s.a.x).max(s.b.x);
    }
    for c in &data.circles {
        max_x = max_x.max(c.center.x + c.radius);
    }
    let _ = px;
    if max_x > 0.0 {
        (max_x + 2.0) * t.scale
    } else {
        t.height * t.scale
    }
}

fn fmt(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    if s.is_empty() || s == "-0" {
        "0".to_string()
    } else {
        s
    }
}

fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_and_flip() {
        let t = Transform { scale: 0.5, height: 100.0, flip_y: true };
        let p = t.apply(Pt::new(10.0, 10.0));
        assert!((p.x - 5.0).abs() < 1e-9);
        assert!((p.y - 45.0).abs() < 1e-9);
    }

    #[test]
    fn arc_mirrored_when_flipped() {
        let t = Transform { scale: 1.0, height: 100.0, flip_y: true };
        let (a0, a1) = t.arc_angles(0.0, std::f64::consts::FRAC_PI_2);
        assert!((a0 + std::f64::consts::FRAC_PI_2).abs() < 1e-9);
        assert!(a1.abs() < 1e-9);
    }
}
