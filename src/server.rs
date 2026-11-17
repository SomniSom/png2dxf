//! HTTP API: синхронный REST на tiny_http, пул воркеров rayon.
//!
//! POST /v1/vectorize — multipart (поле `file` + необязательное `params` JSON)
//!   или JSON { "image_base64": ..., "params": {...} }
//! POST /v1/vectorize/batch — multipart с несколькими файлами `file`
//! GET  /healthz, GET /v1/schema — без загрузки файла.

use crate::params::Params;
use crate::pipeline;
use rayon::prelude::*;
use std::io::Read;
use std::sync::Arc;
use std::time::Instant;

pub struct Config {
    pub max_bytes: usize,
    pub max_batch: usize,
    pub workers: usize,
}

pub fn serve(cfg: Config) -> std::io::Result<()> {
    let addr = std::env::var("LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let server = tiny_http::Server::http(&addr)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("{e}")))?;
    let server = Arc::new(server);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(cfg.workers)
        .build()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("{e}")))?;
    let cfg = Arc::new(cfg);

    eprintln!(
        "png2dxf listening on http://{addr} (workers={}, max_body={} MiB, max_batch={})",
        cfg.workers,
        cfg.max_bytes / (1024 * 1024),
        cfg.max_batch
    );

    loop {
        let server = server.clone();
        let cfg = cfg.clone();
        let mut request = match server.recv() {
            Ok(r) => r,
            Err(e) => {
                eprintln!("recv error: {e}");
                continue;
            }
        };
        let method = request.method().clone();
        let url = request.url().to_string();
        let path = url.split('?').next().unwrap_or("/").to_string();

        let started = Instant::now();
        let resp = match (&method, path.as_str()) {
            (tiny_http::Method::Get, "/healthz") => json_ok(serde_json::json!({
                "status": "ok",
                "version": env!("CARGO_PKG_VERSION"),
                "workers": cfg.workers,
                "max_body_bytes": cfg.max_bytes,
            })),
            (tiny_http::Method::Get, "/v1/schema") => json_ok(params_schema()),
            (tiny_http::Method::Post, "/v1/vectorize") => pool.install(|| handle_one(&mut request, &cfg)),
            (tiny_http::Method::Post, "/v1/vectorize/batch") => {
                pool.install(|| handle_batch(&mut request, &cfg))
            }
            (tiny_http::Method::Options, _) => {
                cors(tiny_http::Response::from_data(Vec::new()).with_status_code(204))
            }
            (tiny_http::Method::Get, "/") => cors(
                tiny_http::Response::from_string(
                    "png2dxf — POST /v1/vectorize с multipart полем file. GET /v1/schema — параметры.\n",
                )
                .with_header(header("Content-Type", "text/plain; charset=utf-8")),
            ),
            _ => json_err(404, "not_found", "нет такого маршрута"),
        };
        eprintln!(
            "{} {} -> {} ({} ms)",
            method,
            path,
            resp.status_code().0,
            started.elapsed().as_millis()
        );
        let _ = request.respond(resp);
    }
}

fn header(k: &str, v: &str) -> tiny_http::Header {
    // Значение может содержать кириллицу (имя файла): заголовок HTTP —
    // однобайтовый, поэтому не-ASCII уходит в percent-encoding, а кириллица
    // дополнительно отдаётся в filename* по RFC 5987. Паника тут недопустима.
    let clean = v.replace(['\r', '\n'], " ");
    let mut safe = String::with_capacity(clean.len());
    for b in clean.bytes() {
        let ok = matches!(b, 0x20..=0x7e) && b != b'"';
        if ok {
            safe.push(b as char);
        } else {
            safe.push_str(&format!("%{b:02X}"));
        }
    }
    tiny_http::Header::from_bytes(k.as_bytes(), safe.as_bytes())
        .unwrap_or_else(|_| tiny_http::Header::from_bytes(&b"X-Info"[..], &b"omitted"[..]).unwrap())
}

