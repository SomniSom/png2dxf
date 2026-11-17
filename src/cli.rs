//! CLI-режим: `png2dxf input.png output.dxf [--dpi 300 | --scale x1 y1 x2 y2 mm]`.

use crate::params::Params;
use crate::pipeline;
use std::path::PathBuf;

pub fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut o = Options::default();
    let mut positional = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dpi" => {
                let v = args.get(i + 1).ok_or("--dpi требует значение")?;
                o.params.dpi = Some(v.parse().map_err(|_| "--dpi: не число")?);
                i += 2;
            }
            "--scale" => {
                let vals: Vec<f64> = args
                    .iter()
                    .skip(i + 1)
                    .take(5)
                    .map(|s| s.parse::<f64>())
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| "--scale ожидает: x1 y1 x2 y2 мм")?;
                if vals.len() < 5 {
                    return Err("--scale ожидает: x1 y1 x2 y2 мм".into());
                }
                o.params.scale = Some(crate::params::ScaleParams {
                    px1: [vals[0], vals[1]],
                    px2: [vals[2], vals[3]],
                    mm: vals[4],
                });
                i += 6;
            }
            "--no-thin" => {
                o.params.thin = false;
                i += 1;
            }
            "--ocr-conf" => {
                let v = args.get(i + 1).ok_or("--ocr-conf требует значение")?;
                o.params.ocr_min_conf = v.parse().map_err(|_| "--ocr-conf: не число")?;
                i += 2;
            }
            "--ocr-off" => {
                o.params.ocr = false;
                i += 1;
            }
            "--format" => {
                let v = args.get(i + 1).ok_or("--format требует значение")?;
                let v = v.trim().to_ascii_lowercase();
                if !matches!(v.as_str(), "dxf" | "svg" | "obj") {
                    return Err("--format: допустимо dxf, svg или obj".into());
                }
                o.format = v;
                i += 2;
            }
            "--drop-text" => {
                o.params.drop_text = true;
                i += 1;
            }
            "--no-color-boost" => {
                o.params.color_boost = false;
                i += 1;
            }
            "--debug" => {
                o.params.include_debug = true;
                i += 1;
            }
            "--min-line-mm" => {
                let v = args.get(i + 1).ok_or("--min-line-mm требует значение")?;
                o.params.min_line_mm = v.parse().map_err(|_| "--min-line-mm: не число")?;
                i += 2;
            }
            "--angle-tol" => {
                let v = args.get(i + 1).ok_or("--angle-tol требует значение")?;
                o.params.angle_tolerance_deg = v.parse().map_err(|_| "--angle-tol: не число")?;
                i += 2;
            }
            "--epsilon" => {
                let v = args.get(i + 1).ok_or("--epsilon требует значение")?;
                o.params.simplify_epsilon_px = v.parse().map_err(|_| "--epsilon: не число")?;
                i += 2;
            }
            "-h" | "--help" => return Err(HELP.into()),
            other if other.starts_with('-') => return Err(format!("неизвестный флаг: {other}\n{HELP}")),
            _ => {
                positional.push(args[i].clone());
                i += 1;
            }
        }
    }
    o.params.normalize();
    let input = positional.first().ok_or("не указан входной файл")?;
    o.input = PathBuf::from(input);
    o.output = PathBuf::from(positional.get(1).cloned().unwrap_or_else(|| "output.dxf".into()));
    Ok(o)
}

pub const HELP: &str = "\
png2dxf — векторизация растровых чертежей в DXF

Использование:
  png2dxf <вход.png|jpg|tiff> <выход.dxf> [опции]
  png2dxf serve          — запустить HTTP API
  png2dxf initdb         — создать пустой справочник заданий (не требуется)

Опции:
  --dpi <число>                     масштаб по DPI скана (по умолчанию 300)
  --scale <x1> <y1> <x2> <y2> <мм>  масштаб по известной размерной линии
  --min-line-mm <мм>                отбросить линии короче
  --angle-tol <град>                допуск на параллельность при слиянии
  --epsilon <px>                    допуск упрощения полилиний
  --no-thin                         не скелетизировать (толстые исходники)
  --ocr-conf <0..100>               порог уверенности OCR (по умолчанию 35)
  --ocr-off                         не запускать OCR
  --drop-text                       вычищать подписи перед векторизацией
  --no-color-boost                  не усиливать цветные линии
  --format <dxf|svg|obj>            формат вывода (по умолчанию dxf)
  --debug                           добавить слой 'Каркас' со скелетом
  -h, --help                        эта справка
";

#[derive(Debug)]
pub struct Options {
    pub input: PathBuf,
    pub output: PathBuf,
    pub params: Params,
    /// Формат вывода: dxf | svg | obj
    pub format: String,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            input: PathBuf::from("in.png"),
            output: PathBuf::from("output.dxf"),
            params: Params::default(),
            format: "dxf".to_string(),
        }
    }
}

pub fn run(opts: &Options) -> Result<(), String> {
    let bytes = std::fs::read(&opts.input)
        .map_err(|e| format!("не удалось прочитать {}: {e}", opts.input.display()))?;
    let out = pipeline::vectorize_bytes(&bytes, &opts.params).map_err(|e| e.message)?;
    let format = if opts.format != "dxf" {
        opts.format.clone()
    } else {
        // Явно не задан — смотрим на расширение: out.svg / out.obj.
        match opts
            .output
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref()
        {
            Some("svg") => "svg".to_string(),
            Some("obj") => "obj".to_string(),
            _ => "dxf".to_string(),
        }
    };
    let body = match format.as_str() {
        "svg" => &out.svg,
        "obj" => &out.obj,
        _ => &out.dxf,
    };
    std::fs::write(&opts.output, body)
        .map_err(|e| format!("не удалось записать {}: {e}", opts.output.display()))?;
    println!(
        "{} → {} ({}): {} линий, {} полилиний, {} дуг, {} окружностей, масштаб {:.6} мм/px, {} мс",
        opts.input.display(),
        opts.output.display(),
        format.to_ascii_uppercase(),
        out.stats.lines,
        out.stats.polylines,
        out.stats.arcs,
        out.stats.circles,
        out.stats.scale_mm_per_px,
        out.elapsed_ms
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scale() {
        let args: Vec<String> = ["in.png", "out.dxf", "--scale", "1", "2", "3", "4", "500"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let o = parse_args(&args).unwrap();
        assert_eq!(o.params.scale.as_ref().unwrap().mm, 500.0);
        assert_eq!(o.output, PathBuf::from("out.dxf"));
    }

    #[test]
    fn rejects_bad_flag() {
        let args: Vec<String> = ["--nope"].iter().map(|s| s.to_string()).collect();
        assert!(parse_args(&args).is_err());
    }
}
