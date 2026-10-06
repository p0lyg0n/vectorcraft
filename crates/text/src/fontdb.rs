//! Font database: bundled OFL fonts, user fonts, the installed system fonts (cataloged once, loaded
//! on demand), outline cache.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use kurbo::BezPath;
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::raw::FileRef;
use skrifa::string::StringId;
use skrifa::{GlyphId, MetadataProvider};

/// The family used when a requested family is unknown (Illustrator's Myriad Pro analogue).
pub const FALLBACK_FAMILY: &str = "Source Sans 3";

static BUNDLED: &[&[u8]] = &[
    include_bytes!("../../../assets/fonts/SourceSans3-Regular.ttf"),
    include_bytes!("../../../assets/fonts/SourceSans3-Semibold.ttf"),
    include_bytes!("../../../assets/fonts/SourceSans3-Bold.ttf"),
    include_bytes!("../../../assets/fonts/SourceSans3-It.ttf"),
    include_bytes!("../../../assets/fonts/SourceSerif4-Regular.ttf"),
    include_bytes!("../../../assets/fonts/Inter-Regular.ttf"),
    include_bytes!("../../../assets/fonts/Inter-Medium.ttf"),
    include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf"),
    include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf"),
    include_bytes!("../../../assets/fonts/ShipporiMincho-Regular.ttf"),
];

enum FontBytes {
    Static(&'static [u8]),
    Owned(Arc<Vec<u8>>),
}

/// One loaded font face.
pub struct FontFace {
    id: u32,
    /// Typographic family name (e.g. "Source Sans 3").
    pub family: String,
    /// Typographic style name (e.g. "Semibold", "Italic").
    pub style: String,
    /// usWeightClass-style weight (400 = regular).
    pub weight: f32,
    pub italic: bool,
    bytes: FontBytes,
    index: u32,
    /// The file the face was read from (cataloged system fonts); `None` for the bundled fonts and
    /// fonts added as bytes.
    path: Option<std::path::PathBuf>,
    pub(crate) upem: f64,
    /// Ascender in font units (positive = up).
    pub(crate) ascent: f64,
    /// Descender in font units (positive = down).
    pub(crate) descent: f64,
    /// Cap height and x height in font units (estimated from the ascent when the font has no OS/2
    /// values).
    pub(crate) cap_height: f64,
    pub(crate) x_height: f64,
    pub(crate) shaper: harfrust::ShaperData,
}

impl std::fmt::Debug for FontFace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FontFace({} {})", self.family, self.style)
    }
}

impl FontFace {
    pub(crate) fn data(&self) -> &[u8] {
        match &self.bytes {
            FontBytes::Static(b) => b,
            FontBytes::Owned(v) => v.as_slice(),
        }
    }
    pub(crate) fn skrifa(&self) -> Option<skrifa::FontRef<'_>> {
        skrifa::FontRef::from_index(self.data(), self.index).ok()
    }
    pub(crate) fn hb(&self) -> Option<harfrust::FontRef<'_>> {
        harfrust::FontRef::from_index(self.data(), self.index).ok()
    }
    /// The face's index in its font file (collections hold several).
    pub(crate) fn index(&self) -> u32 {
        self.index
    }
    /// Unique id of this face within the process.
    pub fn id(&self) -> u32 {
        self.id
    }
    /// Does the face map `c` to a glyph?
    pub fn covers(&self, c: char) -> bool {
        self.skrifa().is_some_and(|f| f.charmap().map(c).is_some())
    }
    /// Units per em.
    pub fn units_per_em(&self) -> f64 {
        self.upem
    }
    /// (ascent, descent) in font units, both positive.
    pub fn vertical_metrics(&self) -> (f64, f64) {
        (self.ascent, self.descent)
    }
    /// Every mapped character and its glyph id, sorted by code point (the Glyphs panel).
    pub fn chars(&self) -> Vec<(char, u32)> {
        let Some(f) = self.skrifa() else { return vec![] };
        let mut v: Vec<(char, u32)> = f.charmap().mappings().filter_map(|(cp, g)| char::from_u32(cp).map(|c| (c, g.to_u32()))).collect();
        v.sort_unstable_by_key(|x| x.0);
        v.dedup_by_key(|x| x.0);
        v
    }
    /// Advance width of glyph `gid` in font units.
    pub fn advance(&self, gid: u32) -> f64 {
        self.skrifa()
            .and_then(|f| f.glyph_metrics(Size::unscaled(), LocationRef::default()).advance_width(GlyphId::new(gid)))
            .map(|a| a as f64)
            .unwrap_or(self.upem * 0.5)
    }
    /// Glyph id for `c` (0 = .notdef).
    pub fn glyph_for(&self, c: char) -> u32 {
        self.skrifa().and_then(|f| f.charmap().map(c)).map(|g| g.to_u32()).unwrap_or(0)
    }
    /// The file the face was read from (cataloged system fonts).
    pub fn path(&self) -> Option<&std::path::Path> {
        self.path.as_deref()
    }
    /// The bytes of the face's font file (a collection holds several faces).
    pub fn file_data(&self) -> &[u8] {
        self.data()
    }
    /// The embedding permissions of the OS/2 table (`fsType`; 0, installable, when it has none).
    pub fn fs_type(&self) -> u16 {
        use skrifa::raw::TableProvider;
        self.skrifa().and_then(|f| f.os2().ok()).map_or(0, |t| t.fs_type())
    }
    /// May the font file be copied along with a document (its licence doesn't restrict embedding)?
    pub fn embeddable(&self) -> bool {
        self.fs_type() & 0x000f != 0x0002
    }
    /// The face's index in its font file ([`Self::file_data`]; collections hold several).
    pub fn face_index(&self) -> u32 {
        self.index
    }
}