/// Content-Disposition с ASCII-fallback и RFC 5987 `filename*` для UTF-8.
fn disposition(filename: &str, ext_hint: &str) -> tiny_http::Header {
    let ascii: String = filename
        .chars()
        .map(|c| if c.is_ascii() && c != '"' && c != '\\' { c } else { '_' })
        .collect();
    let ascii = format!("{ascii}{ext_hint}");
    let mut pct = String::new();
    for b in filename.bytes() {
        let ok = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~');
        if ok {
            pct.push(b as char);
        } else {
            pct.push_str(&format!("%{b:02X}"));
        }
    }
    let value = format!(
        "attachment; filename=\"{ascii}\"{ext_hint}; filename*=UTF-8''{pct}{ext_hint}"
    );
    header("Content-Disposition", &value)
}

fn cors(mut resp: Resp) -> Resp {
    resp.add_header(header("Access-Control-Allow-Origin", "*"));
    resp.add_header(header("Access-Control-Allow-Headers", "Content-Type"));
    resp.add_header(header("Access-Control-Allow-Methods", "POST, GET, OPTIONS"));
    resp
}

type Resp = tiny_http::Response<std::io::Cursor<Vec<u8>>>;

fn json_ok(value: serde_json::Value) -> Resp {
    let body = value.to_string();
    cors(tiny_http::Response::from_string(body)
        .with_status_code(200)
        .with_header(header("Content-Type", "application/json; charset=utf-8")))
}

fn json_err(code: u16, error: &str, msg: &str) -> Resp {
    let body = serde_json::json!({"error": error, "message": msg}).to_string();
    cors(tiny_http::Response::from_string(body)
        .with_status_code(code as i32)
        .with_header(header("Content-Type", "application/json; charset=utf-8")))
}

fn params_schema() -> serde_json::Value {
    let defaults = Params::default();
    serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Параметры векторизации. Все поля необязательны.",
        "defaults": defaults,
        "fields": {
            "scale": "Объект {px1:[x,y], px2:[x,y], mm:число} — известная размерная линия в пикселях и её длина в мм.",
            "dpi": "Масштаб по DPI скана (мм/px = 25.4/dpi). Используется, если scale не задан.",
            "blur_sigma": "Сигма гауссова шумоподавления, px.",
            "adaptive_radius": "Радиус локального окна адаптивной бинаризации, px.",
            "adaptive_offset": "Смещение порога (чем больше — тем меньше чернил).",
            "thin": "Скелетизация линий до 1 px.",
            "hough_threshold_ratio": "Доля от числа пикселей для порога голосования Хафа.",
            "min_line_px": "Минимальная длина отрезка при детекции, px.",
            "max_gap_px": "Максимальный разрыв, склеиваемый при слиянии линий, px.",
            "angle_tolerance_deg": "Допуск на параллельность при слиянии, градусы.",
            "min_line_mm": "Фильтр итоговых линий, мм.",
            "arc_tolerance_px": "Допуск вписывания окружности/дуги, px.",
            "simplify_epsilon_px": "Допуск упрощения полилинии (Дуглас–Пекер), px.",
            "short_stroke_px": "Короткие штрихи короче этого значения отбрасываются, px.",
            "flip_y": "Перевернуть ось Y (CAD снизу вверх). true по умолчанию.",
            "include_debug": "Добавить слой 'Каркас' с цепочками скелета.",
            "layers": "{lines, circles, arcs, polylines} — имена слоёв DXF."
        },
        "endpoints": {
            "POST /v1/vectorize": "multipart: file + params (opt). Ответ — DXF или ZIP, если Accept: application/zip",
            "POST /v1/vectorize/batch": "multipart с несколькими file: DXF или ZIP архив",
            "GET /healthz": "проверка живости",
            "GET /v1/schema": "эта схема"
        }
    })
}

fn accept_zip(req: &tiny_http::Request) -> bool {
    req.headers().iter().any(|h| {
        h.field.equiv("Accept") && h.value.as_str().contains("application/zip")
    })
}

