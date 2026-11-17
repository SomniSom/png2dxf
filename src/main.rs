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
mod params;
mod pipeline;
mod preprocess;
mod render;
mod server;
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
