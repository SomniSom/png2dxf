//! Пиксельная геометрия: точки, отрезки, поиск пересечений, слияние
//! коллинеарных сегментов и нарезка линий в узлах (аналог shapely
//! linemerge/unary_union). Всё в O(n log n)..O(n²) на локальных индексах.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pt {
    pub x: f64,
    pub y: f64,
}

impl Pt {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
    pub fn dist(&self, o: &Pt) -> f64 {
        ((self.x - o.x).powi(2) + (self.y - o.y).powi(2)).sqrt()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Seg {
    pub a: Pt,
    pub b: Pt,
    pub votes: u32,
}

impl Seg {
    pub fn len(&self) -> f64 {
        self.a.dist(&self.b)
    }

    /// Единичный направляющий вектор a→b.
    pub fn dir(&self) -> (f64, f64) {
        let l = self.len().max(1e-12);
        ((self.b.x - self.a.x) / l, (self.b.y - self.a.y) / l)
    }

    pub fn angle(&self) -> (f64, f64) {
        self.dir()
    }
}

#[inline]
pub fn lerp(a: &Pt, b: &Pt, t: f64) -> Pt {
    Pt::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
}

/// Расстояние от точки до бесконечной прямой AB.
pub fn pt_line_dist(p: &Pt, a: &Pt, b: &Pt) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let l = (dx * dx + dy * dy).sqrt();
    if l < 1e-12 {
        return p.dist(a);
    }
    ((p.x - a.x) * dy - (p.y - a.y) * dx).abs() / l
}

/// Расстояние от точки до отрезка.
pub fn dist_point_segment(p: &Pt, a: &Pt, b: &Pt) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let l2 = dx * dx + dy * dy;
    if l2 < 1e-12 {
        return p.dist(a);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / l2).clamp(0.0, 1.0);
    (p.x - (a.x + t * dx)).hypot(p.y - (a.y + t * dy))
}

#[inline]
fn cross(a: (f64, f64), b: (f64, f64)) -> f64 {
    a.0 * b.1 - a.1 * b.0
}

/// Пересечение бесконечных прямых AB и CD в параметрах t (по AB) и u (по CD).
pub fn line_intersection(a: &Pt, b: &Pt, c: &Pt, d: &Pt) -> Option<(f64, f64)> {
    let r = (b.x - a.x, b.y - a.y);
    let s = (d.x - c.x, d.y - c.y);
    let denom = cross(r, s);
    if denom.abs() < 1e-12 {
        return None;
    }
    let qp = (c.x - a.x, c.y - a.y);
    let t = cross(qp, s) / denom;
    let u = cross(qp, r) / denom;
    Some((t, u))
}

