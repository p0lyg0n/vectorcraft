//! Illustrator's own data in a PDF-compatible `.ai` file, best effort: the paths type on a path
//! is set along.
//!
//! A `.ai` file is a PDF whose pages draw the art, plus the editor's private data
//! (`PieceInfo` › `Illustrator` › `Private` › `AIPrivateData1…N`): the document in the
//! PostScript-like Illustrator file format, compressed (zstd or Flate) or not. The PDF part
//! draws type on a path glyph by glyph, so the importer rebuilds the path through the glyphs;
//! the private data keeps the path itself (`/AI11Text` objects: the `/Art` path before
//! `/ConfiningPath`, in the format's documented path operators). Only those paths and the
//! artboards are read here; anything this doesn't understand leaves the import as it was.
//!
//! Where each part comes from:
//! - `PieceInfo` (where editors keep private data): the PDF specification (ISO 32000).
//! - The path operators (`m`, `l`/`L`, `c`/`C`, `v`/`V`, `y`/`Y`): Adobe's published Illustrator
//!   file format specification.
//! - The compression marker (`%AI24_ZStandard_Data`, `%AI12_CompressedData`), the split into
//!   `AIPrivateData1…N`, `/AI11Text` … `/ConfiningPath` and the artboards' `PositionPoint1/2`:
//!   observed in files; not in a published specification.
//! - The text document (the characters and their formatting, numeric keys): not read.

use std::io::Read;

use hayro_syntax::object::{Dict, Stream};
use hayro_syntax::page::Page;
use kurbo::{BezPath, ParamCurve, ParamCurveArclen, ParamCurveNearest, Point, Rect, Shape};

/// The most private data read once decompressed (bytes).
const MAX_DATA: usize = 768 << 20;
/// How the uncompressed document text starts.
const DSC_HEADER: &[u8] = b"%!PS-Adobe"; // brand-ok: the DSC header the document text starts with
/// The most private data streams read.
const MAX_STREAMS: usize = 100_000;

/// What is read of a file's private data, in Illustrator's coordinates (y up).
#[derive(Debug, Default)]
pub(crate) struct AiPrivate {
    /// Each artboard: left, top, right, bottom.
    pub artboards: Vec<[f64; 4]>,
    /// The paths text objects are set in or along.
    pub frames: Vec<BezPath>,
}

impl AiPrivate {
    /// The private data of `page`, if it has any this can read.
    pub fn read(page: &Page<'_>) -> Option<Self> {
        let data = decompress(&raw_private_data(page)?)?;
        let ai = Self { artboards: artboards(&data), frames: frames(&data) };
        (!ai.frames.is_empty()).then_some(ai)
    }

    /// The paths in document space for artboard `index` placed at `at` (the imported page's
    /// artboard); None when the artboard isn't known or differs in size.
    pub fn frames_for(&self, index: usize, at: Rect) -> Option<Vec<BezPath>> {
        let &[l, t, r, b] = self.artboards.get(index)?;
        if ((r - l) - at.width()).abs() > 1.0 || ((t - b) - at.height()).abs() > 1.0 {
            return None;
        }
        let xf = kurbo::Affine::new([1.0, 0.0, 0.0, -1.0, at.x0 - l, at.y0 + t]);
        Some(self.frames.iter().map(|p| xf * p.clone()).collect())
    }
}

/// The private data streams of `page`, joined in order.
fn raw_private_data(page: &Page<'_>) -> Option<Vec<u8>> {
    let piece = page.raw().get::<Dict<'_>>(b"PieceInfo")?;
    let private = piece.get::<Dict<'_>>(b"Illustrator")?.get::<Dict<'_>>(b"Private")?; // brand-ok: the PieceInfo key such files carry
    let mut out = vec![];
    for i in 1..=MAX_STREAMS {
        let Some(s) = private.get::<Stream<'_>>(format!("AIPrivateData{i}").as_bytes()) else { break };
        let bytes = s.decoded().ok()?;
        if out.len() + bytes.len() > MAX_DATA {
            return None;
        }
        out.extend_from_slice(&bytes);
    }
    (!out.is_empty()).then_some(out)
}