/// What a family is for, as font lists group families.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FontScript {
    /// Latin and other alphabets.
    #[default]
    Latin,
    /// Symbols, dingbats and emoji.
    Symbol,
    /// Japanese (kana and kanji).
    Japanese,
    /// Chinese or Korean.
    OtherCjk,
}

impl FontScript {
    /// From the OS/2 table's `ulCodePageRange1` bits: 17 Japanese; 18 and 20 Chinese; 19 and 21
    /// Korean; 31 symbol.
    pub(crate) fn from_code_pages(cp1: u32) -> Self {
        if cp1 & (1 << 17) != 0 {
            Self::Japanese
        } else if cp1 & ((1 << 18) | (1 << 19) | (1 << 20) | (1 << 21)) != 0 {
            Self::OtherCjk
        } else if cp1 & (1 << 31) != 0 {
            Self::Symbol
        } else {
            Self::Latin
        }
    }
}

/// One installed family found by the system font scan: its name and the file of each style.
#[derive(Debug, Default)]
struct CatalogFamily {
    /// What the family is for.
    script: FontScript,
    name: String,
    /// The family's Japanese name, when the font has one ("MS ゴシック" for MS Gothic).
    local: Option<String>,
    /// (style, file) of each face.
    faces: Vec<(String, PathBuf)>,
}

/// The installed fonts by ASCII-lowercased family name (lookups ignore ASCII case, as
/// [`FontDb::face`] does). Always empty on wasm, which has no system fonts.
type Catalog = HashMap<String, CatalogFamily>;

/// The installed faces' (family, style) by ASCII-lowercased PostScript name.
type PostScriptNames = HashMap<String, (String, String)>;