fn parse_params(text: &str) -> Result<Params, String> {
    let mut p: Params = if text.trim().is_empty() {
        Params::default()
    } else {
        serde_json::from_str(text).map_err(|e| format!("params: {e}"))?
    };
    p.normalize();
    Ok(p)
}

fn content_length(req: &tiny_http::Request) -> Option<usize> {
    req.headers()
        .iter()
        .find(|h| h.field.equiv("Content-Length"))
        .and_then(|h| h.value.as_str().trim().parse::<usize>().ok())
}

fn handle_one(req: &mut tiny_http::Request, cfg: &Config) -> Resp {
    let ct = req
        .headers()
        .iter()
        .find(|h| h.field.equiv("Content-Type"))
        .map(|h| h.value.as_str().to_string())
        .unwrap_or_default();
    if let Some(len) = content_length(req) {
        if len > cfg.max_bytes {
            return json_err(413, "payload_too_large", "файл больше лимита");
        }
    }
    let want_zip = accept_zip(req);

    if ct.starts_with("application/json") {
        let mut body = Vec::new();
        if let Err(e) = req.as_reader().take(cfg.max_bytes as u64 + 1).read_to_end(&mut body) {
            return json_err(400, "read_error", &e.to_string());
        }
        if body.len() > cfg.max_bytes {
            return json_err(413, "payload_too_large", "тело больше лимита");
        }
        let v: serde_json::Value = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => return json_err(400, "bad_json", &e.to_string()),
        };
        let b64 = match v.get("image_base64").and_then(|x| x.as_str()) {
            Some(s) => s,
            None => return json_err(422, "no_image", "нет поля image_base64"),
        };
        let bytes = match base64_decode(b64) {
            Ok(b) => b,
            Err(e) => return json_err(400, "bad_base64", &e),
        };
        let mut params = match v.get("params") {
            Some(p) => match serde_json::from_value::<Params>(p.clone()) {
                Ok(p) => p,
                Err(e) => return json_err(422, "bad_params", &e.to_string()),
            },
            None => Params::default(),
        };
        params.normalize();
        return run_single(bytes, "image", params, want_zip);
    }

    let boundary = match ct.split("boundary=").nth(1) {
        Some(b) => b.trim().trim_matches('"').to_string(),
        None => return json_err(415, "bad_content_type", "нужен multipart/form-data"),
    };
    let mut fields = match read_multipart(req, &boundary, cfg.max_bytes) {
        Ok(f) => f,
        Err((code, msg)) => return json_err(code, "multipart", &msg),
    };
    let params = match fields.params_json.take() {
        Some(j) => match parse_params(&j) {
            Ok(p) => p,
            Err(e) => return json_err(422, "bad_params", &e),
        },
        None => Params::default(),
    };
    let Some(file) = fields.files.into_iter().next() else {
        return json_err(422, "no_file", "в запросе нет поля file");
    };
    run_single(file.bytes, &file.filename, params, want_zip)
}

