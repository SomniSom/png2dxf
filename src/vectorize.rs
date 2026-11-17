//! Векторизация: вероятностное преобразование Хафа по маске рёбер,
//! обход скелета в цепочки, упрощение (Дуглас–Пекер) и вписывание
//! окружностей/дуг (алгебраический МНК Kasa).

use crate::geom::{merge_collinear, pt_line_dist, split_at_intersections, Pt, Seg};
use crate::params::Params;
use image::{GrayImage, Luma};
use rayon::prelude::*;

#[derive(Debug, Clone)]
pub struct Circle {
    pub center: Pt,
    pub radius: f64,
    pub layer_circle: bool,
}

#[derive(Debug, Clone)]
pub struct Arc {
    pub center: Pt,
    pub radius: f64,
    pub start_rad: f64,
    pub end_rad: f64,
}

#[derive(Debug, Clone, Default)]
pub struct VectorData {
    pub segs: Vec<Seg>,
    pub arcs: Vec<Arc>,
    pub circles: Vec<Circle>,
    pub polylines: Vec<Vec<Pt>>,
    pub skeleton: Vec<Vec<Pt>>,
    pub dropped_short: usize,
    pub chain_points: usize,
}

/// Детектор прямых по преобразованию Хафа.
///
/// Отличие от классического HoughLinesP: линия не набирает голоса «на бин»,
/// а затем ищется по полосе. Здесь обратное — каждый пиксель голосует за
/// (rho, theta), после чего пик аккумулятора **разворачивается обратно**:
/// берём пиксели, лежащие в полосе от него, сортируем по проекции вдоль
/// прямой и собираем непрерывные участки допустимой плотностью. Поэтому
/// тонкая линия (1–2 px после скелетизации) не теряется из-за разрывов:
/// достаточно, чтобы 40% участка приходилось на чернила.
pub fn hough_lines(edges: &GrayImage, px: &Params) -> Vec<Seg> {
    hough_lines_with_stride(edges, px, 1)
}