/// Process-wide font database.
pub struct FontDb {
    faces: RwLock<Vec<Arc<FontFace>>>,
    outlines: Mutex<HashMap<(u32, u32), Arc<BezPath>>>,
    catalog: RwLock<Catalog>,
    /// The installed faces by PostScript name (documents name fonts so: PDF, EPS, .ai).
    postscript: RwLock<PostScriptNames>,
    /// The folders the system font scan reads (the platform's font folders for [`FontDb::global`]).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    font_dirs: Vec<PathBuf>,
    /// Set once the font folders have been scanned. Lookups by family name wait for the first
    /// scan, so what they find never depends on what ran before them.
    #[cfg(not(target_arch = "wasm32"))]
    cataloged: std::sync::OnceLock<()>,
    /// [`FontDb::family_list`], built on demand and dropped when fonts are added or rescanned.
    family_cache: Mutex<Option<Arc<[String]>>>,
    generation: AtomicU64,
    /// Han characters (shared by Chinese, Japanese and Korean) and kana fall back to a Japanese
    /// font first, so kanji take Japanese glyph shapes ([`FontDb::set_japanese_first`]).
    japanese_first: AtomicBool,
    /// System fallback state: characters no system font covers.
    #[cfg(not(target_arch = "wasm32"))]
    sys: Mutex<SysFallback>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
struct SysFallback {
    enabled: bool,
    misses: std::collections::HashSet<char>,
}

/// Families tried (when installed) for characters the loaded fonts lack: CJK, symbols, emoji.
#[cfg(not(target_arch = "wasm32"))]
const SYSTEM_FALLBACKS: &[&str] = &[
    "Helvetica Neue",
    "Arial",
    "Segoe UI",
    "Noto Sans",
    "DejaVu Sans",
    "PingFang SC",
    "Hiragino Sans",
    "Hiragino Kaku Gothic ProN",
    "Apple SD Gothic Neo",
    "Heiti SC",
    "STHeiti",
    "Microsoft YaHei",
    "Yu Gothic",
    "Malgun Gothic",
    "Noto Sans CJK SC",
    "Noto Sans CJK JP",
    "Arial Unicode MS",
    "Apple Symbols",
    "Segoe UI Symbol",
    "Noto Sans Symbols",
    "Noto Sans Symbols2",
    "Noto Emoji",
    "Segoe UI Emoji",
    "Apple Color Emoji",
    "Noto Color Emoji",
];

/// Japanese families tried first for Han characters and kana when [`FontDb::set_japanese_first`]
/// is on: the systems' UI gothics, then other Japanese gothics, then the bundled Shippori Mincho
/// (the only one on the web).
pub(crate) const JAPANESE_FALLBACKS: &[&str] = &[
    "Yu Gothic UI",
    "Yu Gothic",
    "Meiryo UI",
    "Meiryo",
    "Hiragino Sans",
    "Hiragino Kaku Gothic ProN",
    "Noto Sans CJK JP",
    "Noto Sans JP",
    "Source Han Sans JP",
    "BIZ UDPGothic",
    "MS PGothic",
    "Shippori Mincho",
];

/// Han ideographs, kana and CJK punctuation: the characters Japanese and Chinese fonts both cover
/// but draw differently.
fn is_han_or_kana(c: char) -> bool {
    matches!(c as u32, 0x3000..=0x30FF | 0x3190..=0x31FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF | 0x20000..=0x3FFFF)
}

static NEXT_ID: AtomicU32 = AtomicU32::new(1);
const OUTLINE_CACHE_MAX: usize = 50_000;

fn name(font: &skrifa::FontRef<'_>, ids: &[StringId]) -> Option<String> {
    ids.iter().find_map(|id| font.localized_strings(*id).english_or_first().map(|s| s.to_string()).filter(|s| !s.is_empty()))
}

/// A face's (family, style) names.
fn face_names(f: &skrifa::FontRef<'_>) -> Option<(String, String)> {
    let family = name(f, &[StringId::TYPOGRAPHIC_FAMILY_NAME, StringId::FAMILY_NAME])?;
    let style = name(f, &[StringId::TYPOGRAPHIC_SUBFAMILY_NAME, StringId::SUBFAMILY_NAME]).unwrap_or_else(|| "Regular".into());
    Some((family, style))
}

/// The names the system font scan keeps of a face.
#[cfg(not(target_arch = "wasm32"))]
struct ScannedFace {
    family: String,
    style: String,
    postscript: Option<String>,
    /// The family's Japanese name.
    local: Option<String>,
    script: FontScript,
}

/// A face's family name in Japanese, if its name table has one.
fn japanese_family(f: &skrifa::FontRef<'_>) -> Option<String> {
    [StringId::TYPOGRAPHIC_FAMILY_NAME, StringId::FAMILY_NAME].iter().find_map(|id| {
        f.localized_strings(*id).find(|s| s.language().is_some_and(|l| l.starts_with("ja"))).map(|s| s.to_string()).filter(|s| !s.is_empty())
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn scanned_face(f: &skrifa::FontRef<'_>) -> Option<ScannedFace> {
    let (family, style) = face_names(f)?;
    let local = japanese_family(f).filter(|l| *l != family);
    // A Japanese name is a Japanese font whatever its code pages say.
    let script = if local.is_some() { FontScript::Japanese } else { FontScript::Latin };
    Some(ScannedFace { family, style, postscript: name(f, &[StringId::POSTSCRIPT_NAME]), local, script })
}

/// Parse every face in `data` (a font file or collection). Returns `(index, family, style)`.
fn enumerate_faces(data: &[u8]) -> Vec<(u32, String, String)> {
    let count = match FileRef::new(data) {
        Ok(FileRef::Font(_)) => 1,
        Ok(FileRef::Collection(c)) => c.len(),
        Err(_) => 0,
    };
    (0..count)
        .filter_map(|i| {
            let (family, style) = face_names(&skrifa::FontRef::from_index(data, i).ok()?)?;
            Some((i, family, style))
        })
        .collect()
}

/// (family, style, PostScript name) of every face in the font file at `path`, reading only its
/// table directories and `name` tables: a scan opens hundreds of font files, many of them
/// megabytes long.
#[cfg(not(target_arch = "wasm32"))]
fn file_face_names(path: &Path) -> Vec<ScannedFace> {
    use std::io::{Read, Seek, SeekFrom};
    /// Caps on what a (possibly damaged) file can make the scan read.
    const MAX_FACES: u32 = 256;
    const MAX_NAME_TABLE: u32 = 1 << 20;
    let Ok(mut file) = std::fs::File::open(path) else { return vec![] };
    let mut read_at = |offset: u64, len: usize| -> Option<Vec<u8>> {
        let mut buf = vec![0; len];
        file.seek(SeekFrom::Start(offset)).ok()?;
        file.read_exact(&mut buf).ok()?;
        Some(buf)
    };
    let be32 = |b: &[u8], at: usize| b.get(at..at + 4).and_then(|s| s.try_into().ok()).map(u32::from_be_bytes);
    let Some(head) = read_at(0, 12) else { return vec![] };
    // A collection lists where each face's table directory starts.
    let starts: Vec<u32> = if head.starts_with(b"ttcf") {
        let n = be32(&head, 8).unwrap_or(0).min(MAX_FACES) as usize;
        read_at(12, n * 4).map(|b| b.as_chunks::<4>().0.iter().map(|c| u32::from_be_bytes(*c)).collect()).unwrap_or_default()
    } else {
        vec![0]
    };
    starts
        .into_iter()
        .filter_map(|start| {
            let dir = read_at(start.into(), 12)?;
            let tables = u16::from_be_bytes(dir.get(4..6)?.try_into().ok()?) as usize;
            let records = read_at(u64::from(start) + 12, tables * 16)?;
            let rec: &[u8] = records.as_chunks::<16>().0.iter().find(|r| r.starts_with(b"name"))?;
            let (offset, len) = (be32(rec, 8)?, be32(rec, 12)?);
            if len > MAX_NAME_TABLE {
                return None;
            }
            let table = read_at(offset.into(), len as usize)?;
            // A one-table font holding just the `name` table, to read it as the font itself would.
            let mut font = Vec::with_capacity(28 + table.len());
            font.extend_from_slice(&0x0001_0000_u32.to_be_bytes());
            font.extend_from_slice(&[0, 1, 0, 16, 0, 0, 0, 0]);
            font.extend_from_slice(b"name");
            for v in [0, 28, len] {
                font.extend_from_slice(&u32::to_be_bytes(v));
            }
            font.extend_from_slice(&table);
            let mut face = scanned_face(&skrifa::FontRef::new(&font).ok()?)?;
            // The OS/2 table's code pages (version 1 and later), read as 4 bytes at offset 78.
            if face.script == FontScript::Latin
                && let Some(os2) = records.as_chunks::<16>().0.iter().find(|r| r.starts_with(b"OS/2"))
                && be32(os2, 12).is_some_and(|len| len >= 82)
                && let Some(cp) = be32(os2, 8).and_then(|offset| read_at(u64::from(offset) + 78, 4))
                && let Some(cp1) = be32(&cp, 0)
            {
                face.script = FontScript::from_code_pages(cp1);
            }
            Some(face)
        })
        .collect()
}

/// The platform's font folders (the system's and the user's), scanned by [`FontDb::global`].
pub fn system_font_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if cfg!(target_arch = "wasm32") {
        return dirs;
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        dirs.extend(["/System/Library/Fonts", "/Library/Fonts"].map(Into::into));
        if let Some(h) = &home {
            dirs.push(h.join("Library/Fonts"));
        }
    } else if cfg!(windows) {
        let root = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| "C:\\Windows".into());
        dirs.push(root.join("Fonts"));
        if let Some(l) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(l).join("Microsoft\\Windows\\Fonts"));
        }
    } else {
        dirs.extend(["/usr/share/fonts", "/usr/local/share/fonts"].map(Into::into));
        if let Some(h) = &home {
            dirs.push(h.join(".fonts"));
            dirs.push(h.join(".local/share/fonts"));
        }
    }
    dirs
}