/// Объединение сегментов, лежащих на одной прямой и перекрывающихся/почти
/// стыкующихся. Возвращает упорядоченный список без повторов.
pub fn merge_collinear(segs: Vec<Seg>, angle_tol_rad: f64, gap_px: f64) -> Vec<Seg> {
    if segs.len() < 2 {
        return segs;
    }
    // Сначала длинные: они задают «ось» линии.
    let mut order: Vec<usize> = (0..segs.len()).collect();
    order.sort_by(|&i, &j| segs[j].len().partial_cmp(&segs[i].len()).unwrap());

    let sin_tol = angle_tol_rad.sin().max(1e-6);
    let mut taken = vec![false; segs.len()];
    let mut out: Vec<Seg> = Vec::with_capacity(segs.len());

    for &i in &order {
        if taken[i] {
            continue;
        }
        taken[i] = true;
        let mut dir = segs[i].dir();
        let origin = segs[i].a;
        // Границы вдоль оси в координатах проекции.
        let mut lo = 0.0f64;
        let mut hi = segs[i].len();
        let mut votes = segs[i].votes;

        loop {
            let mut best: Option<(usize, f64, f64)> = None;
            for j in 0..segs.len() {
                if taken[j] {
                    continue;
                }
                let s = segs[j];
                let sd = s.dir();
                if cross(dir, sd).abs() > sin_tol {
                    continue;
                }
                // Перпендикулярные расстояния концов до оси прямой
                // (бесконечной, не отрезка — иначе концы, лежащие за
                // пределами текущего сегмента, получают «расстояние до
                // конечной точки» и ничего не сливается).
                let axis_b = Pt::new(origin.x + dir.0, origin.y + dir.1);
                let d1 = pt_line_dist(&s.a, &origin, &axis_b);
                let d2 = pt_line_dist(&s.b, &origin, &axis_b);
                if d1.max(d2) > gap_px * 2.0 + 2.0 {
                    continue;
                }
                let t1 = (s.a.x - origin.x) * dir.0 + (s.a.y - origin.y) * dir.1;
                let t2 = (s.b.x - origin.x) * dir.0 + (s.b.y - origin.y) * dir.1;
                let (slo, shi) = if t1 <= t2 { (t1, t2) } else { (t2, t1) };
                // Зазор между отрезками вдоль оси.
                let join = if shi < lo { lo - shi } else if slo > hi { slo - hi } else { 0.0 };
                if join > gap_px {
                    continue;
                }
                let gain = shi.max(hi) - slo.min(lo);
                if best.map_or(true, |(_, _, g)| gain > g) {
                    best = Some((j, slo, shi));
                }
            }
            match best {
                Some((j, slo, shi)) => {
                    taken[j] = true;
                    votes = votes.max(segs[j].votes);
                    lo = lo.min(slo);
                    hi = hi.max(shi);
                }
                None => break,
            }
        }
        let a = Pt::new(origin.x + dir.0 * lo, origin.y + dir.1 * lo);
        let b = Pt::new(origin.x + dir.0 * hi, origin.y + dir.1 * hi);
        let _ = &mut dir;
        out.push(Seg { a, b, votes });
    }
    out
}

/// Пространственный индекс по сетке — чтобы искать пересечения локально.
struct Grid {
    cell: f64,
    min_x: f64,
    min_y: f64,
    cols: usize,
    rows: usize,
}

impl Grid {
    fn new(bbox: (f64, f64, f64, f64), cell: f64) -> Self {
        let (x0, y0, x1, y1) = bbox;
        let cols = (((x1 - x0) / cell).ceil() as usize).max(1).min(4096);
        let rows = (((y1 - y0) / cell).ceil() as usize).max(1).min(4096);
        Self { cell, min_x: x0, min_y: y0, cols, rows }
    }

    fn cells(&self, s: &Seg) -> Vec<usize> {
        let x0 = ((s.a.x.min(s.b.x) - self.min_x) / self.cell).floor();
        let x1 = ((s.a.x.max(s.b.x) - self.min_x) / self.cell).floor();
        let y0 = ((s.a.y.min(s.b.y) - self.min_y) / self.cell).floor();
        let y1 = ((s.a.y.max(s.b.y) - self.min_y) / self.cell).floor();
        let x0 = (x0 as i64).clamp(0, self.cols as i64 - 1);
        let x1 = (x1 as i64).clamp(0, self.cols as i64 - 1);
        let y0 = (y0 as i64).clamp(0, self.rows as i64 - 1);
        let y1 = (y1 as i64).clamp(0, self.rows as i64 - 1);
        let mut out = Vec::new();
        for y in y0..=y1 {
            for x in x0..=x1 {
                out.push(y as usize * self.cols + x as usize);
            }
        }
        out
    }
}

