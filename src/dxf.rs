//! Запись DXF R2013 (AC1027) в ASCII. Без внешних зависимостей —
//! формат групповых кодов прост, а полный контроль даёт предсказуемый файл.

use crate::geom::{Pt, Seg};
use crate::vectorize::{Arc, Circle};
use std::fmt::Write as _;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayerDef {
    pub name: &'static str,
    pub color: i16,
    pub linetype: &'static str,
}

pub struct DxfWriter {
    buf: String,
    layer_count: usize,
}

fn code(out: &mut String, group: i32, value: &str) {
    let _ = writeln!(out, "{group}");
    let _ = writeln!(out, "{value}");
}

fn num(v: f64) -> String {
    // DXF не любит экспоненту; 6 знаков после точки — микроны в мм.
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    if s == "-0" || s.is_empty() {
        "0".to_string()
    } else {
        s
    }
}

impl DxfWriter {
    pub fn new() -> Self {
        Self { buf: String::with_capacity(1 << 20), layer_count: 0 }
    }

    pub fn begin(&mut self, layers: &[LayerDef]) {
        self.layer_count = layers.len();

        code(&mut self.buf, 999, "png2dxf R2013 — raster→vector converter");
        // HEADER
        code(&mut self.buf, 0, "SECTION");
        code(&mut self.buf, 2, "HEADER");
        code(&mut self.buf, 9, "$ACADVER");
        code(&mut self.buf, 1, "AC1027");
        code(&mut self.buf, 9, "$INSBASE");
        code(&mut self.buf, 10, "0.0");
        code(&mut self.buf, 20, "0.0");
        code(&mut self.buf, 30, "0.0");
        code(&mut self.buf, 9, "$MEASUREMENT");
        code(&mut self.buf, 70, "1");
        code(&mut self.buf, 9, "$INSUNITS");
        code(&mut self.buf, 70, "4");
        code(&mut self.buf, 0, "ENDSEC");

        // TABLES: LTYPE + LAYER
        code(&mut self.buf, 0, "SECTION");
        code(&mut self.buf, 2, "TABLES");

        code(&mut self.buf, 0, "TABLE");
        code(&mut self.buf, 2, "LTYPE");
        code(&mut self.buf, 70, "2");
        for (name, descr) in [("CONTINUOUS", "Solid line"), ("DASHED", "Dashed __ __ __")] {
            code(&mut self.buf, 0, "LTYPE");
            code(&mut self.buf, 2, name);
            code(&mut self.buf, 70, "0");
            code(&mut self.buf, 3, descr);
            code(&mut self.buf, 72, "65");
            code(&mut self.buf, 73, "2");
            code(&mut self.buf, 40, "15.0");
            code(&mut self.buf, 49, "10.0");
            code(&mut self.buf, 74, "0");
            code(&mut self.buf, 49, "-5.0");
            code(&mut self.buf, 74, "0");
        }
        code(&mut self.buf, 0, "ENDTAB");

        code(&mut self.buf, 0, "TABLE");
        code(&mut self.buf, 2, "LAYER");
        code(&mut self.buf, 70, &layers.len().max(1).to_string());
        for l in layers {
            code(&mut self.buf, 0, "LAYER");
            code(&mut self.buf, 2, l.name);
            code(&mut self.buf, 70, "0");
            code(&mut self.buf, 62, &l.color.to_string());
            code(&mut self.buf, 6, l.linetype);
        }
        code(&mut self.buf, 0, "ENDTAB");
        code(&mut self.buf, 0, "ENDSEC");

        code(&mut self.buf, 0, "SECTION");
        code(&mut self.buf, 2, "BLOCKS");
        code(&mut self.buf, 0, "ENDSEC");

        code(&mut self.buf, 0, "SECTION");
        code(&mut self.buf, 2, "ENTITIES");
    }

    pub fn line(&mut self, layer: &str, a: Pt, b: Pt) {
        code(&mut self.buf, 0, "LINE");
        code(&mut self.buf, 8, layer);
        code(&mut self.buf, 10, &num(a.x));
        code(&mut self.buf, 20, &num(a.y));
        code(&mut self.buf, 30, "0");
        code(&mut self.buf, 11, &num(b.x));
        code(&mut self.buf, 21, &num(b.y));
        code(&mut self.buf, 31, "0");
    }