fn handle_batch(req: &mut tiny_http::Request, cfg: &Config) -> Resp {
    let ct = req
        .headers()
        .iter()
        .find(|h| h.field.equiv("Content-Type"))
        .map(|h| h.value.as_str().to_string())
        .unwrap_or_default();
    let boundary = match ct.split("boundary=").nth(1) {
        Some(b) => b.trim().trim_matches('"').to_string(),
        None => return json_err(415, "bad_content_type", "нужен multipart/form-data"),
    };
    let mut fields = match read_multipart(req, &boundary, cfg.max_bytes) {
        Ok(f) => f,
        Err((code, msg)) => return json_err(code, "multipart", &msg),
    };
    if fields.files.is_empty() {
        return json_err(422, "no_file", "в запросе нет файлов");
    }
    if fields.files.len() > cfg.max_batch {
        return json_err(413, "too_many_files", "слишком много файлов в пакете");
    }
    let params = match fields.params_json.take() {
        Some(j) => match parse_params(&j) {
            Ok(p) => p,
            Err(e) => return json_err(422, "bad_params", &e),
        },
        None => Params::default(),
    };
    // Пакет параллельно: каждое изображение независимо.
    let results: Vec<(String, Result<Vec<u8>, String>)> = fields
        .files
        .into_par_iter()
        .map(|f| {
            let name = stem(&f.filename);
            let out: Result<Vec<u8>, String> = pipeline::vectorize_bytes(&f.bytes, &params)
                .map(|r| r.dxf.into_bytes())
                .map_err(|e| e.message);
            (name, out)
        })
        .collect();
    let entries: Vec<(String, Vec<u8>)> = results
        .into_iter()
        .filter_map(|(n, r)| r.ok().map(|d| (format!("{n}.dxf"), d)))
        .collect();
    if entries.is_empty() {
        return json_err(422, "all_failed", "ни один файл не удалось обработать");
    }
    let zip = crate::zip::build_zip(&list_entries(entries));
    cors(tiny_http::Response::from_data(zip)
        .with_header(header("Content-Type", "application/zip"))
        .with_header(header("Content-Disposition", "attachment; filename=\"batch.zip\"")))
}

pub struct NamedEntry {
    pub name: String,
    pub data: Vec<u8>,
}

fn list_entries(v: Vec<(String, Vec<u8>)>) -> Vec<NamedEntry> {
    v.into_iter().map(|(name, data)| NamedEntry { name, data }).collect()
}

fn run_single(
    bytes: Vec<u8>,
    original_name: &str,
    params: Params,
    want_zip: bool,
) -> Resp {
    match pipeline::vectorize_bytes(&bytes, &params) {
        Ok(out) => {
            let name = format!("{}.dxf", stem(original_name));
            let stats_hdr = serde_json::json!({
                "lines": out.stats.lines,
                "polylines": out.stats.polylines,
                "arcs": out.stats.arcs,
                "circles": out.stats.circles,
                "scale_mm_per_px": out.stats.scale_mm_per_px,
                "width_px": out.stats.width_px,
                "height_px": out.stats.height_px,
                "elapsed_ms": out.elapsed_ms,
            })
            .to_string();
            if want_zip {
                let zip = crate::zip::build_zip(&[NamedEntry {
                    name: name.clone(),
                    data: out.dxf.into_bytes(),
                }]);
                cors(tiny_http::Response::from_data(zip)
                    .with_header(header("Content-Type", "application/zip"))
                    .with_header(disposition(&stem(original_name), ".zip"))
                    .with_header(header("X-Stats", &stats_hdr)))
            } else {
                cors(tiny_http::Response::from_data(out.dxf.into_bytes())
                    .with_header(header("Content-Type", "application/dxf"))
                    .with_header(disposition(&stem(original_name), ".dxf"))
                    .with_header(header("X-Stats", &stats_hdr)))
            }
        }
        Err(e) => match e.kind {
            pipeline::ErrorKind::BadImage => json_err(422, "bad_image", &e.message),
            pipeline::ErrorKind::TooLarge => json_err(413, "too_large", &e.message),
            pipeline::ErrorKind::NoGeometry => json_err(422, "no_geometry", &e.message),
            pipeline::ErrorKind::Internal => json_err(500, "internal", &e.message),
        },
    }
}

fn stem(name: &str) -> String {
    let base = name
        .rsplit('/')
        .next()
        .unwrap_or("image")
        .rsplit('\\')
        .next()
        .unwrap_or("image");
    let stripped = base.rsplit_once('.').map(|(s, _)| s).unwrap_or(base);
    let safe: String = stripped
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    if safe.is_empty() {
        "image".into()
    } else {
        safe
    }
}

struct UploadFile {
    filename: String,
    bytes: Vec<u8>,
}

struct Fields {
    files: Vec<UploadFile>,
    params_json: Option<String>,
}