/// То же, но голосует каждая `stride`-я чернильная точка — для поиска
/// длинных прямых на плотных сканах, где полный обход всех пикселей дорог.
fn hough_lines_with_stride(edges: &GrayImage, px: &Params, stride: usize) -> Vec<Seg> {
    let (w, h) = edges.dimensions();
    if w < 3 || h < 3 {
        return Vec::new();
    }
    let (ew, eh) = (w as usize, h as usize);
    let mask = edges.as_raw();
    let stride = stride.max(1);
    let mut pts: Vec<(f32, f32)> = Vec::with_capacity(ew * eh / 8);
    let mut all: Vec<(f32, f32)> = Vec::with_capacity(ew * eh / 8);
    let mut seen = 0usize;
    for y in 0..eh {
        for x in 0..ew {
            if mask[y * ew + x] == 0 {
                continue;
            }
            all.push((x as f32, y as f32));
            if seen % stride == 0 {
                pts.push((x as f32, y as f32));
            }
            seen += 1;
        }
    }
    if pts.len() < 8 {
        return Vec::new();
    }
    let vote_pts: &[(f32, f32)] = if stride == 1 { &pts } else { &pts };
    let diag = ((ew * ew + eh * eh) as f64).sqrt();
    let ang_step = px.hough_angle_step_deg as f64;
    let n_theta = ((180.0 / ang_step).round() as usize).max(1);
    let rho_step = px.hough_rho_step as f64;
    let rho_max = (diag / rho_step).ceil() as i64 + 1;
    let n_rho = (2 * rho_max + 1) as usize;
    let offset = rho_max as f64;

    let mut cos = vec![0f64; n_theta];
    let mut sin = vec![0f64; n_theta];
    for k in 0..n_theta {
        let a = (k as f64) * ang_step.to_radians();
        cos[k] = a.cos();
        sin[k] = a.sin();
    }

    // Голосование параллельно по полосам: у каждой полосы свой аккумулятор,
    // затем они складываются. Память — доли сотен МБ даже для A0.
    let bands = rayon::current_num_threads().max(1);
    let band_h = (eh + bands - 1) / bands;
    let chunk = (vote_pts.len() + bands - 1) / bands;
    let partial: Vec<Vec<u32>> = (0..bands)
        .into_par_iter()
        .map(|b| {
            let mut local = vec![0u32; n_theta * n_rho];
            let from = b * chunk;
            let to = ((b + 1) * chunk).min(vote_pts.len());
            if from >= to {
                return local;
            }
            // Полосы по индексу точек, а не по строкам: в скелете точки
            // могут быть размазаны по всей высоте.
            let _ = band_h;
            for item in &vote_pts[from..to] {
                let (x, y) = (item.0 as f64, item.1 as f64);
                for k in 0..n_theta {
                    let rho = (x * cos[k] + y * sin[k]) / rho_step;
                    let idx = ((rho + offset).round() as i64).clamp(0, n_rho as i64 - 1) as usize;
                    local[k * n_rho + idx] += 1;
                }
            }
            local
        })
        .collect();
    let mut acc = vec![0u32; n_theta * n_rho];
    for part in &partial {
        for (i, v) in part.iter().enumerate() {
            acc[i] += *v;
        }
    }

    // Порог: доля от пика аккумулятора. Пик соответствует самой длинной
    // прямой в полосе; на скане с множеством подписей «доля от чернил»
    // даёт слишком высокий порог, а фиксированный минимум — мусор.
    let peak = acc.iter().copied().max().unwrap_or(0);
    let thr_peak = ((peak as f32) * px.hough_threshold_ratio.max(0.06)) as u32;
    let thr = thr_peak.max(px.hough_threshold_min).max(8);

    // Немаксимум по (theta, rho): оставляем только локальные максимумы.
    let mut cand: Vec<(u32, usize, usize)> = Vec::new();
    for k in 0..n_theta {
        for r in 0..n_rho {
            let v = acc[k * n_rho + r];
            if v < thr {
                continue;
            }
            let mut is_max = true;
            'outer: for dk in -1i64..=1 {
                for dr in -1i64..=1 {
                    if dk == 0 && dr == 0 {
                        continue;
                    }
                    let kk = k as i64 + dk;
                    let rr = r as i64 + dr;
                    if kk < 0 || kk >= n_theta as i64 || rr < 0 || rr >= n_rho as i64 {
                        continue;
                    }
                    if acc[kk as usize * n_rho + rr as usize] > v {
                        is_max = false;
                        break 'outer;
                    }
                }
            }
            if is_max {
                cand.push((v, k, r));
            }
        }
    }
    cand.par_sort_unstable_by(|a, b| b.0.cmp(&a.0));

    let angle_tol = (px.angle_tolerance_deg as f64).to_radians();
    let band = px.hough_band_px as f64;
    let mut segs: Vec<Seg> = Vec::new();
    let mut consumed = vec![false; all.len()];
    let max_lines = 20_000usize;
    for (votes, k, r) in cand.into_iter().take(max_lines) {
        let a = (k as f64) * ang_step.to_radians();
        let (c, s) = (a.cos(), a.sin());
        let rho = (r as f64 - offset) * rho_step;
        let base = Pt::new(rho * c, rho * s);
        let d = (-s, c);

        // Пиксели в полосе линии, ещё не «занятые» более сильной прямой.
        let mut ts: Vec<f64> = Vec::new();
        for (i, &(x, y)) in all.iter().enumerate() {
            if consumed[i] {
                continue;
            }
            if (x as f64 * c + y as f64 * s - rho).abs() > band {
                continue;
            }
            ts.push((x as f64 - base.x) * d.0 + (y as f64 - base.y) * d.1);
        }
        if ts.len() < 4 {
            continue;
        }
        ts.sort_by(|a, b| a.partial_cmp(b).unwrap());
        // Разрыв до max_gap_px — это обрыв штриха или провал скелета на
        // пересечении, его склеиваем (иначе одна линия рассыпается на десятки
        // кусков и теряется). Настоящий конец линии — разрыв больше.
        let gap = (px.max_gap_px as f64).max(2.0) + 1.0;
        let mut pieces: Vec<(f64, f64)> = Vec::new();
        let mut run_start = ts[0];
        let mut prev = ts[0];
        for &t in ts.iter().skip(1) {
            if t - prev > gap {
                if prev - run_start >= px.min_line_px as f64 * 0.5 {
                    pieces.push((run_start, prev));
                }
                run_start = t;
            }
            prev = t;
        }
        if prev - run_start >= px.min_line_px as f64 * 0.5 {
            pieces.push((run_start, prev));
        }
        if pieces.is_empty() {
            continue;
        }

        // Отбраковка «натянутых» линий: прямая прошла по длинному участку,
        // но пиксели чернил занимают малую долю длины — это линия через
        // пустой фон, а не контур. Такие снимаем до записи в DXF.
        let span = pieces.iter().map(|&(a, b)| b - a).sum::<f64>();
        let ink_ratio = ts.len() as f64 / span.max(1.0);
        if ink_ratio < 0.55 && span > 6.0 {
            continue;
        }

        // Помечаем использованные пиксели — так следующая прямая не найдёт
        // их повторно и не создаст дубль со сдвигом на пиксель.
        for (i, &(x, y)) in all.iter().enumerate() {
            if consumed[i] {
                continue;
            }
            let t = (x as f64 - base.x) * d.0 + (y as f64 - base.y) * d.1;
            if (x as f64 * c + y as f64 * s - rho).abs() <= band {
                let on = pieces.iter().any(|&(a, b)| t >= a - 1.0 && t <= b + 1.0);
                if on {
                    consumed[i] = true;
                }
            }
        }

        for (t0, t1) in pieces {
            let len = t1 - t0;
            if len < px.min_line_px as f64 {
                continue;
            }
            let p0 = Pt::new(base.x + d.0 * t0, base.y + d.1 * t0);
            let p1 = Pt::new(base.x + d.0 * t1, base.y + d.1 * t1);
            segs.push(Seg { a: p0, b: p1, votes });
        }
    }

    // Сначала обычное слияние плотных линий, затем сборка пунктира:
    // короткие штрихи с зазорами до ~15 px становятся одной линией.
    let merged = merge_collinear(segs, angle_tol, px.max_gap_px as f64);
    crate::geom::dash_merge(merged, angle_tol, 15.0, 45.0)
}