fn make_face(bytes: FontBytes, index: u32, family: String, style: String, path: Option<std::path::PathBuf>) -> Option<FontFace> {
    let data: &[u8] = match &bytes {
        FontBytes::Static(b) => b,
        FontBytes::Owned(v) => v.as_slice(),
    };
    let f = skrifa::FontRef::from_index(data, index).ok()?;
    let m = f.metrics(Size::unscaled(), LocationRef::default());
    let a = f.attributes();
    let shaper = harfrust::ShaperData::new(&harfrust::FontRef::from_index(data, index).ok()?);
    Some(FontFace {
        id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        family,
        style,
        weight: a.weight.value(),
        italic: !matches!(a.style, skrifa::attribute::Style::Normal),
        upem: m.units_per_em.max(1) as f64,
        ascent: m.ascent as f64,
        descent: -(m.descent as f64),
        cap_height: m.cap_height.map(|v| v as f64).filter(|v| *v > 0.0).unwrap_or(m.ascent as f64 * 0.72),
        x_height: m.x_height.map(|v| v as f64).filter(|v| *v > 0.0).unwrap_or(m.ascent as f64 * 0.5),
        shaper,
        bytes,
        index,
        path,
    })
}

/// `s` lowercased, without anything but letters and digits.
fn norm_chars(s: &str) -> impl Iterator<Item = char> + '_ {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase)
}

fn norm(s: &str) -> String {
    norm_chars(s).collect()
}

/// Weight implied by a style name (400 = regular).
pub fn style_weight(style: &str) -> f32 {
    let s = norm(style);
    const TABLE: &[(&str, f32)] = &[
        ("extralight", 200.0),
        ("ultralight", 200.0),
        ("semibold", 600.0),
        ("demibold", 600.0),
        ("extrabold", 800.0),
        ("ultrabold", 800.0),
        ("hairline", 100.0),
        ("thin", 100.0),
        ("light", 300.0),
        ("medium", 500.0),
        ("bold", 700.0),
        ("black", 900.0),
        ("heavy", 900.0),
    ];
    TABLE.iter().find(|(k, _)| s.contains(k)).map(|(_, w)| *w).unwrap_or(400.0)
}