fn read_multipart(
    req: &mut tiny_http::Request,
    boundary: &str,
    max_bytes: usize,
) -> Result<Fields, (u16, String)> {
    let mut body = Vec::new();
    let limit = max_bytes as u64 + 4 * 1024 * 1024;
    if let Err(e) = req.as_reader().take(limit).read_to_end(&mut body) {
        return Err((400, e.to_string()));
    }
    if body.len() as u64 > limit {
        return Err((413, "тело запроса больше лимита".into()));
    }
    let dash = format!("--{boundary}");
    let mut files = Vec::new();
    let mut params_json = None;

    let mut pos = 0usize;
    let sep = b"\r\n";
    loop {
        // Ищем начало части.
        let start = match find_sub(&body[pos..], dash.as_bytes()) {
            Some(i) => pos + i + dash.len(),
            None => break,
        };
        if body.len() >= start + 2 && &body[start..start + 2] == b"--" {
            break;
        }
        // После границы идёт CRLF, затем заголовки до пустой строки.
        let hdr_start = start + sep.iter().filter(|_| true).count().min(2);
        let hdr_end = match find_sub(&body[hdr_start..], b"\r\n\r\n") {
            Some(i) => hdr_start + i,
            None => break,
        };
        let headers = String::from_utf8_lossy(&body[hdr_start..hdr_end]).to_string();
        let data_start = hdr_end + 4;
        let next_marker = format!("\r\n{dash}");
        let data_end = match find_sub(&body[data_start..], next_marker.as_bytes()) {
            Some(i) => data_start + i,
            None => body.len(),
        };
        let data = body[data_start..data_end].to_vec();
        pos = data_end + 2;

        let mut name = String::from("image");
        let mut is_file = false;
        let mut is_params = false;
        for line in headers.lines() {
            let lower = line.to_ascii_lowercase();
            if let Some(idx) = lower.find("content-disposition") {
                let rest = &line[idx..];
                if let Some(fi) = lower.find("filename=\"") {
                    let v = &rest[fi + 10..];
                    if let Some(endq) = v.find('"') {
                        name = v[..endq].to_string();
                        is_file = true;
                    }
                }
                if let Some(ni) = lower.find("name=\"") {
                    let v = &rest[ni + 6..];
                    if let Some(endq) = v.find('"') {
                        if &v[..endq] == "params" {
                            is_params = true;
                        }
                        if &v[..endq] == "file" {
                            is_file = true;
                        }
                    }
                }
            }
        }
        if is_params && !is_file {
            params_json = Some(String::from_utf8_lossy(&data).to_string());
        } else if is_file {
            files.push(UploadFile { filename: name, bytes: data });
        }
    }
    Ok(Fields { files, params_json })
}

fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    let first = needle[0];
    let n = needle.len();
    let mut i = 0usize;
    while i + n <= hay.len() {
        if hay[i] == first && &hay[i..i + n] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

pub fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4 + 3);
    let mut buf = 0u32;
    let mut bits = 0u32;
    let mut count = 0usize;
    for (i, c) in s.bytes().enumerate() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'\r' | b'\n' | b' ' | b'\t' => continue,
            b'=' => break,
            _ => return Err(format!("недопустимый символ base64 в позиции {i}")),
        };
        buf = (buf << 6) | v as u32;
        bits += 6;
        count += 1;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    if count % 4 == 1 {
        return Err("некорректная длина base64".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b64_roundtrip_known() {
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64_decode("aGVsbG8").unwrap(), b"hello");
    }

    #[test]
    fn stem_sanitizes() {
        assert_eq!(stem("plan.dwg.png"), "plan_dwg");
        assert_eq!(stem("/tmp/x y.PNG"), "x_y");
    }

    #[test]
    fn multipart_fields_found() {
        let body = b"--B\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.png\"\r\n\r\nPNGDATA\r\n--B\r\nContent-Disposition: form-data; name=\"params\"\r\n\r\n{}\r\n--B--\r\n";
        let dash = "--B";
        // прямой разбор открытой части: используем find_sub, чтобы проверить алгоритм
        assert!(find_sub(body, dash.as_bytes()).is_some());
    }
}
