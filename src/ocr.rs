//! OCR-слой: распознавание надписей на чертеже через Tesseract.
//!
//! Движок вызывается как внешняя утилита (`tesseract <png> stdout -l rus --psm 11`),
//! поэтому библиотека не тянет TensorFlow и не ломает статическую сборку.
//! Если бинарь не найден — работа продолжается без текста (текст необязателен).
//!
//! Маска текста передаётся сюда же: если у нас уже есть «символьные» блобы,
//! рисуем их на белом фоне и отдаём OCR — так он видит только надписи, а не
//! линии чертежа. Это тот самый случай, когда маска полезна именно для OCR,
//! а не для вычищения геометрии.

use crate::pipeline::TextItem;
use image::{GrayImage, Luma};
use std::process::{Command, Stdio};

pub fn available() -> bool {
    Command::new("tesseract")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Распознаёт надписи. `text_mask` — маска символьных блобов; если пустая,
/// используется всё изображение.
pub fn recognize(
    gray: &GrayImage,
    text_mask: Option<&GrayImage>,
    lang: &str,
    min_conf: f32,
    upscale: u32,
) -> Vec<TextItem> {
    if !available() {
        return Vec::new();
    }
    // Кроп по облаку маски: OCR быстрее и точнее на подготовленном растре.
    let prepared = match text_mask {
        Some(m) if m.as_raw().iter().any(|&p| p > 0) => {
            let (w, h) = gray.dimensions();
            let mut out = GrayImage::from_pixel(w, h, Luma([255]));
            for y in 0..h {
                for x in 0..w {
                    if m.get_pixel(x, y)[0] > 0 {
                        out.put_pixel(x, y, *gray.get_pixel(x, y));
                    }
                }
            }
            out
        }
        _ => gray.clone(),
    };

    // Tesseract лучше читает с 2–3-кратным увеличением мелких сканов.
    let scale = upscale.clamp(1, 4);
    let (w, h) = prepared.dimensions();
    let mut big = GrayImage::new(w * scale, h * scale);
    for y in 0..h * scale {
        for x in 0..w * scale {
            let p = prepared.get_pixel(x / scale, y / scale)[0];
            big.put_pixel(x, y, Luma([p]));
        }
    }

    let dir = std::env::temp_dir();
    let path = dir.join(format!("png2dxf-ocr-{}.png", std::process::id()));
    if image::ImageBuffer::<Luma<u8>, Vec<u8>>::from_raw(big.width(), big.height(), big.as_raw().clone())
        .map(|b| b.save(&path))
        .is_none()
    {
        return Vec::new();
    }

    let output = Command::new("tesseract")
        .arg(&path)
        .arg("stdout")
        .args(["-l", lang, "--psm", "11", "tsv"])
        .stdin(Stdio::null())
        .output();
    let _ = std::fs::remove_file(&path);

    let Ok(out) = output else {
        return Vec::new();
    };
    let tsv = String::from_utf8_lossy(&out.stdout);
    let mut items = parse_tsv(&tsv, scale as f64, min_conf);

    // Второй проход по всему листу: маска режет символы статуса («8» вместо
    // «В», «$» вместо «Ф»). Полный проход даёт больше мусора, но честнее
    // читает крупные надписи. Совпавшие по месту фразы считаем подтверждёнными.
    if let Some(full) = recognize_full(gray, lang, min_conf, upscale) {
        for f in full {
            let dup = items.iter().any(|i| {
                let cx = i.x + i.w / 2.0;
                let cy = i.y + i.h / 2.0;
                let fx = f.x + f.w / 2.0;
                let fy = f.y + f.h / 2.0;
                (cx - fx).abs() < i.w.max(f.w) && (cy - fy).abs() < i.h.max(f.h)
            });
            if !dup {
                items.push(f);
            }
        }
    }
    // Третий проход: плотные блоки (примечания, легенды) распознаются побайтно
    // лучше при --psm 6 по отдельному кропу, чем при --psm 11 по целому листу.
    if let Some(m) = text_mask {
        let blocks = text_blocks(m);
        eprintln!("ocr: блоков найдено {}", blocks.len());
        for blk in blocks {
            let got = recognize_block(gray, &blk, lang);
            eprintln!(
                "ocr: блок {}x{} @({},{}) -> {} фраз: {:?}",
                blk.w, blk.h, blk.x, blk.y, got.len(),
                got.iter().map(|t| t.text.as_str()).take(3).collect::<Vec<_>>()
            );
            // Фраза из блока считается главной: она прочитана целиком
            // (--psm 6 по кропу). Обрывки тех же слов, пришедшие из общих
            // проходов, из списка выкидываем — иначе вместо строки в DXF
            // лежит набор «за», «си», «га».
            for t in got {
                items.retain(|i| {
                    let inside = i.x >= t.x - 2.0
                        && i.y >= t.y - 2.0
                        && i.x + i.w <= t.x + t.w + 2.0
                        && i.y + i.h <= t.y + t.h + 2.0;
                    !inside
                });
                items.push(t);
            }
        }
    }

    // Финальная сборка: слова одного абзаца, пришедшие из разных проходов,
    // снова склеиваются в строки. Иначе примечание лежит в DXF отдельными
    // обрывками («проект», «000», «ДГ,»).
    let items = group_paragraphs(items);
    let mut items = items;
    items.sort_by(|a, b| (a.y as i32).cmp(&(b.y as i32)).then(a.x.partial_cmp(&b.x).unwrap()));
    items
}

/// Склеивает фрагменты в строки по вертикальному центру, затем строки в
/// абзац по горизонтальному перекрытию — восстанавливая блок примечаний.
fn group_paragraphs(items: Vec<TextItem>) -> Vec<TextItem> {
    if items.is_empty() {
        return items;
    }
    let mut lines: Vec<TextItem> = Vec::new();
    let mut rest: Vec<TextItem> = items;
    rest.sort_by(|a, b| a.y.partial_cmp(&b.y).unwrap());

    for it in rest {
        let mut placed = false;
        for line in lines.iter_mut() {
            let cy_l = line.y + line.h / 2.0;
            let cy_i = it.y + it.h / 2.0;
            let gap = it.x - (line.x + line.w);
            if (cy_l - cy_i).abs() < line.h.max(it.h) * 0.55
                && gap > -line.h * 1.5
                && gap < line.h.max(it.h) * 2.5
            {
                if gap > 0.0 {
                    line.text.push(' ');
                }
                line.text.push_str(&it.text);
                let right = (line.x + line.w).max(it.x + it.w);
                let bottom = (line.y + line.h).max(it.y + it.h);
                line.w = right - line.x;
                line.h = bottom - line.y;
                placed = true;
                break;
            }
        }
        if !placed {
            lines.push(it);
        }
    }
    lines
}

/// Прямоугольник текстового блока в пикселях (x, y, w, h).
#[derive(Debug, Clone, Copy)]
struct Block {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

/// Прямоугольники строк текста: компоненты маски группируются в строки по
/// вертикальному перекрытию, затем строки склеиваются в абзац, если между ними
/// нет большого разрыва. Это надёжнее «взрывной» склейки по радиусу: строки
/// примечаний становятся одним блоком, а разрозненные подписи в узле — нет.
fn text_blocks(mask: &GrayImage) -> Vec<Block> {
    let (w, h) = mask.dimensions();
    // Перед анализом «утолщаем» маску на 2 px: внутри подписи знаки иначе
    // разваливаются на отдельные пиксели и bbox каждого символа не собирается.
    let mut dense = mask.clone();
    for _ in 0..2 {
        dense = crate::preprocess::dilate(&dense, 1);
    }
    let raw = dense.as_raw();
    let wu = w as usize;
    let hu = h as usize;

    // 1) Связные компоненты маски (8-связность), только bbox.
    let mut seen = vec![false; wu * hu];
    let mut comps: Vec<(u32, u32, u32, u32, usize)> = Vec::new();
    for y in 0..hu {
        for x in 0..wu {
            let i = y * wu + x;
            if seen[i] || raw[i] == 0 {
                continue;
            }
            let mut stack = vec![(x, y)];
            seen[i] = true;
            let (mut x0, mut y0, mut x1, mut y1) = (x, y, x, y);
            let mut count = 0usize;
            while let Some((cx, cy)) = stack.pop() {
                count += 1;
                x0 = x0.min(cx);
                y0 = y0.min(cy);
                x1 = x1.max(cx);
                y1 = y1.max(cy);
                for dy in -1i64..=1 {
                    for dx in -1i64..=1 {
                        let nx = cx as i64 + dx;
                        let ny = cy as i64 + dy;
                        if nx < 0 || ny < 0 || nx >= wu as i64 || ny >= hu as i64 {
                            continue;
                        }
                        let ni = ny as usize * wu + nx as usize;
                        if !seen[ni] && raw[ni] > 0 {
                            seen[ni] = true;
                            stack.push((nx as usize, ny as usize));
                        }
                    }
                }
            }
            comps.push((x0 as u32, y0 as u32, x1 as u32, y1 as u32, count));
        }
    }

    // 2) Группируем компоненты в строки: пересечение по Y не меньше половины
    //    меньшей высоты и зазор по X не больше 1.5 высоты символа.
    let mut lines: Vec<(u32, u32, u32, u32, usize)> = Vec::new();
    for c in comps {
        let (cx0, cy0, cx1, cy1, cnt) = c;
        let ch = cy1.saturating_sub(cy0) + 1;
        let mut placed = false;
        for line in lines.iter_mut() {
            let (lx0, ly0, lx1, ly1, lcnt) = *line;
            let lh = ly1.saturating_sub(ly0) + 1;
            let overlap = cy1.min(ly1).saturating_sub(cy0.max(ly0)) + 1;
            let min_h = ch.min(lh);
            let gap_x = if cx0 > lx1 { cx0 - lx1 } else if lx0 > cx1 { lx0 - cx1 } else { 0 };
            if overlap * 2 >= min_h && gap_x <= (min_h as f64 * 2.2) as u32 {
                line.0 = lx0.min(cx0);
                line.1 = ly0.min(cy0);
                line.2 = lx1.max(cx1);
                line.3 = ly1.max(cy1);
                line.4 = lcnt + cnt;
                placed = true;
                break;
            }
        }
        if !placed {
            lines.push((cx0, cy0, cx1, cy1, cnt));
        }
    }

    // 3) Склеиваем строки в абзац: одна ширина (±40%), зазор по Y меньше
    //    двух высот строки, перекрытие по X заметное.
    lines.sort_by_key(|l| l.1);
    let mut paras: Vec<(u32, u32, u32, u32, usize, usize)> = Vec::new(); // +строк
    for l in lines {
        let (lx0, ly0, lx1, ly1, lcnt) = l;
        let lh = ly1.saturating_sub(ly0) + 1;
        let lw = lx1.saturating_sub(lx0) + 1;
        let mut merged = false;
        for para in paras.iter_mut() {
            let (px0, py0, px1, py1, pcnt, pn) = *para;
            let pw = px1.saturating_sub(px0) + 1;
            let vertical_gap = if ly0 > py1 { ly0 - py1 } else { 0 };
            let x_overlap = lx1.min(px1).saturating_sub(lx0.max(px0)) + 1;
            let similar_w = (lw as f64) / (pw as f64) > 0.55;
            if vertical_gap <= lh * 2 && similar_w && x_overlap * 8 > lw {
                para.0 = px0.min(lx0);
                para.1 = py0.min(ly0);
                para.2 = px1.max(lx1);
                para.3 = py1.max(ly1);
                para.4 = pcnt + lcnt;
                para.5 = pn + 1;
                merged = true;
                break;
            }
        }
        if !merged {
            paras.push((lx0, ly0, lx1, ly1, lcnt, 1));
        }
    }

    // 4) Абзац — минимум 2 строки и приличная ширина: иначе это подпись,
    //    уже распознанная общим проходом.
    paras
        .into_iter()
        .filter(|p| p.5 >= 2 && (p.2 - p.0) > 90)
        .map(|p| Block {
            x: p.0.saturating_sub(4),
            y: p.1.saturating_sub(4),
            w: (p.2 - p.0) + 8,
            h: (p.3 - p.1) + 8,
        })
        .collect()
}

/// Распознаёт один блок кропом с --psm 6 (единый текстовый блок).
fn recognize_block(gray: &GrayImage, blk: &Block, lang: &str) -> Vec<TextItem> {
    let pad = 6u32;
    let x0 = blk.x.saturating_sub(pad);
    let y0 = blk.y.saturating_sub(pad);
    let x1 = (blk.x + blk.w + pad).min(gray.width());
    let y1 = (blk.y + blk.h + pad).min(gray.height());
    let (cw, ch) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
    if cw < 20 || ch < 20 {
        return Vec::new();
    }
    // Кроп + лёгкое размытие + апскейл ×3: именно эта комбинация на блоке
    // примечаний дала меньше всего ошибок; бинаризация Оцу съедала антиалиасинг.
    let scale = 3u32;
    // Незначительное повышение резкости (unsharp) — именно оно на блоке
    // примечаний убрало «6/8» вместо «в» и «$» вместо «Ф».
    let mut sharp = gray.clone();
    for y in 1..gray.height().saturating_sub(1) {
        for x in 1..gray.width().saturating_sub(1) {
            let c = gray.get_pixel(x, y)[0] as i32;
            let around = (gray.get_pixel(x - 1, y)[0] as i32
                + gray.get_pixel(x + 1, y)[0] as i32
                + gray.get_pixel(x, y - 1)[0] as i32
                + gray.get_pixel(x, y + 1)[0] as i32)
                / 4;
            let v = (c + (c - around)).clamp(0, 255) as u8;
            sharp.put_pixel(x, y, Luma([v]));
        }
    }
    let mut big = GrayImage::new(cw * scale, ch * scale);
    for y in 0..ch * scale {
        for x in 0..cw * scale {
            big.put_pixel(x, y, *sharp.get_pixel(x0 + x / scale, y0 + y / scale));
        }
    }
    let dir = std::env::temp_dir();
    let path = dir.join(format!("png2dxf-blk-{}-{}.png", std::process::id(), blk.x));
    if image::ImageBuffer::<Luma<u8>, Vec<u8>>::from_raw(big.width(), big.height(), big.as_raw().clone())
        .map(|b| b.save(&path))
        .is_none()
    {
        return Vec::new();
    }
    let out = Command::new("tesseract")
        .arg(&path)
        .arg("stdout")
        .args(["-l", lang, "--psm", "6", "tsv"])
        .output();
    let _ = std::fs::remove_file(&path);
    let Ok(out) = out else {
        return Vec::new();
    };
    // Парсер строк: block_num связывает слова в строку, координаты берём от
    // первого и последнего слова — так фраза примечания остаётся целиком,
    // а не рассыпается на «слова из двух букв», отсеянные фильтром.
    let tsv = String::from_utf8_lossy(&out.stdout);
    let mut rows: std::collections::BTreeMap<u32, Vec<TextItem>> = Default::default();
    for line in tsv.lines().skip(1) {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 12 {
            continue;
        }
        let conf: f32 = f[10].parse().unwrap_or(-1.0);
        let text = f[11].trim();
        if conf < 35.0 || text.is_empty() {
            continue;
        }
        let (Ok(left), Ok(top), Ok(w), Ok(h)) = (
            f[6].parse::<f64>(),
            f[7].parse::<f64>(),
            f[8].parse::<f64>(),
            f[9].parse::<f64>(),
        ) else {
            continue;
        };
        let key: u32 = format!("{}{}{}", f[2], f[3], f[4]).parse().unwrap_or(0);
        rows.entry(key).or_default().push(TextItem {
            text: text.to_string(),
            x: left / scale as f64,
            y: top / scale as f64,
            w: w / scale as f64,
            h: h / scale as f64,
        });
    }
    let mut items = Vec::new();
    for (_, mut words) in rows {
        words.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap());
        let mut joined = String::new();
        let first = words[0].clone();
        let mut right = 0.0f64;
        let mut bottom = 0.0f64;
        for wrd in &words {
            if !joined.is_empty() {
                joined.push(' ');
            }
            joined.push_str(&wrd.text);
            right = right.max(wrd.x + wrd.w);
            bottom = bottom.max(wrd.y + wrd.h);
        }
        let letters = joined.chars().filter(|c| c.is_alphanumeric()).count();
        if letters < 3 {
            continue;
        }
        items.push(TextItem {
            text: joined,
            x: first.x + x0 as f64,
            y: first.y + y0 as f64,
            w: right - first.x,
            h: bottom - first.y,
        });
    }
    items
}

/// Полный лист без маски (с апскейлом ×2) — второй проход OCR.
fn recognize_full(gray: &GrayImage, lang: &str, min_conf: f32, upscale: u32) -> Option<Vec<TextItem>> {
    let scale = (upscale + 1).clamp(2, 4);
    let (w, h) = gray.dimensions();
    let mut big = GrayImage::new(w * scale, h * scale);
    for y in 0..h * scale {
        for x in 0..w * scale {
            big.put_pixel(x, y, *gray.get_pixel(x / scale, y / scale));
        }
    }
    let path = std::env::temp_dir().join(format!("png2dxf-ocr-full-{}.png", std::process::id()));
    image::ImageBuffer::<Luma<u8>, Vec<u8>>::from_raw(big.width(), big.height(), big.as_raw().clone())
        .map(|b| b.save(&path))?;
    let out = Command::new("tesseract")
        .arg(&path)
        .arg("stdout")
        .args(["-l", lang, "--psm", "11", "tsv"])
        .output();
    let _ = std::fs::remove_file(&path);
    let out = out.ok()?;
    Some(parse_tsv(&String::from_utf8_lossy(&out.stdout), scale as f64, min_conf))
}

/// Разбирает TSV Tesseract: колонки level, left, top, width, height, conf, text.
fn parse_tsv(tsv: &str, scale: f64, min_conf: f32) -> Vec<TextItem> {
    let mut items = Vec::new();
    for line in tsv.lines().skip(1) {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 12 {
            continue;
        }
        let conf: f32 = f[10].parse().unwrap_or(-1.0);
        let text = f[11].trim();
        if conf < min_conf || text.is_empty() {
            continue;
        }
        // Мусор из одиночных символов и «распознанных» кусков линий.
        // Одиночный символ — обычно обрывок линии. Оставляем только цифры
        // и латиницу: в отметках высот «5» бывает значима, а «|», «-», «а»
        // от случайно распознанных штрихов — нет.
        let alnum = text.chars().filter(|c| c.is_alphanumeric()).count();
        if alnum < 2 {
            let single_ok = text.len() == 1
                && text.chars().all(|c| c.is_ascii_digit() || c.is_ascii_alphabetic());
            if !single_ok || conf < 60.0 {
                continue;
            }
        }
        let (Ok(left), Ok(top), Ok(w), Ok(h)) = (
            f[6].parse::<f64>(),
            f[7].parse::<f64>(),
            f[8].parse::<f64>(),
            f[9].parse::<f64>(),
        ) else {
            continue;
        };
        items.push(TextItem {
            text: text.to_string(),
            x: left / scale as f64,
            y: top / scale as f64,
            w: w / scale as f64,
            h: h / scale as f64,
        });
    }
    // Склеиваем рядом стоящие слова одной строки в фразы.
    merge_words(items)
}

fn merge_words(mut items: Vec<TextItem>) -> Vec<TextItem> {
    items.sort_by(|a, b| {
        (a.y as i32)
            .cmp(&(b.y as i32))
            .then(a.x.partial_cmp(&b.x).unwrap())
    });
    let mut out: Vec<TextItem> = Vec::new();
    for it in items {
        if let Some(last) = out.last_mut() {
            let same_line = (it.y - last.y).abs() < last.h.max(it.h) * 0.6;
            let gap = it.x - (last.x + last.w);
            // Склеиваем только близкие слова: зазор < 1.2 высоты строки.
            // Иначе фраза «съедает» соседнюю колонку.
            let same_font = last.h.max(it.h) > 0.0
                && (last.h / it.h).clamp(0.5, 2.0) == last.h / it.h;
            if same_line && same_font && gap >= -last.w * 0.3 && gap < last.h.max(it.h) * 1.2 {
                last.text.push(' ');
                last.text.push_str(&it.text);
                let right = (last.x + last.w).max(it.x + it.w);
                let bottom = (last.y + last.h).max(it.y + it.h);
                last.w = right - last.x;
                last.h = bottom - last.y;
                continue;
            }
        }
        out.push(it);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tsv_and_merges_words() {
        let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
5\t1\t1\t1\t1\t1\t100\t50\t80\t20\t92.5\tЗаглушка\n\
5\t1\t1\t1\t1\t2\t190\t50\t40\t20\t88.0\tФ32\n\
5\t1\t1\t1\t2\t1\t100\t200\t10\t10\t20.0\t|\n";
        let items = parse_tsv(tsv, 2.0, 30.0);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text, "Заглушка Ф32");
        assert!((items[0].x - 50.0).abs() < 1e-6);
    }
}
