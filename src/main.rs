//! png2dxf — векторизация растровых строительных чертежей в DXF.
//!
//! Режимы:
//!   png2dxf serve          — HTTP API (по умолчанию, если переменная MODE=serve)
//!   png2dxf <in> <out.dxf> — офлайн-конвертация
//!
//! Пайплайн: grayscale → гауссов blur → адаптивная бинаризация →
//! морфологическая чистка → скелетизация (Zhang-Suen) → вероятностное
//! преобразование Хафа (по полосам, параллельно) → обход скелета в цепочки →
//! упрощение Дугласом–Пекером → вписывание окружностей/дуг (Kasa) →
//! нарезка в пересечениях и слияние коллинеарных → DXF R2013 ASCII.

mod cli;
mod dxf;
mod geom;
mod lines;
mod ocr;
mod obj;
mod params;
mod pipeline;
mod preprocess;
mod render;
mod server;
mod synth;
mod vectorize;
mod zip;

use std::io::Write;

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let head = args.first().map(|s| s.as_str()).unwrap_or("");

    if head == "serve" || (head.is_empty() && std::env::var("MODE").as_deref() == Ok("serve")) {
        let cfg = server::Config {
            max_bytes: env_usize("MAX_BODY_MB", 64) * 1024 * 1024,
            max_batch: env_usize("MAX_BATCH", 8),
            workers: env_usize("WORKERS", rayon::current_num_threads()),
        };
        if let Err(e) = server::serve(cfg) {
            eprintln!("не удалось запустить сервер: {e}");
            std::process::exit(1);
        }
        return;
    }

    if head == "gen-test-image" {
        let out = args.get(1).cloned().unwrap_or_else(|| "synthetic.png".into());
        let w: u32 = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(1600);
        let h: u32 = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(1100);
        let (img, gt) = synth::make_sheet(w, h);
        match image::ImageBuffer::<image::Luma<u8>, Vec<u8>>::from_raw(
            img.width(),
            img.height(),
            img.as_raw().clone(),
        )
        .map(|b| b.save(&out))
        .unwrap_or_else(|| Err(image::ImageError::IoError(std::io::Error::other("buffer"))))
        {
            Ok(_) => {
                println!(
                    "{}: {}x{}, эталонных отрезков {}, окружностей {}",
                    out,
                    w,
                    h,
                    gt.segs.len(),
                    gt.circles.len()
                );
            }
            Err(e) => {
                eprintln!("не удалось записать {out}: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    let opts = match cli::parse_args(&args) {
        Ok(o) => o,
        Err(msg) => {
            if msg.contains("Использование") {
                println!("{msg}");
                return;
            }
            eprintln!("{msg}\n");
            eprintln!("{}", cli::HELP);
            std::process::exit(2);
        }
    };
    if let Err(e) = cli::run(&opts) {
        eprintln!("ошибка: {e}");
        std::process::exit(1);
    }
    let _ = std::io::stdout().flush();
}
