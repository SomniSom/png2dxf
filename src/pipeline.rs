//! Связка: байты изображения → предобработка → векторизация → DXF.

use crate::params::{Params, Stats};
use crate::vectorize::VectorData;
use image::ImageReader;
use std::io::Cursor;
use std::time::Instant;

pub struct Output {
    pub dxf: String,
    pub stats: Stats,
    pub elapsed_ms: u64,
    /// Надписи, найденные OCR-движком (если он доступен), в координатах растра.
    pub texts: Vec<TextItem>,
}

#[derive(Debug, Clone)]
pub struct TextItem {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ErrorKind {
    BadImage,
    TooLarge,
    NoGeometry,
    Internal,
}

#[derive(Debug)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
}

impl Error {
    fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into() }
    }
}

/// Масштаб: явная размерная линия приоритетнее DPI.
pub fn resolve_scale(px: &Params) -> Result<f64, Error> {
    if let Some(s) = &px.scale {
        let dpx = ((s.px2[0] - s.px1[0]).powi(2) + (s.px2[1] - s.px1[1]).powi(2)).sqrt();
        if dpx < 1e-6 {
            return Err(Error::new(ErrorKind::BadImage, "точки масштаба совпадают"));
        }
        if s.mm <= 0.0 {
            return Err(Error::new(ErrorKind::BadImage, "длина в мм должна быть > 0"));
        }
        return Ok(s.mm / dpx);
    }
    if let Some(dpi) = px.dpi {
        if dpi > 0.0 {
            return Ok(25.4 / dpi);
        }
    }
    Err(Error::new(
        ErrorKind::BadImage,
        "не задан масштаб: укажите scale (px1, px2, mm) или dpi",
    ))
}

pub fn vectorize_bytes(bytes: &[u8], params: &Params) -> Result<Output, Error> {
    let started = Instant::now();
    if bytes.is_empty() {
        return Err(Error::new(ErrorKind::BadImage, "пустой файл"));
    }
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| Error::new(ErrorKind::BadImage, format!("не удалось определить формат: {e}")))?;
    let img = reader
        .decode()
        .map_err(|e| Error::new(ErrorKind::BadImage, format!("не удалось декодировать изображение: {e}")))?;

    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return Err(Error::new(ErrorKind::BadImage, "нулевой размер изображения"));
    }
    if (w as u64) * (h as u64) > params.max_pixels {
        return Err(Error::new(
            ErrorKind::TooLarge,
            format!(
                "изображение {w}×{h} = {} Мпикс превышает лимит {} Мпикс",
                (w as u64 * h as u64) / 1_000_000,
                params.max_pixels / 1_000_000
            ),
        ));
    }
    let scale = resolve_scale(params)?;

    let t0 = Instant::now();
    let gray = if params.color_boost {
        crate::preprocess::grayscale_colored(&img.to_rgba8())
    } else {
        crate::preprocess::grayscale(&img.to_rgba8())
    };
    let blurred = crate::preprocess::gaussian_blur(&gray, params.blur_sigma);
    let bin = crate::preprocess::adaptive_binarize(&blurred, params.adaptive_radius, params.adaptive_offset);
    let cleaned = crate::preprocess::clean(&bin);
    // Текст убираем до векторизации: подписи и отметки — не геометрия.
    // Маска текста вычисляется всегда: она нужна и для OCR, и как гарантия,
    // что подписи не «просачиваются» в геометрию. Удаление из геометрии —
    // отдельное решение (drop_text), по умолчанию выключено.
    let text = crate::preprocess::text_mask(&cleaned);
    let geom = if params.drop_text {
        crate::preprocess::remove_text(&cleaned, &text)
    } else {
        cleaned.clone()
    };
    // Полутон для LSD-прохода: гасим текстовые пиксели в сером (белым),
    // а не просто отдаём исходный blur. Так детектор по градиентам не «грызёт»
    // буквы, а работает по контурам чертежа.
    let gray_clean = if params.drop_text {
        crate::preprocess::mask_gray(&blurred, &text)
    } else {
        blurred.clone()
    };
    if params.drop_text {
        eprintln!(
            "text mask: {} px ({:.1}% чернил)",
            text.as_raw().iter().filter(|&&p| p > 0).count(),
            100.0 * text.as_raw().iter().filter(|&&p| p > 0).count() as f64
                / cleaned.as_raw().iter().filter(|&&p| p > 0).count().max(1) as f64
        );
    }
    let edges = if params.thin {
        crate::preprocess::thin(&geom)
    } else {
        geom
    };
    eprintln!(
        "preprocess {}x{} in {} ms",
        w,
        h,
        t0.elapsed().as_millis()
    );

    let t1 = Instant::now();
    let mut data: VectorData = crate::vectorize::vectorize(&edges, Some(&gray_clean), params);
    eprintln!(
        "vectorize: {} segs, {} arcs, {} circles in {} ms",
        data.segs.len(),
        data.arcs.len(),
        data.circles.len(),
        t1.elapsed().as_millis()
    );

    if params.include_debug {
        data.skeleton = crate::vectorize::trace_chains(&edges, 4_000.0);
    }

    // OCR считаем до сборки DXF, чтобы надписи попали в слой «Текст».
    let t2 = Instant::now();
    let texts = if params.ocr {
        crate::ocr::recognize(
            &gray,
            Some(&text),
            &params.ocr_lang,
            params.ocr_min_conf,
            params.ocr_upscale,
        )
    } else {
        Vec::new()
    };
    eprintln!(
        "ocr: {} надписей{} за {} ms",
        texts.len(),
        if crate::ocr::available() { "" } else { " (tesseract не найден)" },
        t2.elapsed().as_millis()
    );
    let text_out: Vec<crate::render::TextOut> = texts
        .iter()
        .map(|t| crate::render::TextOut {
            text: t.text.clone(),
            x_px: t.x,
            y_px: t.y,
            h_px: t.h,
        })
        .collect();

    let built = crate::render::build(&data, params, w, h, scale, &text_out);
    if built.stats.entities() == 0 {
        return Err(Error::new(
            ErrorKind::NoGeometry,
            "на изображении не найдено ни одной линии — проверьте пороги бинаризации и Хафа",
        ));
    }
    Ok(Output {
        dxf: built.dxf,
        stats: built.stats,
        elapsed_ms: started.elapsed().as_millis() as u64,
        texts,
    })
}