fn style_italic(style: &str) -> bool {
    let s = norm(style);
    s.contains("italic") || s.contains("oblique") || s == "it"
}

impl FontDb {
    /// A database holding the bundled fonts, whose system font scan reads `font_dirs`.
    pub fn with_font_dirs(font_dirs: Vec<PathBuf>) -> Self {
        let mut faces = Vec::new();
        for data in BUNDLED {
            for (i, family, style) in enumerate_faces(data) {
                if let Some(f) = make_face(FontBytes::Static(data), i, family, style, None) {
                    faces.push(Arc::new(f));
                }
            }
        }
        Self {
            faces: RwLock::new(faces),
            outlines: Mutex::new(HashMap::new()),
            catalog: RwLock::new(Catalog::new()),
            postscript: RwLock::new(PostScriptNames::new()),
            font_dirs,
            #[cfg(not(target_arch = "wasm32"))]
            cataloged: std::sync::OnceLock::new(),
            family_cache: Mutex::new(None),
            generation: AtomicU64::new(0),
            japanese_first: AtomicBool::new(false),
            #[cfg(not(target_arch = "wasm32"))]
            sys: Mutex::new(SysFallback { enabled: true, ..Default::default() }),
        }
    }

    /// Prefer Japanese fonts for Han characters and kana the requested font lacks (the interface
    /// is in Japanese). Off by default: the system fallback order then decides.
    pub fn set_japanese_first(&self, on: bool) {
        if self.japanese_first.swap(on, Ordering::Relaxed) != on {
            self.changed();
        }
    }