/// Делит отрезок на куски, где под ним есть «чернила» (скелет).
fn split_line_by_gaps(edges: &GrayImage, a: &Pt, b: &Pt, max_gap: f64, band: f64) -> Vec<Seg> {
    let len = a.dist(b);
    let steps = (len / 0.5).ceil().max(1.0) as usize;
    let n = steps.min(20_000);
    let (w, h) = edges.dimensions();
    let dir = ((b.x - a.x) / len.max(1e-9), (b.y - a.y) / len.max(1e-9));
    let nrm = (-dir.1, dir.0);
    let has = |t: f64| -> bool {
        let p = Pt::new(a.x + dir.0 * t, a.y + dir.1 * t);
        for off in [-band, -band * 0.5, 0.0, band * 0.5, band] {
            let x = (p.x + nrm.0 * off).round();
            let y = (p.y + nrm.1 * off).round();
            if x >= 0.0 && y >= 0.0 && (x as u32) < w && (y as u32) < h {
                if edges.get_pixel(x as u32, y as u32)[0] > 0 {
                    return true;
                }
            }
        }
        false
    };
    let mut pieces = Vec::new();
    let mut start: Option<f64> = None;
    let mut last_hit = 0.0;
    for i in 0..=n {
        let t = (i as f64 / n as f64) * len;
        if has(t) {
            if start.is_none() {
                start = Some(t);
            }
            last_hit = t;
        } else if let Some(s) = start {
            if t - last_hit > max_gap {
                if last_hit - s >= 2.0 {
                    pieces.push(Seg {
                        a: Pt::new(a.x + dir.0 * s, a.y + dir.1 * s),
                        b: Pt::new(a.x + dir.0 * last_hit, a.y + dir.1 * last_hit),
                        votes: 1,
                    });
                }
                start = None;
            }
        }
    }
    if let Some(s) = start {
        if last_hit - s >= 2.0 {
            pieces.push(Seg {
                a: Pt::new(a.x + dir.0 * s, a.y + dir.1 * s),
                b: Pt::new(a.x + dir.0 * last_hit, a.y + dir.1 * last_hit),
                votes: 1,
            });
        }
    }
    pieces
}

