# png2dxf — raster drawing to DXF vectorizer

A Rust service that takes a scanned or exported drawing (PNG, JPG, TIFF, BMP, WebP)
and returns DXF R2013 (AC1027) with lines, arcs, circles and polylines in real
millimetres. Ships an HTTP API and a CLI in one Docker image — no OpenCV, no Python,
no external services.

- [Русская версия](README.ru.md)
- [Quick start](#quick-start)
- [API](#api)
- [How it works](#how-it-works)

## Quick start

```bash
docker build -t png2dxf:latest .

docker run -d --rm -p 8080:8080 --name png2dxf png2dxf:latest

curl -s http://localhost:8080/healthz
# {"max_body_bytes":67108864,"status":"ok","version":"0.1.0","workers":N}

curl -s -o out.dxf \
  -F "file=@scan.png" \
  -F 'params={"scale":{"px1":[120,45],"px2":[340,45],"mm":5000}}' \
  http://localhost:8080/v1/vectorize
```

The response is the DXF itself (`Content-Type: application/dxf`); statistics are in
the `X-Stats` header (entity counts, scale, processing time).

### CLI without the server

```bash
docker run --rm -v "$PWD:/data" png2dxf:latest /data/scan.png /data/scan.dxf --dpi 300
docker run --rm -v "$PWD:/data" png2dxf:latest /data/scan.png /data/scan.dxf \
    --scale 120 45 340 45 5000
```

## API

| Method | Path | Purpose |
|---|---|---|
| POST | `/v1/vectorize` | one file → one DXF or ZIP |
| POST | `/v1/vectorize/batch` | several `file` fields → ZIP |
| POST | `/v1/vectorize` with `Content-Type: application/json` | `{"image_base64": "...", "params": {...}}` |
| GET | `/healthz` | liveness probe |
| GET | `/v1/schema` | defaults and description of every parameter |
| OPTIONS | * | CORS preflight |

Parameters travel in the multipart field `params` (JSON) or under the `params` key
of a JSON request. All fields are optional.

```json
{
  "scale": { "px1": [120, 45], "px2": [340, 45], "mm": 5000 },
  "dpi": 300,
  "blur_sigma": 1.2,
  "adaptive_radius": 12,
  "adaptive_offset": 8,
  "thin": true,
  "hough_threshold_ratio": 0.22,
  "hough_band_px": 2.5,
  "min_line_px": 15,
  "max_gap_px": 6,
  "angle_tolerance_deg": 1.2,
  "min_line_mm": 0,
  "arc_tolerance_px": 2.0,
  "simplify_epsilon_px": 0.8,
  "short_stroke_px": 4,
  "flip_y": true,
  "include_debug": false,
  "color_boost": true,
  "drop_text": false,
  "ocr": true,
  "ocr_lang": "rus",
  "ocr_min_conf": 35,
  "ocr_upscale": 2,
  "layers": {
    "lines": "Lines",
    "circles": "Circles",
    "arcs": "Arcs",
    "polylines": "Polylines"
  },
  "max_pixels": 120000000
}
```

### Scale

A drawing without a scale is useless for estimates, so the service **never guesses**
it.

* `scale` — two points of a known dimension line in pixels plus its real length in mm:
  `scale = mm / |px2 − px1|`. The preferred option; accuracy depends only on the
  points you supply.
* `dpi` — scale from scan resolution: `mm/px = 25.4 / dpi`. Fast path when the scan was
  made at a known DPI without geometric resampling.
* If neither is given the request fails with `422` / "scale is not set".

Y is flipped (`flip_y`) because raster Y grows downwards while CAD Y grows upwards.
Arc angles are mirrored and swapped when flipping.

### Error codes

| Code | `error` | When |
|---|---|---|
| 413 | `payload_too_large`, `too_large`, `too_many_files` | body over `MAX_BODY_MB`, `max_pixels` exceeded, more than `MAX_BATCH` files |
| 415 | `bad_content_type` | no `multipart/form-data` with a boundary |
| 422 | `bad_image`, `bad_json`, `bad_params`, `no_file`, `no_geometry` | file cannot be decoded, malformed JSON, missing `file` field, empty geometry |
| 500 | `internal` | internal processing error |

## How it works

```
PNG/JPG/TIFF ─ decode (image crate)
  → grayscale (RGBA composited over white, optional colour boost)
  → Gaussian blur (σ from parameters)
  → adaptive binarization via integral image
  → morphological cleanup (open+close 1px)
  → text mask (small glyph-like components) — used for OCR, optionally removed
  → Zhang–Suen thinning (1px)
  → line recovery with a Hough transform over (rho, theta), NMS and parallel
    voting across bands (rayon)
  → LSD-style gradient line tracing on the blurred grayscale
  → residual skeleton: already recovered lines are subtracted
  → skeleton chain tracing (8-connectivity, direction-priority walking)
  → Douglas–Peucker simplification
  → circle/arc fitting by algebraic least squares (Kasa) + coverage check
  → dash merging, splitting in X/T nodes (shapely unary_union analogue),
    collinear merging
  → OCR of the text mask with Tesseract (two passes + block pass)
  → DXF R2013: LINE / ARC / CIRCLE / LWPOLYLINE / TEXT, layers, Y flip
```

Key differences from the classic OpenCV pipeline:

* **No shapely**: intersections are found through a spatial grid
  (`split_at_intersections`) and collinear merging is implemented in-house
  (`merge_collinear`). Lines are split at nodes, so contours join like CAD lines.
* **No OpenCV**: binarization, skeletonization and the Hough transform are pure Rust;
  the image does not pull `libopencv` (~300 MB) or a C++ toolchain.
* **Peak-based detection, not total-ink thresholds**: every ink pixel votes for
  `(rho, theta)`, the accumulator peak is expanded back into the pixels of a
  `hough_band_px` strip, sorted along the line, and gaps up to `max_gap_px` are
  bridged. On real scans this raised the longest recovered line from 13 to 545 px.
* **LSD-style tracing** (`src/lines.rs`): gradient orientation plus level-following
  growth, so thin and dashed lines survive where the Hough transform yields fragments.
* **Arcs instead of 100-point polylines**: skeleton chains are validated as circles
  (Kasa residual + covered angle), so rings become `CIRCLE` and partial arcs `ARC`.
* **Garbage is filtered twice**: by length/votes when selecting segments and by ink
  density along the recovered line.

## DXF layers

Layer defaults are:

| Layer | Colour | Contents |
|---|---|---|
| `Lines` | 7 | LINE entities |
| `Circles` | 1 | CIRCLE entities |
| `Arcs` | 3 | ARC entities |
| `Polylines` | 5 | LWPOLYLINE entities |
| `Text` | 2 | TEXT entities produced by OCR |
| `Skeleton` | 8 | debug skeleton, only with `include_debug` |

Every name is overridable through the `layers` object, so any naming can be set
per request (see the JSON example above). All coordinates are in mm,
`$INSUNITS = 4`.

## Performance

Measured on 2 vCPU (`WORKERS=2`):

| Input | Time |
|---|---|
| plan 1600×1200 PNG, 22 KB | ≈115 ms |
| drawing 1350×1065 PNG, 500 KB | ≈120 ms |
| same drawing ×3 (4050×3195, ~30 MP) | ≈900 ms |
| OCR adds | ≈1–2 s per sheet |

Idle container memory is about 80 MB. Hough voting is parallelised across image bands
and batch processing across files (`rayon`).

## Limitations

* **OCR works but is imperfect.** Tesseract (`-l rus`, two passes + block mode) writes
  labels to the `Text` layer. On GOST drafting fonts some characters are confused
  (`Ф`→`$`, `В`→`8`) and dense notes are read column by column. Tune with
  `ocr` / `ocr_min_conf` / `ocr_upscale`; without `tesseract` in the image the layer is
  empty and geometry is unaffected.
* **Semantic layers** (`Walls`/`Dimensions`/`Axes`) are not produced: that needs
  semantics, not geometry. Layers are grouped by primitive type.
* **Arcs** are recovered for clean scans; on smeared arcs with breaks some remain
  polylines (which is more honest than "almost a circle").
* **Grids and hatching** produce many short segments. Use `min_line_px`,
  `short_stroke_px` and `min_line_mm`.
* **A4+ at 600 dpi** hits `MAX_BODY_MB` and memory limits: raise `MAX_BODY_MB`,
  `WORKERS` and `max_pixels` for your hardware.
* PDF is not accepted: rasterise the page first (e.g. `pdftoppm -r 300`).

## Configuration

| Variable | Default | Meaning |
|---|---|---|
| `LISTEN_ADDR` | `0.0.0.0:8080` | server address |
| `WORKERS` | CPU count | rayon thread pool size |
| `MAX_BODY_MB` | `64` | multipart body limit |
| `MAX_BATCH` | `8` | files per batch |
| `MODE` | `serve` | `serve` runs the API when no arguments are given |

For Kubernetes: `--memory=2g --cpus=2`, `readinessProbe`/`livenessProbe` on
`/healthz`, and an ingress body limit with a timeout of at least 30 s for large scans.


## What's new in 2.0

* **Dashed lines via chains**: dashes are grouped by orientation and fitted
  into a single line by least squares over their centers (`geom::dash_chains`).
* **Pale colored lines**: saturation boosting now targets light tones
  (`grayscale_colored`), so pale pink parcel boundaries reach binarization.
* **Note paragraphs are masked out** of the raster before detection
  (`drop_block_areas`): blocks of 3+ lines no longer produce artifact segments.
* **Subpixel refinement** (`refine_segments`): every segment is shifted onto the
  actual stroke axis via an intensity profile — better dimensional accuracy.
* **Synthetic ground truth for tests**: `png2dxf gen-test-image out.png 1600 1100`
  renders a procedural sheet with known geometry, so no licensed scans are needed.


## Output formats

`format` in the JSON params (or `--format`, or the output file extension in the CLI):
`dxf` (default), `svg`, `obj`.

* **DXF** — the main format, R2013, `Standard` style with a `Liberation Sans`
  TTF (Cyrillic renders in LibreCAD/QCAD/nanoCAD), text written as `\U+XXXX`.
* **SVG** — opens in any browser and in Inkscape with no setup, system font,
  Y axis pointing down like the raster.
* **OBJ** — the Blender bridge: `File → Import → Wavefront (.obj)`, geometry as
  lines (`l`), coordinates in mm. OBJ has no text support — annotations are
  written as comments at the top of the file.

## Development

```bash
cargo test             # unit tests: geometry, DXF, Hough, LSD, ZIP, multipart, base64, OCR
cargo build --release
```

OCR requires the `tesseract` binary with the `rus` language pack at runtime; it is
installed in the Docker image.