    /// Enable or disable the lazy system-font fallback for characters the loaded fonts lack
    /// (native only; on by default).
    pub fn set_system_fallback(&self, on: bool) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.sys.lock().unwrap_or_else(|e| e.into_inner()).enabled = on;
        }
        #[cfg(target_arch = "wasm32")]
        let _ = on;
    }

    /// Process-wide database: the bundled fonts, then the installed system fonts, cataloged the
    /// first time a lookup by family name needs them (or ahead of time by
    /// [`FontDb::scan_in_background`]).
    pub fn global() -> &'static FontDb {
        static DB: std::sync::OnceLock<FontDb> = std::sync::OnceLock::new();
        DB.get_or_init(|| FontDb::with_font_dirs(system_font_dirs()))
    }

    fn read_faces(&self) -> std::sync::RwLockReadGuard<'_, Vec<Arc<FontFace>>> {
        self.faces.read().unwrap_or_else(|e| e.into_inner())
    }

    fn read_catalog(&self) -> std::sync::RwLockReadGuard<'_, Catalog> {
        self.ensure_catalog();
        self.catalog.read().unwrap_or_else(|e| e.into_inner())
    }

    /// Fonts were added or the catalog changed: cached family lists are stale.
    fn changed(&self) {
        *self.family_cache.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// A number that changes whenever the available fonts do (fonts added, system fonts
    /// rescanned): lists built from [`FontDb::family_list`] are current while it doesn't.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    /// Family names available (loaded plus installed system fonts), sorted and deduplicated.
    pub fn families(&self) -> Vec<String> {
        self.family_list().to_vec()
    }

    /// [`FontDb::families`], shared: cheap to call every frame.
    pub fn family_list(&self) -> Arc<[String]> {
        let catalog = self.read_catalog();
        let mut cache = self.family_cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(list) = cache.as_ref() {
            return list.clone();
        }
        let mut v: Vec<String> = self.read_faces().iter().map(|f| f.family.clone()).collect();
        v.extend(catalog.values().map(|c| c.name.clone()));
        v.sort_by_key(|a| a.to_lowercase());
        v.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        let list: Arc<[String]> = v.into();
        *cache = Some(list.clone());
        list
    }

    /// Style names available for `family`: upright styles by weight, then italics.
    pub fn styles(&self, family: &str) -> Vec<String> {
        let mut v: Vec<(bool, f32, String)> =
            self.read_faces().iter().filter(|f| f.family.eq_ignore_ascii_case(family)).map(|f| (f.italic, f.weight, f.style.clone())).collect();
        if let Some(c) = self.read_catalog().get(&family.to_ascii_lowercase()) {
            for (style, _) in &c.faces {
                if !v.iter().any(|(_, _, s)| s.eq_ignore_ascii_case(style)) {
                    v.push((style_italic(style), style_weight(style), style.clone()));
                }
            }
        }
        v.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)));
        v.dedup_by(|a, b| a.2 == b.2);
        v.into_iter().map(|t| t.2).collect()
    }

    /// Add a user font (TTF/OTF/TTC bytes). Returns the number of faces added (0 if unparseable or
    /// every face was already present).
    pub fn add_font(&self, bytes: Vec<u8>) -> usize {
        self.add_font_from(bytes, None)
    }

    /// [`FontDb::add_font`] for the bytes of the file at `path` (when known).
    fn add_font_from(&self, bytes: Vec<u8>, path: Option<&Path>) -> usize {
        let data = Arc::new(bytes);
        let mut added = 0;
        for (i, family, style) in enumerate_faces(&data) {
            if self.read_faces().iter().any(|f| f.family.eq_ignore_ascii_case(&family) && f.style.eq_ignore_ascii_case(&style)) {
                continue;
            }
            if let Some(f) = make_face(FontBytes::Owned(data.clone()), i, family, style, path.map(Path::to_path_buf)) {
                self.faces.write().unwrap_or_else(|e| e.into_inner()).push(Arc::new(f));
                added += 1;
            }
        }
        if added > 0 {
            self.changed();
        }
        added
    }

    /// Catalog the installed fonts unless that has been done: the first caller scans the font
    /// folders, any other waits for that scan to finish. A no-op on wasm.
    fn ensure_catalog(&self) {
        #[cfg(not(target_arch = "wasm32"))]
        self.cataloged.get_or_init(|| {
            self.scan_font_dirs();
        });
    }

    /// Catalog the installed fonts on a background thread, so the first lookup by family name
    /// (opening a file, the font menus) doesn't wait for the scan. A no-op once they are
    /// cataloged, and on wasm.
    pub fn scan_in_background(&'static self) {
        #[cfg(not(target_arch = "wasm32"))]
        if self.cataloged.get().is_none() {
            // A failed spawn leaves the scan to the first lookup that needs it.
            let _ = std::thread::Builder::new().name("font-scan".into()).spawn(move || self.ensure_catalog());
        }
    }

    /// Scan the font folders again (fonts installed or removed since), cataloging the faces
    /// found (native only). Font data is loaded when a cataloged family is first resolved.
    /// Returns the number of faces cataloged.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load_system_fonts(&self) -> usize {
        // The first scan, or a rescan once it is done (never both at once).
        let mut first = None;
        self.cataloged.get_or_init(|| first = Some(self.scan_font_dirs()));
        first.unwrap_or_else(|| self.scan_font_dirs())
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn scan_font_dirs(&self) -> usize {
        let mut catalog = Catalog::new();
        let mut postscript = PostScriptNames::new();
        let mut n = 0;
        let mut stack = self.font_dirs.clone();
        // Each folder once, however links lead back to it.
        let mut visited = std::collections::HashSet::new();
        while let Some(d) = stack.pop() {
            if !visited.insert(std::fs::canonicalize(&d).unwrap_or_else(|_| d.clone())) {
                continue;
            }
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let ext = p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase());
                if !matches!(ext.as_deref(), Some("ttf" | "otf" | "ttc" | "otc")) {
                    continue;
                }
                for ScannedFace { family, style, postscript: ps, local, script } in file_face_names(&p) {
                    if let Some(ps) = ps {
                        postscript.insert(ps.to_ascii_lowercase(), (family.clone(), style.clone()));
                    }
                    let entry = catalog.entry(family.to_ascii_lowercase()).or_default();
                    if entry.name.is_empty() {
                        entry.name = family;
                    }
                    if entry.local.is_none() {
                        entry.local = local;
                    }
                    entry.script = entry.script.max(script);
                    entry.faces.push((style, p.clone()));
                    n += 1;
                }
            }
        }
        log::debug!("cataloged {n} system font faces");
        *self.catalog.write().unwrap_or_else(|e| e.into_inner()) = catalog;
        *self.postscript.write().unwrap_or_else(|e| e.into_inner()) = postscript;
        self.changed();
        n
    }

    /// The Japanese name of installed `family`, when its fonts have one ("MS ゴシック" for MS
    /// Gothic). Menus show it unless Preferences ▸ Type ▸ Show Font Names in English is on.
    pub fn local_name(&self, family: &str) -> Option<String> {
        self.read_catalog().get(&family.to_ascii_lowercase()).and_then(|c| c.local.clone())
    }

    /// What `family` is for (font lists group by it): from the installed fonts' code pages, or
    /// for loaded fonts (bundled, user) from what they cover.
    pub fn script(&self, family: &str) -> FontScript {
        if let Some(c) = self.read_catalog().get(&family.to_ascii_lowercase()) {
            return c.script;
        }
        let faces = self.read_faces();
        let Some(f) = faces.iter().find(|f| f.family.eq_ignore_ascii_case(family)) else { return FontScript::Latin };
        if f.covers('あ') {
            FontScript::Japanese
        } else if f.covers('永') || f.covers('한') {
            FontScript::OtherCjk
        } else if f.covers('A') {
            FontScript::Latin
        } else {
            FontScript::Symbol
        }
    }

    /// The family whose Japanese name is `name` ("MS ゴシック" → "MS Gothic"): documents and
    /// people name Japanese fonts either way.
    pub fn family_of_local_name(&self, name: &str) -> Option<String> {
        self.read_catalog().values().find(|c| c.local.as_deref() == Some(name)).map(|c| c.name.clone())
    }

    /// The (family, style) of the installed face whose PostScript name is `name` (ignoring ASCII
    /// case). Documents name fonts this way, and a family can hold hyphens
    /// ("Rounded-X-Mplus-1c-black" is Rounded-X M+ 1c, black), so the name can't be split.
    pub fn by_postscript_name(&self, name: &str) -> Option<(String, String)> {
        self.ensure_catalog();
        self.postscript.read().unwrap_or_else(|e| e.into_inner()).get(&name.to_ascii_lowercase()).cloned()
    }

    /// Load the files of the installed `family`. Returns whether any face was added.
    #[cfg(not(target_arch = "wasm32"))]
    fn load_cataloged(&self, family: &str) -> bool {
        let mut paths: Vec<PathBuf> =
            self.read_catalog().get(&family.to_ascii_lowercase()).map(|c| c.faces.iter().map(|(_, p)| p.clone()).collect()).unwrap_or_default();
        paths.sort();
        paths.dedup();
        let mut any = false;
        for p in paths {
            if let Ok(data) = std::fs::read(&p) {
                any |= self.add_font_from(data, Some(&p)) > 0;
            }
        }
        any
    }

    /// Resolve a family + style to a face, falling back to the closest style of the family, then to
    /// Source Sans 3 Regular. Installed system fonts are found by name whatever ran before. `None`
    /// only if no font at all is loaded (the bundled fonts failed to parse), in which case text has
    /// no glyphs.
    pub fn face(&self, family: &str, style: &str) -> Option<Arc<FontFace>> {
        // A Japanese family name ("MS ゴシック") is the family it names.
        #[cfg(not(target_arch = "wasm32"))]
        if !family.is_ascii()
            && !self.is_loaded(family)
            && let Some(f) = self.family_of_local_name(family)
        {
            return self.face(&f, style);
        }
        let found = self.find(family, style);
        if found.as_ref().is_some_and(|f| norm(&f.style) == norm(style)) {
            return found;
        }
        // The family, or this style of it, is installed but not loaded yet.
        #[cfg(not(target_arch = "wasm32"))]
        if (found.is_none() || self.is_cataloged(family, style))
            && self.load_cataloged(family)
            && let Some(f) = self.find(family, style)
        {
            return Some(f);
        }
        found
            .or_else(|| self.find(FALLBACK_FAMILY, style))
            .or_else(|| self.find(FALLBACK_FAMILY, "Regular"))
            .or_else(|| self.read_faces().first().cloned())
    }

    /// Is `style` of `family` among the installed fonts?
    #[cfg(not(target_arch = "wasm32"))]
    fn is_cataloged(&self, family: &str, style: &str) -> bool {
        let ns = norm(style);
        self.read_catalog().get(&family.to_ascii_lowercase()).is_some_and(|c| c.faces.iter().any(|(s, _)| norm(s) == ns))
    }

    /// The loaded face with [`FontFace::id`] `id` (the face a laid-out glyph came from).
    pub fn face_by_id(&self, id: u32) -> Option<Arc<FontFace>> {
        self.read_faces().iter().find(|f| f.id == id).cloned()
    }

    /// The available family `name` names, ignoring case and anything but letters and digits, as
    /// PostScript names write families ("MicrosoftYaHei" is Microsoft YaHei).
    pub fn find_family(&self, name: &str) -> Option<String> {
        self.family_list().iter().find(|f| norm_chars(f).eq(norm_chars(name))).cloned()
    }

    /// Is `family` available (loaded, or installed on the system)?
    pub fn has_family(&self, family: &str) -> bool {
        self.is_loaded(family) || self.read_catalog().contains_key(&family.to_ascii_lowercase())
    }

    pub(crate) fn is_loaded(&self, family: &str) -> bool {
        self.read_faces().iter().any(|f| f.family.eq_ignore_ascii_case(family))
    }

    pub(crate) fn find(&self, family: &str, style: &str) -> Option<Arc<FontFace>> {
        let faces = self.read_faces();
        let cands: Vec<&Arc<FontFace>> = faces.iter().filter(|f| f.family.eq_ignore_ascii_case(family)).collect();
        if cands.is_empty() {
            return None;
        }
        let ns = norm(style);
        if let Some(f) = cands.iter().find(|f| norm(&f.style) == ns) {
            return Some((*f).clone());
        }
        let (tw, ti) = (style_weight(style), style_italic(style));
        cands
            .iter()
            .min_by(|a, b| {
                let sa = (a.weight - tw).abs() + if a.italic != ti { 1000.0 } else { 0.0 };
                let sb = (b.weight - tw).abs() + if b.italic != ti { 1000.0 } else { 0.0 };
                sa.total_cmp(&sb)
            })
            .map(|f| (*f).clone())
    }

    /// First face (fallback family first, then load order) that covers `c`; on native, system
    /// fonts are loaded lazily the first time no loaded face covers a character.
    pub(crate) fn fallback_for(&self, c: char, exclude: u32) -> Option<Arc<FontFace>> {
        if self.japanese_first.load(Ordering::Relaxed)
            && is_han_or_kana(c)
            && let Some(f) = self.japanese_fallback(c, exclude)
        {
            return Some(f);
        }
        if let Some(f) = self.loaded_fallback(c, exclude) {
            return Some(f);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if self.system_fallback(c) {
            return self.loaded_fallback(c, exclude);
        }
        None
    }

    /// A face that covers `c`, for text drawn outside the canvas (the app's own UI): the loaded
    /// fonts first, then (native) an installed one, loaded on demand. Characters no font covers
    /// are remembered, so they are looked for once.
    pub fn face_covering(&self, c: char) -> Option<Arc<FontFace>> {
        self.fallback_for(c, 0)
    }

    /// The first of [`JAPANESE_FALLBACKS`] that covers `c`: loaded, or (native) installed and
    /// loaded now.
    fn japanese_fallback(&self, c: char, exclude: u32) -> Option<Arc<FontFace>> {
        for fam in JAPANESE_FALLBACKS {
            let loaded = |db: &FontDb| {
                db.read_faces()
                    .iter()
                    .filter(|f| f.id != exclude && f.family.eq_ignore_ascii_case(fam) && f.covers(c))
                    .min_by_key(|f| (f.italic, (f.weight - 400.0).abs() as i32))
                    .cloned()
            };
            if let Some(f) = loaded(self) {
                return Some(f);
            }
            #[cfg(not(target_arch = "wasm32"))]
            if !self.is_loaded(fam)
                && self.load_cataloged(fam)
                && let Some(f) = loaded(self)
            {
                return Some(f);
            }
        }
        None
    }

    fn loaded_fallback(&self, c: char, exclude: u32) -> Option<Arc<FontFace>> {
        let faces = self.read_faces();
        let mut order: Vec<&Arc<FontFace>> = faces.iter().filter(|f| f.id != exclude).collect();
        order.sort_by_key(|f| (!f.family.eq_ignore_ascii_case(FALLBACK_FAMILY), f.italic, (f.weight - 400.0).abs() as i32));
        order.into_iter().find(|f| f.covers(c)).cloned()
    }

    /// Load a system font covering `c` (preferred fallback families first, then any cataloged
    /// file under 40 MB). Returns true if one was loaded. Misses are remembered.
    #[cfg(not(target_arch = "wasm32"))]
    fn system_fallback(&self, c: char) -> bool {
        if c.is_control() || c.is_whitespace() {
            return false;
        }
        {
            let sys = self.sys.lock().unwrap_or_else(|e| e.into_inner());
            if !sys.enabled || sys.misses.contains(&c) {
                return false;
            }
        }
        let covered = |db: &FontDb| db.read_faces().iter().any(|f| f.covers(c));
        for fam in SYSTEM_FALLBACKS {
            if !self.is_loaded(fam) && self.load_cataloged(fam) && covered(self) {
                return true;
            }
        }
        let mut paths: Vec<PathBuf> = self.read_catalog().values().flat_map(|c| c.faces.iter().map(|(_, p)| p.clone())).collect();
        paths.sort();
        paths.dedup();
        for p in paths {
            if std::fs::metadata(&p).map(|m| m.len() > 40 << 20).unwrap_or(true) {
                continue;
            }
            let Ok(data) = std::fs::read(&p) else { continue };
            let hit =
                enumerate_faces(&data).iter().any(|(i, _, _)| skrifa::FontRef::from_index(&data, *i).is_ok_and(|f| f.charmap().map(c).is_some()));
            if hit && self.add_font_from(data, Some(&p)) > 0 && covered(self) {
                return true;
            }
        }
        self.sys.lock().unwrap_or_else(|e| e.into_inner()).misses.insert(c);
        false
    }

    /// Glyph outline in font units, y-down (flipped), cached per (face, glyph).
    pub fn outline(&self, face: &FontFace, gid: u32) -> Arc<BezPath> {
        let key = (face.id, gid);
        if let Some(p) = self.outlines.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            return p.clone();
        }
        let mut pen = FlipPen(BezPath::new());
        if let Some(f) = face.skrifa()
            && let Some(g) = f.outline_glyphs().get(GlyphId::new(gid))
        {
            let _ = g.draw(DrawSettings::unhinted(Size::unscaled(), LocationRef::default()), &mut pen);
        }
        let p = Arc::new(pen.0);
        let mut cache = self.outlines.lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() >= OUTLINE_CACHE_MAX {
            cache.clear();
        }
        cache.insert(key, p.clone());
        p
    }
}

struct FlipPen(BezPath);

impl OutlinePen for FlipPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to((x as f64, -y as f64));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to((x as f64, -y as f64));
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.0.quad_to((cx0 as f64, -cy0 as f64), (x as f64, -y as f64));
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.0.curve_to((cx0 as f64, -cy0 as f64), (cx1 as f64, -cy1 as f64), (x as f64, -y as f64));
    }
    fn close(&mut self) {
        self.0.close_path();
    }
}