/// Удаляет пиксели, уже покрытые найденными прямыми (толщина band px), чтобы
/// цепочки скелета собирали только кривые и служебные обводки.
pub fn residual_skeleton(edges: &GrayImage, segs: &[Seg], band: f64) -> GrayImage {
    let (w, h) = edges.dimensions();
    let mut out = edges.clone();
    if segs.is_empty() {
        return out;
    }
    // Сетка для быстрого поиска близких отрезков.
    let cell = 64.0;
    let cols = ((w as f64 / cell).ceil() as usize).max(1);
    let rows = ((h as f64 / cell).ceil() as usize).max(1);
    let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); cols * rows];
    for (i, s) in segs.iter().enumerate() {
        let x0 = (s.a.x.min(s.b.x) / cell).floor().max(0.0) as usize;
        let x1 = (s.a.x.max(s.b.x) / cell).floor().max(0.0) as usize;
        let y0 = (s.a.y.min(s.b.y) / cell).floor().max(0.0) as usize;
        let y1 = (s.a.y.max(s.b.y) / cell).floor().max(0.0) as usize;
        for y in y0..=y1.min(rows - 1) {
            for x in x0..=x1.min(cols - 1) {
                buckets[y * cols + x].push(i);
            }
        }
    }
    for (x, y, px) in edges.enumerate_pixels() {
        if px[0] == 0 {
            continue;
        }
        let cx = ((x as f64 / cell).floor() as usize).min(cols - 1);
        let cy = ((y as f64 / cell).floor() as usize).min(rows - 1);
        let p = Pt::new(x as f64, y as f64);
        let mut covered = false;
        for &i in &buckets[cy * cols + cx] {
            if pt_line_dist(&p, &segs[i].a, &segs[i].b) <= band {
                covered = true;
                break;
            }
        }
        if covered {
            out.put_pixel(x, y, Luma([0]));
        }
    }
    out
}

const NB8: [(i32, i32); 8] = [
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
    (0, -1),
    (1, -1),
];