/// The document text: zstd (`%AI24_ZStandard_Data`), Flate (`%AI12_CompressedData`) or as it is.
fn decompress(raw: &[u8]) -> Option<Vec<u8>> {
    let after = |tag: &[u8]| find(raw, tag).and_then(|i| raw.get(i + tag.len()..));
    let mut out = vec![];
    if let Some(z) = after(b"%AI24_ZStandard_Data") {
        let dec = ruzstd::decoding::StreamingDecoder::new(z).ok()?;
        dec.take(MAX_DATA as u64 + 1).read_to_end(&mut out).ok()?;
    } else if let Some(z) = after(b"%AI12_CompressedData") {
        flate2::read::ZlibDecoder::new(z).take(MAX_DATA as u64 + 1).read_to_end(&mut out).ok()?;
    } else if raw.starts_with(DSC_HEADER) {
        out = raw.to_vec();
    } else {
        return None;
    }
    (out.len() <= MAX_DATA).then_some(out)
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// The words of `text`, without the `%_` the format puts before lines inside dictionaries.
fn words(text: &[u8]) -> impl Iterator<Item = &[u8]> {
    text.split(|b| b.is_ascii_whitespace()).map(|w| w.strip_prefix(b"%_").unwrap_or(w)).filter(|w| !w.is_empty())
}

fn number(w: &[u8]) -> Option<f64> {
    std::str::from_utf8(w).ok()?.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// The artboards (`PositionPoint1` top left, `PositionPoint2` bottom right), in order.
fn artboards(data: &[u8]) -> Vec<[f64; 4]> {
    let mut out = vec![];
    let mut at = 0;
    while let Some(i) = data.get(at..).and_then(|d| find(d, b"(PositionPoint1)")) {
        let p1 = at + i;
        let before = |end: usize| -> Option<(f64, f64)> {
            let w: Vec<&[u8]> = words(data.get(end.saturating_sub(120)..end)?).collect();
            let n = w.len();
            let (x, y) = (number(w.get(n.checked_sub(3)?)?)?, number(w.get(n.checked_sub(2)?)?)?);
            Some((x, y))
        };
        let next = data.get(p1..).and_then(|d| find(d, b"(PositionPoint2)")).map(|j| p1 + j);
        if let (Some((l, t)), Some(Some((r, b)))) = (before(p1), next.map(before)) {
            out.push([l, t, r, b]);
        }
        at = next.unwrap_or(p1) + 16;
        if out.len() > 1000 {
            break;
        }
    }
    // The array is written more than once (document and print settings): the first copy.
    let mut seen = vec![];
    for a in out {
        if seen.first() == Some(&a) {
            break;
        }
        seen.push(a);
    }
    seen
}

/// The `/Art` path of every `/AI11Text` object that has one.
fn frames(data: &[u8]) -> Vec<BezPath> {
    let mut out: Vec<BezPath> = vec![];
    let mut at = 0;
    while let Some(i) = data.get(at..).and_then(|d| find(d, b"/AI11Text :")) {
        let start = at + i;
        at = start + 11;
        // The object's header, then its path, ending at /ConfiningPath (point type has none).
        let Some(seg) = data.get(start..(start + (64 << 10)).min(data.len())) else { break };
        let (Some(art), Some(end)) = (find(seg, b"/Art :"), find(seg, b"/ConfiningPath")) else { continue };
        if art > 200 || end < art {
            continue;
        }
        if let Some(p) = seg.get(art + 6..end).and_then(path)
            && !out.iter().any(|q| same(q, &p))
        {
            out.push(p);
        }
        if out.len() > 100_000 {
            break;
        }
    }
    out
}

fn same(a: &BezPath, b: &BezPath) -> bool {
    a.elements().len() == b.elements().len() && a.bounding_box().union(b.bounding_box()).area() - a.bounding_box().area() < 1e-6
}

/// A path written with the format's path operators (`m`, `l`/`L`, `c`/`C`, `v`/`V`, `y`/`Y`;
/// other operators end the numbers before them).
fn path(text: &[u8]) -> Option<BezPath> {
    let mut bp = BezPath::new();
    let mut nums: Vec<f64> = vec![];
    let mut cur: Option<Point> = None;
    for w in words(text) {
        if let Some(v) = number(w) {
            nums.push(v);
            continue;
        }
        let p = |i: usize| -> Option<Point> { Some(Point::new(*nums.get(i)?, *nums.get(i + 1)?)) };
        match w {
            b"m" if nums.len() >= 2 => {
                let q = p(nums.len() - 2)?;
                bp.move_to(q);
                cur = Some(q);
            }
            b"l" | b"L" if nums.len() >= 2 && cur.is_some() => {
                let q = p(nums.len() - 2)?;
                bp.line_to(q);
                cur = Some(q);
            }
            b"c" | b"C" if nums.len() >= 6 && cur.is_some() => {
                let n = nums.len() - 6;
                let q = p(n + 4)?;
                bp.curve_to(p(n)?, p(n + 2)?, q);
                cur = Some(q);
            }
            b"v" | b"V" if nums.len() >= 4 => {
                let (n, c0) = (nums.len() - 4, cur?);
                let q = p(n + 2)?;
                bp.curve_to(c0, p(n)?, q);
                cur = Some(q);
            }
            b"y" | b"Y" if nums.len() >= 4 && cur.is_some() => {
                let n = nums.len() - 4;
                let q = p(n + 2)?;
                bp.curve_to(p(n)?, q, q);
                cur = Some(q);
            }
            _ => {}
        }
        nums.clear();
    }
    (bp.elements().len() >= 2).then_some(bp)
}

/// Type on a path in `nodes` set along one of `frames` (document space): that path, from where
/// its first glyph lies on it, instead of the path rebuilt through the glyphs.
pub(crate) fn use_frames(nodes: &mut [std::sync::Arc<vectorcraft_doc::Node>], frames: &[BezPath]) {
    use vectorcraft_doc::{NodeKind, TextKind};
    for n in nodes.iter_mut() {
        let node = std::sync::Arc::make_mut(n);
        if let Some(children) = node.children_mut() {
            use_frames(children, frames);
            continue;
        }
        let NodeKind::Text(t) = &mut node.kind else { continue };
        if !matches!(t.kind, TextKind::OnPath { .. }) {
            continue;
        }
        let l = vectorcraft_text::layout(vectorcraft_text::FontDb::global(), t);
        let pts: Vec<Point> = l.glyphs.iter().map(|g| t.xf * g.origin).collect();
        let tol = t.runs.first().map_or(1.0, |r| (r.style.size * 0.1).max(0.5));
        if let Some((path, start)) = frames.iter().find_map(|f| start_on(f, &pts, tol).map(|s| (f, s))) {
            t.kind = TextKind::OnPath { path: vectorcraft_geom::PathData::from_bezpath(path), start, end: None };
            t.xf = kurbo::Affine::IDENTITY;
        }
    }
}

/// Where along `path` (0..1 of its length) the glyph origins `pts` lie, when every one lies on it
/// (within `tol`) in order: the start of type on that path.
pub(crate) fn start_on(path: &BezPath, pts: &[Point], tol: f64) -> Option<f64> {
    let segs: Vec<kurbo::PathSeg> = path.segments().collect();
    let lens: Vec<f64> = segs.iter().map(|s| s.arclen(1e-3)).collect();
    let total: f64 = lens.iter().sum();
    if total <= 0.0 || pts.len() < 2 {
        return None;
    }
    let mut last = -1.0;
    let mut first = None;
    for p in pts {
        let (mut best, mut best_d) = (0.0, f64::INFINITY);
        let mut before = 0.0;
        for (s, len) in segs.iter().zip(&lens) {
            let n = s.nearest(*p, 1e-4);
            let d = n.distance_sq.sqrt();
            if d < best_d {
                best_d = d;
                best = before + s.subsegment(0.0..n.t).arclen(1e-4);
            }
            before += len;
        }
        if best_d > tol || best < last - tol {
            return None;
        }
        last = best;
        first.get_or_insert(best);
    }
    first.map(|f| (f / total).clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_read_with_the_format_operators() {
        let p = path(b"X=\r7 As\r10 20 m\r%_30 20 L\r40 20 50 30 60 40 c\r70 50 80 60 v\rLn").unwrap();
        let els = p.elements();
        assert_eq!(els.len(), 4, "{els:?}");
        assert_eq!(els[0], kurbo::PathEl::MoveTo(Point::new(10.0, 20.0)));
        assert_eq!(els[1], kurbo::PathEl::LineTo(Point::new(30.0, 20.0)));
        assert_eq!(els[3], kurbo::PathEl::CurveTo(Point::new(60.0, 40.0), Point::new(70.0, 50.0), Point::new(80.0, 60.0)));
        assert!(path(b"garbage ( ) / nothing").is_none());
    }

    #[test]
    fn text_frames_and_artboards_are_found_in_the_document_text() {
        let doc =
            b"%_123 456 /RealPointRelToROrigin\r%_ (PositionPoint1) ,\r%_1 /Real (PAR) ,\r%_723 -144 /RealPointRelToROrigin\r%_ (PositionPoint2) ,\r\
            /AI11Text :\r0 /FreeUndo ,\r0 /FrameIndex ,\r3 /StoryIndex ,\r/Art :\rX=\r1 As\r0 0 m\r100 0 L\rN\rX+\r;\r/ConfiningPath ,\r\
            /AI11Text :\r1 /FreeUndo ,\r0 /FrameIndex ,\r4 /StoryIndex ,\r2 /TextAntialiasing ,\r;\r";
        assert_eq!(artboards(doc), [[123.0, 456.0, 723.0, -144.0]]);
        let f = frames(doc);
        assert_eq!(f.len(), 1, "point type has no path");
        assert_eq!(f[0].bounding_box(), Rect::new(0.0, 0.0, 100.0, 0.0));
        let ai = AiPrivate { artboards: artboards(doc), frames: f };
        // Artboard 600 × 600 placed at (10, 20): y flips about its top.
        let placed = ai.frames_for(0, Rect::new(10.0, 20.0, 610.0, 620.0)).unwrap();
        assert_eq!(placed[0].bounding_box(), Rect::new(-113.0, 476.0, -13.0, 476.0));
        assert!(ai.frames_for(0, Rect::new(0.0, 0.0, 100.0, 100.0)).is_none(), "another size");
    }

    #[test]
    fn glyph_origins_on_a_path_give_its_start() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((100.0, 0.0));
        let pts = [Point::new(25.0, 0.1), Point::new(35.0, -0.1), Point::new(50.0, 0.0)];
        assert!((start_on(&p, &pts, 0.5).unwrap() - 0.25).abs() < 1e-6);
        assert!(start_on(&p, &[Point::new(25.0, 5.0), Point::new(30.0, 0.0)], 0.5).is_none(), "off the path");
        assert!(start_on(&p, &[Point::new(50.0, 0.0), Point::new(25.0, 0.0)], 0.5).is_none(), "backwards");
    }
}
