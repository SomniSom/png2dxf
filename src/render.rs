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
    Built { dxf: w.finish(), stats }
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