/// Обход 8-связного скелета в цепочки (с учётом пересечений как узлов).
pub fn trace_chains(skel: &GrayImage, max_len_px: f64) -> Vec<Vec<Pt>> {
    let (w, h) = skel.dimensions();
    let (wu, hu) = (w as usize, h as usize);
    let raw = skel.as_raw();
    let idx = |x: usize, y: usize| y * wu + x;
    let is_ink = |i: usize| raw[i] > 0;

    let mut deg = vec![0u8; wu * hu];
    let mut pts: Vec<usize> = Vec::new();
    for y in 0..hu {
        for x in 0..wu {
            let i = idx(x, y);
            if !is_ink(i) {
                continue;
            }
            pts.push(i);
            let mut d = 0u8;
            for (dx, dy) in NB8 {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                if nx < 0 || ny < 0 || nx >= wu as i32 || ny >= hu as i32 {
                    continue;
                }
                if is_ink(idx(nx as usize, ny as usize)) {
                    d += 1;
                }
            }
            deg[i] = d;
        }
    }
    if pts.is_empty() {
        return Vec::new();
    }

    let mut visited = vec![false; wu * hu];
    let mut chains: Vec<Vec<Pt>> = Vec::new();
    let max_pts = (max_len_px.max(16.0)) as usize;

    let mut chunks: Vec<Vec<Pt>> = Vec::new();
    let walk = |start: usize, visited: &mut Vec<bool>, jump: bool, chunks: &mut Vec<Vec<Pt>>| -> Vec<Pt> {
        let mut chain = Vec::new();
        let mut cur = start;
        visited[cur] = true;
        let sx = cur % wu;
        let sy = cur / wu;
        chain.push(Pt::new(sx as f64, sy as f64));
        loop {
            let x = cur % wu;
            let y = cur / wu;
            let mut next: Option<usize> = None;
            let mut best_d = 0.0;
            for (dx, dy) in NB8 {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                if nx < 0 || ny < 0 || nx >= wu as i32 || ny >= hu as i32 {
                    continue;
                }
                let ni = idx(nx as usize, ny as usize);
                if !is_ink(ni) || visited[ni] {
                    continue;
                }
                let dist = ((dx * dx + dy * dy) as f64).sqrt();
                if jump {
                    // «Прыжок» через узлы: идём дальше сквозь пересечения,
                    // не останавливаясь на соседях-развилках, если можем.
                    if next.is_none() || dist > best_d {
                        next = Some(ni);
                        best_d = dist;
                    }
                } else if next.is_none() {
                    next = Some(ni);
                    best_d = dist;
                } else {
                    // В развилке предпочитаем продолжение по текущему
                    // направлению — иначе цепочки «срезают» углы и кольца.
                    let px = chain[chain.len().saturating_sub(2)];
                    if chain.len() >= 2 {
                        let rx = x as f64 - px.x;
                        let ry = y as f64 - px.y;
                        let rl = (rx * rx + ry * ry).sqrt().max(1e-9);
                        let cur_score = (dx as f64 * rx / rl) + (dy as f64 * ry / rl);
                        let bx = next.unwrap() % wu;
                        let by = next.unwrap() / wu;
                        let bdx = bx as f64 - x as f64;
                        let bdy = by as f64 - y as f64;
                        let best_score = (bdx * rx / rl) + (bdy * ry / rl);
                        if cur_score > best_score + 0.4 {
                            next = Some(ni);
                            best_d = cur_score;
                        }
                    }
                }
            }
            match next {
                Some(ni) => {
                    visited[ni] = true;
                    chain.push(Pt::new((ni % wu) as f64, (ni / wu) as f64));
                    if chain.len() > max_pts {
                        chunks.push(chain.split_off(0));
                    }
                    // Останавливаемся на узле, если это не стартовая точка.
                    if deg[ni] > 2 && chain.len() > 2 && !jump {
                        break;
                    }
                    cur = ni;
                }
                None => break,
            }
        }
        chain
    };

    // 1. От узлов (deg != 2) — рёбра между узлами.
    for &i in &pts {
        if visited[i] || (deg[i] == 2) {
            continue;
        }
        let mut ch = walk(i, &mut visited, false, &mut chunks);
        if ch.len() > 2 {
            trim_chain(&mut ch);
            chains.push(ch);
        }
    }
    // 2. Остатки — замкнутые кольца (например, обводка окружности).
    for &i in &pts {
        if visited[i] {
            continue;
        }
        let mut ch = walk(i, &mut visited, true, &mut chunks);
        if ch.len() > 2 {
            trim_chain(&mut ch);
            chains.push(ch);
        }
    }
    for c in &mut chunks {
        trim_chain(c);
    }
    chains.extend(chunks);
    chains
}

/// Убирает у цепочки точки, которые топологически «прилипли» к узлу:
/// первый и последний пиксель в развилке не относятся к ребру.
fn trim_chain(ch: &mut Vec<Pt>) {
    while ch.len() > 3 {
        let d01 = ch[0].dist(&ch[1]);
        if d01 <= std::f64::consts::SQRT_2 + 1e-9 {
            ch.remove(0);
        } else {
            break;
        }
        break;
    }
}