    /// Однострочный текст плана (надпись на чертеже).
    pub fn text(&mut self, layer: &str, x: f64, y: f64, height: f64, value: &str, rotation_deg: f64) {
        code(&mut self.buf, 0, "TEXT");
        code(&mut self.buf, 8, layer);
        code(&mut self.buf, 10, &num(x));
        code(&mut self.buf, 20, &num(y));
        code(&mut self.buf, 30, "0");
        code(&mut self.buf, 40, &num(height.max(0.1)));
        code(&mut self.buf, 1, value);
        code(&mut self.buf, 7, "Standard");
        if rotation_deg.abs() > 1e-9 {
            code(&mut self.buf, 50, &num(rotation_deg.rem_euclid(360.0)));
        }
        code(&mut self.buf, 72, "0");
        code(&mut self.buf, 11, &num(x));
        code(&mut self.buf, 21, &num(y));
        code(&mut self.buf, 31, "0");
    }

    pub fn circle(&mut self, layer: &str, c: Circle) {
        code(&mut self.buf, 0, "CIRCLE");
        code(&mut self.buf, 8, layer);
        code(&mut self.buf, 10, &num(c.center.x));
        code(&mut self.buf, 20, &num(c.center.y));
        code(&mut self.buf, 30, "0");
        code(&mut self.buf, 40, &num(c.radius));
    }

    pub fn arc(&mut self, layer: &str, a: Arc) {
        code(&mut self.buf, 0, "ARC");
        code(&mut self.buf, 8, layer);
        code(&mut self.buf, 10, &num(a.center.x));
        code(&mut self.buf, 20, &num(a.center.y));
        code(&mut self.buf, 30, "0");
        code(&mut self.buf, 40, &num(a.radius));
        code(&mut self.buf, 50, &num(a.start_rad.to_degrees().rem_euclid(360.0)));
        code(&mut self.buf, 51, &num(a.end_rad.to_degrees().rem_euclid(360.0)));
    }

    pub fn polyline(&mut self, layer: &str, pts: &[Pt], closed: bool) {
        if pts.len() < 2 {
            return;
        }
        code(&mut self.buf, 0, "LWPOLYLINE");
        code(&mut self.buf, 8, layer);
        code(&mut self.buf, 90, &pts.len().to_string());
        code(&mut self.buf, 70, if closed { "1" } else { "0" });
        for p in pts {
            code(&mut self.buf, 10, &num(p.x));
            code(&mut self.buf, 20, &num(p.y));
        }
    }

    /// Черновая полилиния, собранная со скелета (слой отладки).
    pub fn skeleton_polyline(&mut self, layer: &str, pts: &[Pt]) {
        self.polyline(layer, pts, false);
    }

    pub fn finish(mut self) -> String {
        code(&mut self.buf, 0, "ENDSEC");
        code(&mut self.buf, 0, "EOF");
        self.buf
    }

    pub fn layer_count(&self) -> usize {
        self.layer_count
    }
}

impl Default for DxfWriter {
    fn default() -> Self {
        Self::new()
    }
}

/// Общие данные для записи: всё уже в миллиметрах и с перевёрнутой осью Y.
pub struct Drawn {
    pub segs: Vec<Seg>,
    pub arcs: Vec<Arc>,
    pub circles: Vec<Circle>,
    pub polylines: Vec<Vec<Pt>>,
    pub skeleton: Vec<Vec<Pt>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_valid_structure() {
        let mut w = DxfWriter::new();
        w.begin(&[
            LayerDef { name: "Линии", color: 7, linetype: "CONTINUOUS" },
            LayerDef { name: "Окружности", color: 1, linetype: "CONTINUOUS" },
        ]);
        w.line("Линии", Pt::new(0.0, 0.0), Pt::new(100.5, 20.0));
        w.circle("Окружности", Circle { center: Pt::new(50.0, 50.0), radius: 10.0, layer_circle: true });
        let s = w.finish();
        assert!(s.contains("AC1027"));
        assert!(s.ends_with("EOF\n"));
        // Каждый групповой код идёт парами: чётное число строк.
        assert_eq!(s.lines().count() % 2, 0);
        assert!(s.contains("Окружности"));
    }

    #[test]
    fn num_has_no_exponent() {
        assert_eq!(num(1e-7), "0");
        assert_eq!(num(-0.0), "0");
        assert!(!num(12345678.9).contains('e'));
    }
}
