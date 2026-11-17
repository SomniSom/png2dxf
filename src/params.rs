use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ScaleParams {
    pub px1: [f64; 2],
    pub px2: [f64; 2],
    pub mm: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Params {
    pub scale: Option<ScaleParams>,
    pub dpi: Option<f64>,
    pub blur_sigma: f32,
    pub adaptive_radius: u32,
    pub adaptive_offset: i32,
    pub thin: bool,
    pub hough_angle_step_deg: f32,
    pub hough_rho_step: f32,
    pub hough_threshold_ratio: f32,
    pub hough_threshold_min: u32,
    pub hough_band_px: f32,
    pub lsd_threshold: f32,
    pub lsd_min_len_px: f32,
    pub drop_text: bool,
    pub color_boost: bool,
    pub ocr: bool,
    pub ocr_lang: String,
    pub ocr_min_conf: f32,
    pub ocr_upscale: u32,
    pub min_line_px: f32,
    pub max_gap_px: f32,
    pub angle_tolerance_deg: f32,
    pub min_line_mm: f64,
    pub min_arc_points: usize,
    pub arc_tolerance_px: f64,
    pub simplify_epsilon_px: f64,
    pub short_stroke_px: f32,
    pub stroke_gap_px: f32,
    pub layers: Layers,
    pub max_pixels: u64,
    pub flip_y: bool,
    pub include_debug: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Layers {
    pub lines: String,
    pub circles: String,
    pub arcs: String,
    pub polylines: String,
}

impl Default for Layers {
    fn default() -> Self {
        Self {
            lines: "Линии".into(),
            circles: "Окружности".into(),
            arcs: "Дуги".into(),
            polylines: "Полилинии".into(),
        }
    }
}

impl Default for Params {
    fn default() -> Self {
        Self {
            scale: None,
            // Масштаб не угадываем: для смет «примерно 300 dpi» — это ошибка
            // в разы. Пользователь передаёт scale (по размерной линии) или dpi.
            dpi: None,
            blur_sigma: 1.2,
            adaptive_radius: 12,
            adaptive_offset: 8,
            thin: true,
            hough_angle_step_deg: 0.5,
            hough_rho_step: 1.0,
            hough_threshold_ratio: 0.22,
            hough_threshold_min: 25,
            hough_band_px: 2.5,
            lsd_threshold: 30.0,
            lsd_min_len_px: 25.0,
            drop_text: false,
            color_boost: true,
            ocr: true,
            ocr_lang: "rus".to_string(),
            // Агрессивный режим по умолчанию: полнота важнее чистоты.
            ocr_min_conf: 35.0,
            ocr_upscale: 2,
            min_line_px: 15.0,
            max_gap_px: 6.0,
            angle_tolerance_deg: 1.2,
            min_line_mm: 0.0,
            min_arc_points: 24,
            arc_tolerance_px: 2.0,
            simplify_epsilon_px: 0.8,
            short_stroke_px: 4.0,
            stroke_gap_px: 3.5,
            layers: Layers::default(),
            max_pixels: 120_000_000,
            flip_y: true,
            include_debug: false,
        }
    }
}

impl Params {
    pub fn normalize(&mut self) {
        self.blur_sigma = self.blur_sigma.clamp(0.0, 10.0);
        self.adaptive_radius = self.adaptive_radius.clamp(2, 200);
        self.adaptive_offset = self.adaptive_offset.clamp(-50, 50);
        self.hough_angle_step_deg = self.hough_angle_step_deg.clamp(0.1, 5.0);
        self.hough_rho_step = self.hough_rho_step.clamp(0.5, 4.0);
        self.hough_threshold_ratio = self.hough_threshold_ratio.clamp(0.02, 1.0);
        self.hough_threshold_min = self.hough_threshold_min.clamp(3, 10_000);
        self.hough_band_px = self.hough_band_px.clamp(0.7, 10.0);
        self.ocr_min_conf = self.ocr_min_conf.clamp(0.0, 100.0);
        self.ocr_upscale = self.ocr_upscale.clamp(1, 4);
        self.lsd_threshold = self.lsd_threshold.clamp(5.0, 400.0);
        self.lsd_min_len_px = self.lsd_min_len_px.clamp(5.0, 1000.0);
        self.min_line_px = self.min_line_px.max(1.0);
        self.max_gap_px = self.max_gap_px.max(0.0);
        self.angle_tolerance_deg = self.angle_tolerance_deg.clamp(0.1, 15.0);
        self.min_arc_points = self.min_arc_points.clamp(5, 100_000);
        self.arc_tolerance_px = self.arc_tolerance_px.clamp(0.3, 20.0);
        self.simplify_epsilon_px = self.simplify_epsilon_px.clamp(0.0, 20.0);
        self.short_stroke_px = self.short_stroke_px.max(0.0);
        self.stroke_gap_px = self.stroke_gap_px.max(0.0);
        self.max_pixels = self.max_pixels.clamp(1_000_000, 400_000_000);
        if let Some(s) = &self.scale {
            if s.mm <= 0.0 {
                self.scale = None;
            }
        }
        if let Some(d) = self.dpi {
            if !(1.0..=10_000.0).contains(&d) {
                self.dpi = None;
            }
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Stats {
    pub width_px: u32,
    pub height_px: u32,
    pub scale_mm_per_px: f64,
    pub lines: usize,
    pub polylines: usize,
    pub arcs: usize,
    pub circles: usize,
    pub dropped_short: usize,
    pub chain_points: usize,
}

impl Stats {
    pub fn entities(&self) -> usize {
        self.lines + self.polylines + self.arcs + self.circles
    }
}