/// Упрощение полилинии (Дуглас–Пекер).
pub fn simplify(points: &[Pt], eps: f64) -> Vec<Pt> {
    if points.len() < 3 || eps <= 0.0 {
        return points.to_vec();
    }
    let n = points.len();
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0usize, n - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let mut max_d = -1.0;
        let mut arg = a;
        for i in (a + 1)..b {
            let d = pt_line_dist(&points[i], &points[a], &points[b]);
            if d > max_d {
                max_d = d;
                arg = i;
            }
        }
        if max_d > eps {
            keep[arg] = true;
            stack.push((a, arg));
            stack.push((arg, b));
        }
    }
    points.iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| *p).collect()
}

fn kasa_fit(points: &[Pt]) -> Option<(Pt, f64)> {
    if points.len() < 3 {
        return None;
    }
    let n = points.len() as f64;
    let (mut sx, mut sy, mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0, 0.0, 0.0);
    let (mut sxz, mut syz, mut sz) = (0.0, 0.0, 0.0);
    for p in points {
        let z = p.x * p.x + p.y * p.y;
        sx += p.x;
        sy += p.y;
        sxx += p.x * p.x;
        syy += p.y * p.y;
        sxy += p.x * p.y;
        sxz += p.x * z;
        syz += p.y * z;
        sz += z;
    }
    let mut a = [[sxx, sxy, sx], [sxy, syy, sy], [sx, sy, n]];
    let mut b = [sxz, syz, sz];
    // Гаусс с частичным выбором ведущего элемента.
    for col in 0..3 {
        let mut piv = col;
        for r in (col + 1)..3 {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        if a[piv][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for r in (col + 1)..3 {
            let f = a[r][col] / a[col][col];
            for k in col..3 {
                a[r][k] -= f * a[col][k];
            }
            b[r] -= f * b[col];
        }
    }
    let mut x = [0.0; 3];
    for r in (0..3).rev() {
        let mut acc = b[r];
        for k in (r + 1)..3 {
            acc -= a[r][k] * x[k];
        }
        x[r] = acc / a[r][r];
    }
    let cx = x[0] / 2.0;
    let cy = x[1] / 2.0;
    let r2 = x[2] + cx * cx + cy * cy;
    if !r2.is_finite() || r2 <= 1.0 {
        return None;
    }
    Some((Pt::new(cx, cy), r2.sqrt()))
}

/// Вписывает окружность или дугу в упрощённую полилинию.
pub fn fit_circular(points: &[Pt], full: bool, tol: f64) -> Option<(Pt, f64)> {
    let (c, r) = kasa_fit(points)?;
    let mut max_err = 0.0f64;
    for p in points {
        max_err = max_err.max((p.dist(&c) - r).abs());
    }
    if max_err > tol.max(0.7) {
        return None;
    }
    if full && r < 3.0 {
        return None;
    }
    Some((c, r))
}

/// Полный проход векторизации.
pub fn vectorize(edges: &GrayImage, gray: Option<&GrayImage>, px: &Params) -> VectorData {
    let mut out = VectorData::default();
    let angle_tol = (px.angle_tolerance_deg as f64).to_radians();
    let band_gap = (px.max_gap_px as f64).max(2.0);
    let mut segs = hough_lines(edges, px);

    // Отбраковка мусора: короткие штрихи с малым числом голосов.
    if px.short_stroke_px > 0.0 {
        let before = segs.len();
        segs.retain(|s| !(s.len() < px.short_stroke_px as f64 && s.votes < 8));
        out.dropped_short += before - segs.len();
    }

    // Второй проход: детектор в духе LSD работает по полутону и ловит
    // пунктир, длинные выноски и тонкие контуры, которые Хаф по скелету
    // разменивает на короткие куски. Если передан полутон — используем его.
    if let Some(gray) = gray {
        let lsd = crate::lines::detect(
            gray,
            &crate::lines::LsdParams {
                threshold: px.lsd_threshold,
                min_len: px.lsd_min_len_px as f64,
                max_seeds: 160_000,
            },
        );
        // Сначала добиваем LSD-отрезки до «настоящих» прямых: тонкая линия
        // на скане даёт несколько параллельных цепочек (обе стороны штриха),
        // и без склейки в DXF уходят дубли со сдвигом на 1–2 px.
        let mut lsd_segs: Vec<Seg> = lsd
            .into_iter()
            .map(|r| Seg { a: r.a, b: r.b, votes: 5 })
            .collect();
        lsd_segs = merge_collinear(lsd_segs, angle_tol, 2.0);
        let extra: Vec<Seg> = lsd_segs
            .into_iter()
            .filter(|s| s.len() >= px.min_line_px as f64)
            .collect();
        segs.extend(extra);
    }

    let skel = residual_skeleton(edges, &segs, 2.0);
    let chains = trace_chains(&skel, 12_000.0);
    out.chain_points = chains.iter().map(|c| c.len()).sum();

    let band = band_gap;
    let mut polylines: Vec<Vec<Pt>> = Vec::new();
    let mut local_segs: Vec<Seg> = Vec::new();
    for ch in chains {
        if ch.len() < 3 {
            continue;
        }
        let sp = simplify(&ch, px.simplify_epsilon_px);
        if sp.len() < 2 {
            continue;
        }
        let chord = sp[0].dist(sp.last().unwrap());
        // Кольцо: точки цепочки покрывают почти полный круг и лежат
        // достаточно далеко от центра — раньше этого не знаем, поэтому
        // проверяем по факту вписывания ниже.
        let mut is_ring = ch.len() >= px.min_arc_points && chord < band * 2.0 + 1.0;
        if ch.len() >= 12 {
            if let Some((c, r)) = fit_circular(&sp, is_ring, px.arc_tolerance_px) {
                let (a0, a1, coverage) = arc_angles(&ch, &c);
                // Кольцо определяется по факту обхода, а не по расстоянию
                // между концами: у замкнутой обводки скелет почти всегда
                // расходятся концы на 1..3 px.
                if coverage > 0.93 && r > 5.0 && is_ring_or_near(&ch, &c, r) {
                    out.circles.push(Circle {
                        center: c,
                        radius: r,
                        layer_circle: true,
                    });
                    continue;
                }
                is_ring = false;
                // Дуга должна реально описывать свою цепочку, а не задевать её
                // краем наименьшей окружности.
                if coverage >= 0.5 {
                    out.arcs.push(Arc {
                        center: c,
                        radius: r,
                        start_rad: a0,
                        end_rad: a1,
                    });
                    continue;
                }
            }
        }
        // Почти прямая — в отрезки, иначе в полилинию.
        let mut straight = true;
        for p in sp.iter().skip(1) {
            if pt_line_dist(p, &sp[0], sp.last().unwrap()) > px.arc_tolerance_px as f64 {
                straight = false;
                break;
            }
        }
        if straight {
            for w in sp.windows(2) {
                if w[0].dist(&w[1]) >= px.min_line_px as f64 {
                    local_segs.push(Seg { a: w[0], b: w[1], votes: 1 });
                }
            }
        } else {
            polylines.push(sp);
        }
    }

    // Слияние сегментов со скелета в прямой линии.
    local_segs = merge_collinear(local_segs, (px.angle_tolerance_deg as f64).to_radians(), band);
    segs.extend(local_segs);

    // Нарезка в узлах и финальное объединение.
    let tol = 1.5;
    out.segs = split_at_intersections(segs, tol);
    out.segs = merge_collinear(out.segs, (px.angle_tolerance_deg as f64).to_radians(), 1.0);
    out.segs.sort_by(|a, b| {
        a.a.x
            .partial_cmp(&b.a.x)
            .unwrap()
            .then(a.a.y.partial_cmp(&b.a.y).unwrap())
    });
    out.arcs.sort_by(|a, b| {
        a.center
            .x
            .partial_cmp(&b.center.x)
            .unwrap()
            .then(a.center.y.partial_cmp(&b.center.y).unwrap())
    });
    out.polylines = polylines;
    out
}

/// Замкнута ли цепочка вокруг центра: разрыв между концами не больше
/// нескольких пикселей при ощутимом радиусе.
fn is_ring_or_near(chain: &[Pt], center: &Pt, r: f64) -> bool {
    if chain.len() < 12 || r < 5.0 {
        return false;
    }
    let a = &chain[0];
    let b = &chain[chain.len() - 1];
    let gap = a.dist(b);
    let angular_gap = {
        let a0 = (a.y - center.y).atan2(a.x - center.x);
        let a1 = (b.y - center.y).atan2(b.x - center.x);
        let mut d = (a1 - a0).abs();
        if d > std::f64::consts::PI {
            d = std::f64::consts::TAU - d;
        }
        d
    };
    gap < (r * 0.25).clamp(3.0, 20.0) || angular_gap < 0.35
}

/// Углы дуги плюс доля полного круга, которую цепочка реально покрывает.
/// Для почти замкнутого кольца возвращаем полный оборот (coverage = 1).
fn arc_angles(chain: &[Pt], center: &Pt) -> (f64, f64, f64) {
    let n = chain.len();
    let a0 = (chain[0].y - center.y).atan2(chain[0].x - center.x);
    let a1 = (chain[n - 1].y - center.y).atan2(chain[n - 1].x - center.x);
    // Накопленный угол обхода: сумма приращений между соседними точками,
    // каждое приведено к (-pi, pi].
    let mut swept = 0.0f64;
    let mut prev = (chain[0].y - center.y).atan2(chain[0].x - center.x);
    for p in chain.iter().skip(1) {
        let cur = (p.y - center.y).atan2(p.x - center.x);
        let mut d = cur - prev;
        while d > std::f64::consts::PI {
            d -= std::f64::consts::TAU;
        }
        while d < -std::f64::consts::PI {
            d += std::f64::consts::TAU;
        }
        if d.abs() < 0.9 {
            swept += d;
        }
        prev = cur;
    }
    let chord = chain[0].dist(&chain[n - 1]);
    let closed = swept.abs() > std::f64::consts::PI * 1.7 && chord < 4.0;
    let coverage = if closed { 1.0 } else { (swept.abs() / std::f64::consts::TAU).min(1.0) };
    (a0, a1, coverage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::GrayImage;
    use std::f64::consts::PI;

    fn blank(w: u32, h: u32) -> GrayImage {
        GrayImage::from_pixel(w, h, Luma([0]))
    }

    #[test]
    fn detects_simple_line() {
        let mut img = blank(200, 200);
        for x in 20..180 {
            img.put_pixel(x, 100, Luma([255]));
        }
        let mut p = Params::default();
        p.hough_threshold_ratio = 0.05;
        p.hough_threshold_min = 10;
        let segs = hough_lines(&img, &p);
        assert!(!segs.is_empty());
        let longest = segs.iter().map(|s| s.len()).fold(0.0, f64::max);
        assert!(longest > 120.0, "longest={longest}");
    }

    #[test]
    fn fits_circle() {
        let mut pts = Vec::new();
        for i in 0..200 {
            let a = 2.0 * PI * i as f64 / 200.0;
            pts.push(Pt::new(100.0 + 50.0 * a.cos(), 100.0 + 50.0 * a.sin()));
        }
        let (c, r) = fit_circular(&pts, true, 2.0).expect("fit");
        assert!((c.x - 100.0).abs() < 0.5);
        assert!((r - 50.0).abs() < 0.5);
    }

    #[test]
    fn simplify_straight() {
        let pts: Vec<Pt> = (0..100).map(|i| Pt::new(i as f64, 0.0)).collect();
        assert_eq!(simplify(&pts, 1.0).len(), 2);
    }
}