/// Разрезает отрезки в точках пересечения (X- и T-соединения), возвращает
/// набор уникальных рёбер — так DXF-геометрия «сшивается» по узлам, как после
/// shapely linemerge + polygonize.
pub fn split_at_intersections(segs: Vec<Seg>, tol: f64) -> Vec<Seg> {
    let n = segs.len();
    if n < 2 {
        return segs;
    }
    if n > 200_000 {
        // защита от катастрофы на гигантских сканах — не дробим
        return segs;
    }
    let mut bbox = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    let mut total_len = 0.0;
    for s in &segs {
        bbox.0 = bbox.0.min(s.a.x).min(s.b.x);
        bbox.1 = bbox.1.min(s.a.y).min(s.b.y);
        bbox.2 = bbox.2.max(s.a.x).max(s.b.x);
        bbox.3 = bbox.3.max(s.a.y).max(s.b.y);
        total_len += s.len();
    }
    let cell = ((total_len / n as f64) * 2.0).max(32.0).min(1024.0);
    let grid = Grid::new(bbox, cell);
    let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); grid.cols * grid.rows];
    for (i, s) in segs.iter().enumerate() {
        for c in grid.cells(s) {
            buckets[c].push(i as u32);
        }
    }

    let mut cuts: Vec<Vec<f64>> = vec![Vec::new(); n];
    let mut checked: std::collections::HashSet<u64> = std::collections::HashSet::new();
    for bucket in &buckets {
        if bucket.len() < 2 {
            continue;
        }
        for w in 0..bucket.len() {
            for v in (w + 1)..bucket.len() {
                let i = bucket[w] as usize;
                let j = bucket[v] as usize;
                let key = if i < j {
                    (i as u64) << 32 | j as u64
                } else {
                    (j as u64) << 32 | i as u64
                };
                if !checked.insert(key) {
                    continue;
                }
                let si = segs[i];
                let sj = segs[j];
                let Some((t, u)) = line_intersection(&si.a, &si.b, &sj.a, &sj.b) else {
                    continue;
                };
                let li = si.len().max(1e-9);
                let lj = sj.len().max(1e-9);
                let pi = Pt::new(si.a.x + (si.b.x - si.a.x) * t, si.a.y + (si.b.y - si.a.y) * t);
                let pj = Pt::new(sj.a.x + (sj.b.x - sj.a.x) * u, sj.a.y + (sj.b.y - sj.a.y) * u);
                if pi.dist(&pj) > tol * 2.0 {
                    continue;
                }
                // Расстояние от точки пересечения до отрезка вдоль его оси.
                let ext_i = if t < 0.0 { -t * li } else if t > 1.0 { (t - 1.0) * li } else { 0.0 };
                let ext_j = if u < 0.0 { -u * lj } else if u > 1.0 { (u - 1.0) * lj } else { 0.0 };
                if ext_i > tol || ext_j > tol {
                    continue;
                }
                let ti = (t * li) / li;
                let eps_i = tol / li;
                let eps_j = tol / lj;
                if ti > eps_i && ti < 1.0 - eps_i {
                    cuts[i].push(ti);
                }
                if u > eps_j && u < 1.0 - eps_j {
                    cuts[j].push(u);
                }
            }
        }
    }

    let mut out = Vec::with_capacity(n + n / 4);
    for (i, s) in segs.iter().enumerate() {
        let mut c = std::mem::take(&mut cuts[i]);
        if c.is_empty() {
            out.push(*s);
            continue;
        }
        c.sort_by(|a, b| a.partial_cmp(b).unwrap());
        c.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        let mut prev = 0.0;
        for t in c {
            if t - prev > 1e-9 {
                out.push(Seg { a: lerp(&s.a, &s.b, prev), b: lerp(&s.a, &s.b, t), votes: s.votes });
            }
            prev = t;
        }
        if 1.0 - prev > 1e-9 {
            out.push(Seg { a: lerp(&s.a, &s.b, prev), b: s.b, votes: s.votes });
        }
    }
    out.retain(|s| s.len() > 1e-6);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_cross() {
        let segs = vec![
            Seg { a: Pt::new(0.0, 0.0), b: Pt::new(100.0, 0.0), votes: 1 },
            Seg { a: Pt::new(50.0, -50.0), b: Pt::new(50.0, 50.0), votes: 1 },
        ];
        let out = split_at_intersections(segs, 1.0);
        assert_eq!(out.len(), 4);
    }

    #[test]
    fn splits_t_joint() {
        let segs = vec![
            Seg { a: Pt::new(0.0, 0.0), b: Pt::new(100.0, 0.0), votes: 1 },
            Seg { a: Pt::new(50.0, 0.0), b: Pt::new(50.0, 40.0), votes: 1 },
        ];
        let out = split_at_intersections(segs, 1.0);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn merges_collinear() {
        let segs = vec![
            Seg { a: Pt::new(0.0, 0.0), b: Pt::new(40.0, 0.0), votes: 1 },
            Seg { a: Pt::new(42.0, 0.5), b: Pt::new(90.0, 0.0), votes: 1 },
        ];
        let out = merge_collinear(segs, 0.03, 4.0);
        assert_eq!(out.len(), 1);
        assert!((out[0].len() - 90.0).abs() < 1.0);
    }
}

/// Собирает прерывистую (пунктирную) линию: короткие коллинеарные штрихи,
/// идущие друг за другом с зазорами, объединяются в один отрезок.
///
/// Отличие от `merge_collinear`: допускаются длинные зазоры (до `max_gap_len`),
/// но не длиннее самого штриха — иначе «сшивались» бы далёкие элементы чертежа.
pub fn dash_merge(segs: Vec<Seg>, angle_tol_rad: f64, max_gap_len: f64, max_short: f64) -> Vec<Seg> {
    if segs.len() < 2 {
        return segs;
    }
    let sin_tol = angle_tol_rad.sin().max(1e-6);
    let mut order: Vec<usize> = (0..segs.len()).collect();
    order.sort_by(|&i, &j| segs[j].len().partial_cmp(&segs[i].len()).unwrap());
    let mut used = vec![false; segs.len()];
    let mut out: Vec<Seg> = Vec::with_capacity(segs.len());

    for &i in &order {
        if used[i] {
            continue;
        }
        used[i] = true;
        let (mut a, mut b) = (segs[i].a, segs[i].b);
        let mut votes = segs[i].votes;
        loop {
            let len = a.dist(&b);
            if len < 1e-9 {
                break;
            }
            let d = ((b.x - a.x) / len, (b.y - a.y) / len);
            let mut best: Option<(usize, f64, f64)> = None;
            for j in 0..segs.len() {
                if used[j] {
                    continue;
                }
                let s = segs[j];
                if s.len() > max_short && s.len() > len * 0.5 {
                    continue;
                }
                let sl = s.len().max(1e-9);
                let sd = ((s.b.x - s.a.x) / sl, (s.b.y - s.a.y) / sl);
                if (d.0 * sd.1 - d.1 * sd.0).abs() > sin_tol {
                    continue;
                }
                if d.0 * sd.0 + d.1 * sd.1 < 0.0 {
                    continue;
                }
                // Поперечное смещение концов штриха от оси линии.
                let p_ax = pt_line_dist(&s.a, &a, &b);
                let p_bx = pt_line_dist(&s.b, &a, &b);
                if p_ax.max(p_bx) > max_gap_len * 0.6 {
                    continue;
                }
                let t1 = (s.a.x - a.x) * d.0 + (s.a.y - a.y) * d.1;
                let t2 = (s.b.x - a.x) * d.0 + (s.b.y - a.y) * d.1;
                let (lo, hi) = if t1 <= t2 { (t1, t2) } else { (t2, t1) };
                let join = if lo > len { lo - len } else if hi < 0.0 { -hi } else { 0.0 };
                if join > max_gap_len {
                    continue;
                }
                let gain = hi.max(len) - lo.min(0.0);
                if best.map_or(true, |(_, _, g)| gain > g) {
                    best = Some((j, lo, hi));
                }
            }
            match best {
                Some((j, lo, hi)) => {
                    used[j] = true;
                    votes = votes.max(segs[j].votes);
                    // Расширяем отрезок вдоль его собственной оси.
                    let o = Pt::new(a.x, a.y);
                    let new_lo = lo.min(0.0);
                    let new_hi = hi.max(len);
                    a = Pt::new(o.x + d.0 * new_lo, o.y + d.1 * new_lo);
                    b = Pt::new(o.x + d.0 * new_hi, o.y + d.1 * new_hi);
                }
                None => break,
            }
        }
        out.push(Seg { a, b, votes });
    }
    out
}
