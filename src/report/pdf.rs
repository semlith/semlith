//! The PDF, typeset natively to match the HTML's print rendering.
//!
//! A saved report that only exists if a browser is open is not a saved report
//! — a daemon on a build box, an agent over MCP and `semlith report --out` all
//! have the blocks and no browser — so the page is laid out here, from the
//! same typed structure the other four renderers read.
//!
//! - **Fonts.** IBM Plex (OFL, `fonts/`), cut to Latin, Latin Extended-A and
//!   the punctuation reports print, embedded per document as a subset CID
//!   font with its real glyph widths. A character Plex has no glyph for — a
//!   CJK path, a Hangul query — is looked up in the operating system's own CJK
//!   fonts at render time (Hiragino, Apple SD Gothic Neo, Arial Unicode on
//!   macOS; Noto CJK, Droid, WenQuanYi on Linux; Yu Gothic, MS Gothic, YaHei,
//!   Malgun on Windows) and subset from there. Nothing is kept after the
//!   render. A character no font has draws as the missing-glyph box, and its
//!   text layer is still the character, because every glyph carries a
//!   ToUnicode entry: `pdftotext` returns what the report says, never `?`.
//! - **Layout.** The print stylesheet's geometry in points: A4 with 14 mm sides
//!   (Letter when the locale is US or Canada), the top bar, title block, meta
//!   strip, KPI boxes and table cards. Numbers are right-aligned mono; an id
//!   that does not fit its column ellipsizes rather than wrapping mid-token;
//!   a table that runs over a page repeats its header.
//! - **Structure.** Every page carries a running footer with page x of y and
//!   the ledger head; every card is a bookmark; the document has Title,
//!   Author, Producer and dates, and it is tagged (Document, H1, P, Sect, H2,
//!   Table, TR, TH, TD, Code) with decorations marked as artifacts.

use super::{Block, Cell, Kind, Report, Table};
use pdf_writer::types::{
    CidFontType, FontFlags, OutlineItemFlags, StructRole, SystemInfo, UnicodeCmap,
};
use pdf_writer::{Content, Date, Filter, Finish, Name, Pdf, Rect, Ref, Str, TextStr};
use std::collections::BTreeMap;

// ------------------------------------------------------------------- fonts

/// The five Plex faces, as the HTML loads them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Face {
    Sans,
    SansMedium,
    SansBold,
    Mono,
    MonoMedium,
}

const PLEX: [(Face, &[u8], &str); 5] = [
    (
        Face::Sans,
        include_bytes!("fonts/IBMPlexSans-Regular.ttf"),
        "IBMPlexSans",
    ),
    (
        Face::SansMedium,
        include_bytes!("fonts/IBMPlexSans-Medium.ttf"),
        "IBMPlexSans-Medium",
    ),
    (
        Face::SansBold,
        include_bytes!("fonts/IBMPlexSans-SemiBold.ttf"),
        "IBMPlexSans-SemiBold",
    ),
    (
        Face::Mono,
        include_bytes!("fonts/IBMPlexMono-Regular.ttf"),
        "IBMPlexMono",
    ),
    (
        Face::MonoMedium,
        include_bytes!("fonts/IBMPlexMono-Medium.ttf"),
        "IBMPlexMono-Medium",
    ),
];

/// The OS fonts a glyph Plex lacks is looked for in, in order. Directories
/// are listed for a family whose file name varies by OS release.
fn fallback_candidates() -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    #[cfg(target_os = "macos")]
    {
        let dir = std::path::Path::new("/System/Library/Fonts");
        if let Ok(entries) = std::fs::read_dir(dir) {
            let mut named: Vec<_> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    let name = p.file_name().map(|n| n.to_string_lossy().to_string());
                    name.is_some_and(|n| {
                        n.contains("Hiragino") || n.contains("ヒラギノ") || n.contains("PingFang")
                    })
                })
                .collect();
            named.sort();
            out.extend(named);
        }
        out.push("/System/Library/Fonts/AppleSDGothicNeo.ttc".into());
        out.push("/System/Library/Fonts/Supplemental/Arial Unicode.ttf".into());
        out.push("/System/Library/Fonts/STHeiti Light.ttc".into());
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        for path in [
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf",
            "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
            "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
            "/usr/share/fonts/wenquanyi/wqy-microhei/wqy-microhei.ttc",
            "/usr/share/fonts/truetype/arphic/uming.ttc",
        ] {
            out.push(path.into());
        }
    }
    #[cfg(windows)]
    {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let fonts = std::path::Path::new(&root).join("Fonts");
        for name in [
            "YuGothM.ttc",
            "msgothic.ttc",
            "msyh.ttc",
            "malgun.ttf",
            "simsun.ttc",
        ] {
            out.push(fonts.join(name));
        }
    }
    out
}

/// One font file and the face of it this document uses.
struct Source<'a> {
    data: &'a [u8],
    index: u32,
    face: ttf_parser::Face<'a>,
    /// The PostScript name, without the subset tag.
    name: String,
    cff: bool,
    mono: bool,
}

impl<'a> Source<'a> {
    fn parse(data: &'a [u8], index: u32, name: &str, mono: bool) -> Option<Self> {
        let face = ttf_parser::Face::parse(data, index).ok()?;
        let cff = face.tables().cff.is_some();
        // A CFF2 (variable) font is not something the subsetter takes.
        if face.tables().cff2.is_some() {
            return None;
        }
        Some(Self {
            data,
            index,
            face,
            name: name.to_string(),
            cff,
            mono,
        })
    }

    fn advance(&self, gid: u16) -> f32 {
        let upem = f32::from(self.face.units_per_em());
        f32::from(
            self.face
                .glyph_hor_advance(ttf_parser::GlyphId(gid))
                .unwrap_or(0),
        ) / upem
    }
}

/// Which font a run is drawn in: a Plex face, or the n-th OS fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum FontKey {
    Plex(Face),
    Fallback(usize),
}

/// The glyphs one document drew from one font, and the CIDs they were given.
#[derive(Default)]
struct Used {
    remapper: subsetter::GlyphRemapper,
    /// `(original gid, char)` → CID.
    cids: BTreeMap<(u16, char), u16>,
    /// CID → (new gid, char, width in 1/1000 em).
    table: BTreeMap<u16, (u16, char, f32)>,
}

/// Every font the document can draw with.
struct Fonts<'a> {
    plex: Vec<(Face, Source<'a>)>,
    fallbacks: Vec<Source<'a>>,
    used: BTreeMap<FontKey, Used>,
}

impl<'a> Fonts<'a> {
    fn new(fallback_data: &'a [(Vec<u8>, u32, String)]) -> Self {
        let plex = PLEX
            .iter()
            .filter_map(|(face, data, name)| {
                let mono = matches!(face, Face::Mono | Face::MonoMedium);
                Source::parse(data, 0, name, mono).map(|s| (*face, s))
            })
            .collect();
        let fallbacks = fallback_data
            .iter()
            .filter_map(|(data, index, name)| Source::parse(data, *index, name, false))
            .collect();
        Self {
            plex,
            fallbacks,
            used: BTreeMap::new(),
        }
    }

    fn plex(&self, face: Face) -> &Source<'a> {
        &self
            .plex
            .iter()
            .find(|(f, _)| *f == face)
            .expect("every Plex face is compiled in")
            .1
    }

    /// The font and glyph a character is drawn with.
    fn glyph(&self, face: Face, c: char) -> (FontKey, u16) {
        if let Some(g) = self.plex(face).face.glyph_index(c) {
            return (FontKey::Plex(face), g.0);
        }
        if !c.is_whitespace() {
            for (i, source) in self.fallbacks.iter().enumerate() {
                if let Some(g) = source.face.glyph_index(c) {
                    return (FontKey::Fallback(i), g.0);
                }
            }
        }
        // No font has it: the missing-glyph box, still carrying the character
        // in the text layer.
        (FontKey::Plex(face), 0)
    }

    fn source(&self, key: FontKey) -> &Source<'a> {
        match key {
            FontKey::Plex(face) => self.plex(face),
            FontKey::Fallback(i) => &self.fallbacks[i],
        }
    }

    /// The width of `text` at `size`, in points, with `spacing` added per
    /// character (the eyebrow's tracking).
    fn width(&self, face: Face, size: f32, spacing: f32, text: &str) -> f32 {
        text.chars()
            .map(|c| {
                let (key, gid) = self.glyph(face, c);
                self.source(key).advance(gid) * size + spacing
            })
            .sum()
    }

    /// Assign each character a CID in the font that draws it, and return the
    /// runs: one per change of font, each a string of big-endian CIDs.
    fn encode(&mut self, face: Face, text: &str) -> Vec<(FontKey, Vec<u8>)> {
        let mut runs: Vec<(FontKey, Vec<u8>)> = Vec::new();
        for c in text.chars() {
            let (key, gid) = self.glyph(face, c);
            let source = self.source(key);
            let width = source.advance(gid) * 1000.0;
            let cff = source.cff;
            let used = self.used.entry(key).or_default();
            let cid = match used.cids.get(&(gid, c)) {
                Some(cid) => *cid,
                None => {
                    let new = used.remapper.remap(gid);
                    // A CFF subset is CID-keyed with CID = new glyph id, so
                    // one glyph is one CID. A TrueType one takes any CID
                    // through the CIDToGIDMap, so two characters drawn with
                    // one glyph (or both missing) keep their own text.
                    let cid = if cff {
                        new
                    } else {
                        used.table.keys().next_back().map_or(1, |k| k + 1)
                    };
                    used.cids.insert((gid, c), cid);
                    used.table.insert(cid, (new, c, width));
                    cid
                }
            };
            match runs.last_mut() {
                Some((k, bytes)) if *k == key => bytes.extend(cid.to_be_bytes()),
                _ => runs.push((key, cid.to_be_bytes().to_vec())),
            }
        }
        runs
    }
}

fn resource_name(key: FontKey) -> Vec<u8> {
    match key {
        FontKey::Plex(face) => format!("P{}", face as u8).into_bytes(),
        FontKey::Fallback(i) => format!("F{i}").into_bytes(),
    }
}

/// Read the OS fonts that hold a glyph some character of this document needs
/// and Plex lacks. Empty when Plex covers everything, which is every
/// all-Latin report: nothing is read from disk then.
fn load_fallbacks(text: &str) -> Vec<(Vec<u8>, u32, String)> {
    let plex = ttf_parser::Face::parse(PLEX[0].1, 0).ok();
    let mut wanted: Vec<char> = text
        .chars()
        .filter(|c| !c.is_whitespace() && !c.is_control())
        .filter(|c| plex.as_ref().is_none_or(|f| f.glyph_index(*c).is_none()))
        .collect();
    wanted.sort_unstable();
    wanted.dedup();
    let mut out = Vec::new();
    for path in fallback_candidates() {
        if wanted.is_empty() {
            break;
        }
        let Ok(data) = std::fs::read(&path) else {
            continue;
        };
        let count = ttf_parser::fonts_in_collection(&data).unwrap_or(1);
        // The first face of a collection that serves anything; that is the
        // regular weight in every candidate listed.
        let found = (0..count).find_map(|index| {
            let face = ttf_parser::Face::parse(&data, index).ok()?;
            if face.tables().cff2.is_some() {
                return None;
            }
            let serves: Vec<char> = wanted
                .iter()
                .copied()
                .filter(|c| face.glyph_index(*c).is_some())
                .collect();
            if serves.is_empty() {
                return None;
            }
            // The first PostScript name ttf-parser can decode (a Mac-encoded
            // record comes back `None`), else the file's own name, so
            // `pdffonts` says which system font the glyphs came from.
            let name = face
                .names()
                .into_iter()
                .filter(|n| n.name_id == ttf_parser::name_id::POST_SCRIPT_NAME)
                .find_map(|n| n.to_string())
                .unwrap_or_else(|| {
                    path.file_stem()
                        .map_or("Fallback".into(), |s| s.to_string_lossy().into_owned())
                });
            Some((index, serves, name))
        });
        if let Some((index, serves, name)) = found {
            wanted.retain(|c| !serves.contains(c));
            let name: String = name.chars().filter(char::is_ascii_alphanumeric).collect();
            out.push((data, index, name));
        }
    }
    out
}

// ------------------------------------------------------------------ colour

/// A colour token, as the print stylesheet resolves it: the portal's light
/// tokens with the print overrides (`COMPONENTS`'s `@media print`).
fn colour(name: &str) -> [f32; 3] {
    let print = [
        ("soft", "#f6f8fa"),
        ("line", "#cfd8df"),
        ("line-soft", "#e3e9ee"),
        ("muted", "#5d7083"),
        ("code-bg", "#f3f6f8"),
        ("code-ink", "#1e2a35"),
        ("code-dim", "#5d7083"),
    ];
    let hex = print
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, v)| v.to_string())
        .or_else(|| {
            let light = super::render::light_tokens();
            let start = light.find(&format!("--{name}:"))? + name.len() + 3;
            Some(light[start..start + 7].to_string())
        })
        .unwrap_or_else(|| "#1e2a35".into());
    let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0) as f32 / 255.0;
    [channel(1), channel(3), channel(5)]
}

// ------------------------------------------------------------------ paper

const MM: f32 = 72.0 / 25.4;

/// A4, or Letter where the locale is the United States or Canada.
fn paper() -> (f32, f32) {
    let locale = ["LC_ALL", "LC_PAPER", "LANG"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty()))
        .unwrap_or_default();
    if locale.contains("_US") || locale.contains("_CA") {
        (612.0, 792.0)
    } else {
        (595.28, 841.89)
    }
}

const SIDE: f32 = 14.0 * MM;
const TOP: f32 = 16.0 * MM;
const BOTTOM: f32 = 18.0 * MM;
/// The print stylesheet's px, in points.
const PX: f32 = 0.75;
const PAD_X: f32 = 14.0 * PX;
const RADIUS: f32 = 6.0;

// -------------------------------------------------------------- structure

/// One element of the structure tree.
struct Elem {
    role: StructRole,
    parent: Option<usize>,
    children: Vec<usize>,
    /// `(page, mcid)` of the content it owns.
    content: Vec<(usize, i32)>,
}

/// One bookmark: a card's title, its page and its height on the page.
struct Mark {
    title: String,
    page: usize,
    top: f32,
}

// --------------------------------------------------------------- the page

struct Page {
    content: Content,
    /// The structure element for each MCID on this page, in order.
    mcids: Vec<usize>,
}

struct Writer<'a> {
    fonts: Fonts<'a>,
    w: f32,
    h: f32,
    pages: Vec<Page>,
    y: f32,
    elems: Vec<Elem>,
    marks: Vec<Mark>,
}

impl<'a> Writer<'a> {
    fn new(fonts: Fonts<'a>) -> Self {
        let (w, h) = paper();
        let mut writer = Self {
            fonts,
            w,
            h,
            pages: Vec::new(),
            y: TOP,
            elems: vec![Elem {
                role: StructRole::Document,
                parent: None,
                children: Vec::new(),
                content: Vec::new(),
            }],
            marks: Vec::new(),
        };
        writer.new_page();
        writer
    }

    fn left(&self) -> f32 {
        SIDE
    }

    fn inner(&self) -> f32 {
        self.w - 2.0 * SIDE
    }

    fn bottom(&self) -> f32 {
        self.h - BOTTOM
    }

    fn page(&mut self) -> &mut Page {
        self.pages.last_mut().expect("a page")
    }

    fn new_page(&mut self) {
        self.pages.push(Page {
            content: Content::new(),
            mcids: Vec::new(),
        });
        self.y = TOP;
    }

    /// Start a new page unless `height` still fits on this one.
    fn room(&mut self, height: f32) -> bool {
        if self.y + height > self.bottom() && self.y > TOP + 1.0 {
            self.new_page();
            return true;
        }
        false
    }

    fn elem(&mut self, role: StructRole, parent: usize) -> usize {
        self.elems.push(Elem {
            role,
            parent: Some(parent),
            children: Vec::new(),
            content: Vec::new(),
        });
        let id = self.elems.len() - 1;
        self.elems[parent].children.push(id);
        id
    }

    // ------------------------------------------------------------ drawing

    fn fill(&mut self, x: f32, top: f32, w: f32, h: f32, name: &str) {
        let [r, g, b] = colour(name);
        let ph = self.h;
        let c = &mut self.page().content;
        c.begin_marked_content(Name(b"Artifact"));
        c.set_fill_rgb(r, g, b);
        c.rect(x, ph - top - h, w, h);
        c.fill_nonzero();
        c.end_marked_content();
    }

    fn hline(&mut self, x: f32, y: f32, w: f32, name: &str, width: f32) {
        let [r, g, b] = colour(name);
        let ph = self.h;
        let c = &mut self.page().content;
        c.begin_marked_content(Name(b"Artifact"));
        c.set_stroke_rgb(r, g, b);
        c.set_line_width(width);
        c.move_to(x, ph - y);
        c.line_to(x + w, ph - y);
        c.stroke();
        c.end_marked_content();
    }

    fn vline(&mut self, x: f32, top: f32, bottom: f32, name: &str) {
        let [r, g, b] = colour(name);
        let ph = self.h;
        let c = &mut self.page().content;
        c.begin_marked_content(Name(b"Artifact"));
        c.set_stroke_rgb(r, g, b);
        c.set_line_width(0.75);
        c.move_to(x, ph - top);
        c.line_to(x, ph - bottom);
        c.stroke();
        c.end_marked_content();
    }

    /// A rounded rectangle, filled, stroked or both; `round_top` and
    /// `round_bottom` let a card that runs over a page close square.
    #[allow(clippy::too_many_arguments)]
    fn rounded(
        &mut self,
        x: f32,
        top: f32,
        w: f32,
        h: f32,
        radius: f32,
        fill: Option<&str>,
        stroke: Option<(&str, f32)>,
        corners: (bool, bool),
    ) {
        let ph = self.h;
        let (rt, rb) = (
            if corners.0 { radius } else { 0.0 },
            if corners.1 { radius } else { 0.0 },
        );
        let k = 0.552_284_8;
        let (x0, x1, y0, y1) = (x, x + w, ph - top - h, ph - top);
        let fill = fill.map(colour);
        let stroke = stroke.map(|(n, lw)| (colour(n), lw));
        let c = &mut self.page().content;
        c.begin_marked_content(Name(b"Artifact"));
        c.move_to(x0 + rb, y0);
        c.line_to(x1 - rb, y0);
        c.cubic_to(x1 - rb + rb * k, y0, x1, y0 + rb - rb * k, x1, y0 + rb);
        c.line_to(x1, y1 - rt);
        c.cubic_to(x1, y1 - rt + rt * k, x1 - rt + rt * k, y1, x1 - rt, y1);
        c.line_to(x0 + rt, y1);
        c.cubic_to(x0 + rt - rt * k, y1, x0, y1 - rt + rt * k, x0, y1 - rt);
        c.line_to(x0, y0 + rb);
        c.cubic_to(x0, y0 + rb - rb * k, x0 + rb - rb * k, y0, x0 + rb, y0);
        c.close_path();
        match (fill, stroke) {
            (Some([r, g, b]), Some(([sr, sg, sb], lw))) => {
                c.set_fill_rgb(r, g, b);
                c.set_stroke_rgb(sr, sg, sb);
                c.set_line_width(lw);
                c.fill_nonzero_and_stroke();
            }
            (Some([r, g, b]), None) => {
                c.set_fill_rgb(r, g, b);
                c.fill_nonzero();
            }
            (None, Some(([sr, sg, sb], lw))) => {
                c.set_stroke_rgb(sr, sg, sb);
                c.set_line_width(lw);
                c.stroke();
            }
            (None, None) => {
                c.end_path();
            }
        }
        c.end_marked_content();
    }

    /// Draw one line of text with its baseline `baseline` points below the
    /// page top, owned by structure element `owner` (or an artifact).
    #[allow(clippy::too_many_arguments)]
    fn text(
        &mut self,
        x: f32,
        baseline: f32,
        text: &str,
        face: Face,
        size: f32,
        ink: &str,
        spacing: f32,
        owner: Option<usize>,
    ) {
        if text.is_empty() {
            return;
        }
        let runs = self.fonts.encode(face, text);
        let [r, g, b] = colour(ink);
        let ph = self.h;
        let page_index = self.pages.len() - 1;
        let mcid = owner.map(|owner| {
            let page = self.page();
            page.mcids.push(owner);
            (page.mcids.len() - 1) as i32
        });
        if let (Some(owner), Some(mcid)) = (owner, mcid) {
            self.elems[owner].content.push((page_index, mcid));
        }
        let c = &mut self.page().content;
        match mcid {
            Some(mcid) => {
                c.begin_marked_content_with_properties(Name(b"Span"))
                    .properties()
                    .identify(mcid);
            }
            None => {
                c.begin_marked_content(Name(b"Artifact"));
            }
        }
        c.begin_text();
        c.set_fill_rgb(r, g, b);
        if spacing != 0.0 {
            c.set_char_spacing(spacing);
        }
        c.next_line(x, ph - baseline);
        for (key, bytes) in runs {
            c.set_font(Name(&resource_name(key)), size);
            c.show(Str(&bytes));
        }
        if spacing != 0.0 {
            c.set_char_spacing(0.0);
        }
        c.end_text();
        c.end_marked_content();
    }

    fn width(&self, face: Face, size: f32, text: &str) -> f32 {
        self.fonts.width(face, size, 0.0, text)
    }

    /// Lines of at most `max` points, broken at spaces; a word wider than a
    /// line is broken by characters, because the margin is the promise.
    fn wrap(&self, text: &str, face: Face, size: f32, max: f32) -> Vec<String> {
        let mut out = Vec::new();
        for paragraph in text.split('\n') {
            let mut line = String::new();
            for word in paragraph.split(' ').filter(|w| !w.is_empty()) {
                let candidate = if line.is_empty() {
                    word.to_string()
                } else {
                    format!("{line} {word}")
                };
                if self.width(face, size, &candidate) <= max {
                    line = candidate;
                    continue;
                }
                if !line.is_empty() {
                    out.push(std::mem::take(&mut line));
                }
                if self.width(face, size, word) <= max {
                    line = word.to_string();
                    continue;
                }
                for c in word.chars() {
                    let next = format!("{line}{c}");
                    if self.width(face, size, &next) > max && !line.is_empty() {
                        out.push(std::mem::replace(&mut line, c.to_string()));
                    } else {
                        line = next;
                    }
                }
            }
            out.push(line);
        }
        out
    }

    /// `text`, or as much of it as fits in `max` points followed by `…`.
    fn fit(&self, text: &str, face: Face, size: f32, max: f32) -> String {
        if self.width(face, size, text) <= max {
            return text.to_string();
        }
        let mut out: String = text.to_string();
        while !out.is_empty() && self.width(face, size, &format!("{out}…")) > max {
            out.pop();
        }
        format!("{out}…")
    }

    // ------------------------------------------------------------- pieces

    fn pill(&mut self, right: Option<f32>, x: f32, top: f32, text: &str, tone: &str) -> f32 {
        let size = 11.5 * PX;
        let dot = matches!(tone, "green" | "amber");
        let width =
            self.width(Face::Mono, size, text) + 2.0 * 9.0 * PX + if dot { 12.0 * PX } else { 0.0 };
        let x = right.map_or(x, |r| r - width);
        let (fill, edge, ink) = match tone {
            "green" => ("green-soft", "green-edge", "green-ink"),
            "amber" => ("amber-soft", "amber-edge", "amber-ink"),
            "blue" => ("blue-soft", "blue", "blue-ink"),
            _ => ("soft", "line", "ink-2"),
        };
        let h = 22.0 * PX;
        self.rounded(
            x,
            top,
            width,
            h,
            h / 2.0,
            Some(fill),
            Some((edge, 0.75)),
            (true, true),
        );
        let mut tx = x + 9.0 * PX;
        if dot {
            let d = 6.0 * PX;
            self.rounded(
                tx,
                top + (h - d) / 2.0,
                d,
                d,
                d / 2.0,
                Some(ink),
                None,
                (true, true),
            );
            tx += 12.0 * PX;
        }
        let doc = 0;
        self.text(
            tx,
            top + h / 2.0 + size * 0.35,
            text,
            Face::Mono,
            size,
            ink,
            0.0,
            Some(doc),
        );
        width
    }

    fn eyebrow(&mut self, x: f32, baseline: f32, text: &str, owner: Option<usize>) {
        let size = 10.5 * PX;
        self.text(
            x,
            baseline,
            &text.to_uppercase(),
            Face::MonoMedium,
            size,
            "muted",
            size * 0.08,
            owner,
        );
    }

    fn logo(&mut self, x: f32, top: f32, size: f32) {
        // The portal's mark: a 4×4 grid of rounded squares on a 1000-unit
        // canvas, in its own five colours. Drawn as paths, not an image.
        const GRID: [[&str; 4]; 4] = [
            ["#85A8B8", "#EFA53C", "#6E93A6", "#6E93A6"],
            ["#85A8B8", "#EFA53C", "#6E93A6", "#EFA53C"],
            ["#EFA53C", "#6E93A6", "#446980", "#446980"],
            ["#6E93A6", "#446980", "#EFA53C", "#2F4B60"],
        ];
        let unit = size / 1000.0;
        for (row, colours) in GRID.iter().enumerate() {
            for (col, hex) in colours.iter().enumerate() {
                let channel =
                    |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0) as f32 / 255.0;
                let (cx, cy) = (97.656 + col as f32 * 210.938, 97.656 + row as f32 * 210.938);
                let (bx, by, side, r) =
                    (x + cx * unit, top + cy * unit, 171.875 * unit, 31.25 * unit);
                let ph = self.h;
                let k = 0.552_284_8;
                let (x0, x1, y1, y0) = (bx, bx + side, ph - by, ph - by - side);
                let c = &mut self.page().content;
                c.begin_marked_content(Name(b"Artifact"));
                c.set_fill_rgb(channel(1), channel(3), channel(5));
                c.move_to(x0 + r, y0);
                c.line_to(x1 - r, y0);
                c.cubic_to(x1 - r + r * k, y0, x1, y0 + r - r * k, x1, y0 + r);
                c.line_to(x1, y1 - r);
                c.cubic_to(x1, y1 - r + r * k, x1 - r + r * k, y1, x1 - r, y1);
                c.line_to(x0 + r, y1);
                c.cubic_to(x0 + r - r * k, y1, x0, y1 - r + r * k, x0, y1 - r);
                c.line_to(x0, y0 + r);
                c.cubic_to(x0, y0 + r - r * k, x0 + r - r * k, y0, x0 + r, y0);
                c.close_path();
                c.fill_nonzero();
                c.end_marked_content();
            }
        }
    }
}

// ---------------------------------------------------------------- report

/// Every string the document will draw, so the fallback fonts it needs are
/// known before the first line is laid out.
fn all_text(report: &Report) -> String {
    let mut out = serde_json::to_string(report).unwrap_or_default();
    for table in report.tables() {
        for row in table.rows.iter().chain(table.total.iter()) {
            for cell in row {
                out.push_str(&cell.display);
            }
        }
    }
    out
}

impl Report {
    /// The document, typeset.
    pub(super) fn pdf(&self) -> Vec<u8> {
        let fallback = load_fallbacks(&all_text(self));
        let mut w = Writer::new(Fonts::new(&fallback));
        self.lay_out(&mut w);
        write(self, w)
    }

    fn lay_out(&self, w: &mut Writer<'_>) {
        let doc = 0;
        let (left, inner) = (w.left(), w.inner());
        let kind_label = super::KINDS
            .iter()
            .find(|(k, _)| *k == self.kind)
            .map_or(self.title.as_str(), |(_, t)| t);

        // Top bar.
        let top = w.y;
        let mark = 24.0 * PX;
        w.logo(left, top, mark);
        let mut x = left + mark + 10.0 * PX;
        let base = top + mark / 2.0 + 15.0 * PX * 0.35;
        w.text(
            x,
            base,
            "Semlith",
            Face::SansBold,
            15.0 * PX,
            "ink",
            0.0,
            None,
        );
        x += w.width(Face::SansBold, 15.0 * PX, "Semlith") + 10.0 * PX;
        w.vline(x, top + 4.0, top + mark - 4.0, "line");
        x += 10.0 * PX;
        w.text(
            x,
            base,
            kind_label,
            Face::Sans,
            13.5 * PX,
            "ink-2",
            0.0,
            None,
        );
        let pill = match &self.origin {
            None => (
                "generated on this machine · nothing uploaded".to_string(),
                "green",
            ),
            Some(origin) => (format!("generated in {origin}"), "blue"),
        };
        w.pill(
            Some(left + inner),
            0.0,
            top + (mark - 22.0 * PX) / 2.0,
            &pill.0,
            pill.1,
        );
        w.y = top + mark + 14.0 * PX;
        w.hline(left, w.y, inner, "line", 0.75);

        // Title block.
        w.y += 22.0 * PX;
        w.eyebrow(
            left,
            w.y + 10.5 * PX,
            &format!("{kind_label} · {}", self.audience),
            None,
        );
        w.y += 10.5 * PX + 6.0 * PX;
        let h1 = w.elem(StructRole::H1, doc);
        let h1_size = 26.0 * PX;
        for line in w.wrap(&self.title, Face::SansBold, h1_size, inner) {
            w.y += h1_size * 1.2;
            w.text(
                left,
                w.y - h1_size * 0.22,
                &line,
                Face::SansBold,
                h1_size,
                "ink",
                0.0,
                Some(h1),
            );
        }
        w.y += 8.0 * PX;
        if !self.lede.is_empty() {
            let p = w.elem(StructRole::P, doc);
            self.lede(w, p, left, inner.min(68.0 * 15.0 * PX * 0.55));
        }

        // Meta strip.
        if !self.meta.is_empty() {
            w.y += 18.0 * PX;
            self.meta_strip(w);
        }
        // KPI row.
        if !self.kpis.is_empty() {
            w.y += 20.0 * PX;
            self.kpi_row(w);
        }
        for block in &self.blocks {
            w.y += 16.0 * PX;
            self.block(w, block);
        }
        // The full ledger head where the HTML prints it; the running footer
        // carries the short form on every page.
        w.y += 22.0 * PX;
        w.room(30.0);
        w.hline(left, w.y, inner, "line", 0.75);
        let foot = w.elem(StructRole::P, doc);
        let size = 11.5 * PX;
        for line in w.wrap(&self.footer(" · "), Face::Mono, size, inner) {
            w.y += size * 1.6;
            w.text(left, w.y, &line, Face::Mono, size, "muted", 0.0, Some(foot));
        }
    }

    fn lede(&self, w: &mut Writer<'_>, owner: usize, left: f32, max: f32) {
        let size = 15.0 * PX;
        // Words with their weight, wrapped together.
        let mut words: Vec<(String, bool)> = Vec::new();
        for run in &self.lede {
            for (i, word) in run.text.split(' ').enumerate() {
                // `split` yields an empty first piece for a leading space, so
                // every space is exactly one `i > 0`.
                if i > 0 {
                    words.push((" ".into(), run.bold));
                }
                if !word.is_empty() {
                    words.push((word.to_string(), run.bold));
                }
            }
        }
        let face = |bold: bool| if bold { Face::SansBold } else { Face::Sans };
        let mut lines: Vec<Vec<(String, bool)>> = vec![Vec::new()];
        let mut x = 0.0;
        for (word, bold) in words {
            let width = w.width(face(bold), size, &word);
            if word == " " {
                if x > 0.0 {
                    lines.last_mut().unwrap().push((word, bold));
                    x += width;
                }
                continue;
            }
            if x + width > max && x > 0.0 {
                if let Some(line) = lines.last_mut()
                    && line.last().is_some_and(|(w, _)| w == " ")
                {
                    line.pop();
                }
                lines.push(Vec::new());
                x = 0.0;
            }
            lines.last_mut().unwrap().push((word, bold));
            x += width;
        }
        for line in lines {
            w.y += size * 1.5;
            let mut x = left;
            // Consecutive words of one weight are drawn as one run.
            let mut merged: Vec<(String, bool)> = Vec::new();
            for (word, bold) in line {
                match merged.last_mut() {
                    Some((text, b)) if *b == bold => text.push_str(&word),
                    _ => merged.push((word, bold)),
                }
            }
            for (text, bold) in merged {
                let ink = if bold { "ink" } else { "ink-2" };
                w.text(
                    x,
                    w.y - size * 0.35,
                    &text,
                    face(bold),
                    size,
                    ink,
                    0.0,
                    Some(owner),
                );
                x += w.width(face(bold), size, &text);
            }
        }
    }

    fn meta_strip(&self, w: &mut Writer<'_>) {
        let (left, inner) = (w.left(), w.inner());
        let per_row = 3.min(self.meta.len()).max(1);
        let cell_w = inner / per_row as f32;
        let mono = 12.5 * PX;
        let sect = w.elem(StructRole::Sect, 0);
        // Measure each row first, so the strip is drawn as one box.
        let rows: Vec<&[super::Meta]> = self.meta.chunks(per_row).collect();
        let heights: Vec<f32> = rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|m| {
                        let lines: usize = m
                            .lines
                            .iter()
                            .map(|l| w.wrap(l, Face::Mono, mono, cell_w - 2.0 * PAD_X).len())
                            .sum();
                        10.0 * PX * 2.0
                            + 10.5 * PX
                            + 4.0
                            + if m.chips.is_empty() {
                                0.0
                            } else {
                                22.0 * PX + 4.0
                            }
                            + lines as f32 * mono * 1.45
                    })
                    .fold(0.0, f32::max)
            })
            .collect();
        let total: f32 = heights.iter().sum();
        w.room(total);
        let top = w.y;
        w.rounded(
            left,
            top,
            inner,
            total,
            RADIUS,
            Some("panel"),
            Some(("line", 0.75)),
            (true, true),
        );
        let mut y = top;
        for (r, row) in rows.iter().enumerate() {
            if r > 0 {
                w.hline(left, y, inner, "line-soft", 0.75);
            }
            for (i, m) in row.iter().enumerate() {
                let x = left + i as f32 * cell_w;
                if i > 0 {
                    w.vline(x, y, y + heights[r], "line-soft");
                }
                let owner = w.elem(StructRole::P, sect);
                let mut cy = y + 10.0 * PX + 10.5 * PX;
                w.eyebrow(x + PAD_X, cy - 1.5, &m.label, Some(owner));
                cy += 4.0;
                if !m.chips.is_empty() {
                    let mut cx = x + PAD_X;
                    for chip in &m.chips {
                        let pw = w.pill(None, cx, cy, &chip.text, chip.tone);
                        cx += pw + 4.0;
                    }
                    cy += 22.0 * PX + 4.0;
                }
                for (j, line) in m.lines.iter().enumerate() {
                    let ink = if j == 0 && m.chips.is_empty() {
                        "ink"
                    } else {
                        "muted"
                    };
                    for piece in w.wrap(line, Face::Mono, mono, cell_w - 2.0 * PAD_X) {
                        cy += mono * 1.45;
                        w.text(
                            x + PAD_X,
                            cy - mono * 0.3,
                            &piece,
                            Face::Mono,
                            mono,
                            ink,
                            0.0,
                            Some(owner),
                        );
                    }
                }
            }
            y += heights[r];
        }
        w.y = top + total;
    }

    fn kpi_row(&self, w: &mut Writer<'_>) {
        let (left, inner) = (w.left(), w.inner());
        let n = self.kpis.len();
        let gap = 12.0 * PX;
        let box_w = (inner - gap * (n as f32 - 1.0)) / n as f32;
        let value = 22.0 * PX;
        let sub = 12.0 * PX;
        let sect = w.elem(StructRole::Sect, 0);
        let height = self
            .kpis
            .iter()
            .map(|k| {
                let lines = w
                    .wrap(&k.figure.over, Face::Sans, sub, box_w - 2.0 * PAD_X)
                    .len();
                12.0 * PX * 2.0 + 10.5 * PX + 4.0 + value * 1.25 + 4.0 + lines as f32 * sub * 1.45
            })
            .fold(0.0, f32::max);
        w.room(height);
        let top = w.y;
        for (i, k) in self.kpis.iter().enumerate() {
            let x = left + i as f32 * (box_w + gap);
            let (edge, lw) = if k.lead {
                ("accent", 1.1)
            } else {
                ("line", 0.75)
            };
            w.rounded(
                x,
                top,
                box_w,
                height,
                RADIUS,
                Some("panel"),
                Some((edge, lw)),
                (true, true),
            );
            let owner = w.elem(StructRole::P, sect);
            let mut y = top + 12.0 * PX + 10.5 * PX;
            w.eyebrow(x + PAD_X, y - 1.5, &k.label, Some(owner));
            y += 4.0 + value * 1.1;
            // A figure never wraps; one too wide for its box steps down a size.
            let mut size = value;
            while size > 10.0
                && w.width(Face::MonoMedium, size, &k.figure.display) > box_w - 2.0 * PAD_X
            {
                size -= 0.5;
            }
            w.text(
                x + PAD_X,
                y,
                &k.figure.display,
                Face::MonoMedium,
                size,
                "ink",
                0.0,
                Some(owner),
            );
            y += value * 0.15 + 4.0;
            for line in w.wrap(&k.figure.over, Face::Sans, sub, box_w - 2.0 * PAD_X) {
                y += sub * 1.45;
                w.text(
                    x + PAD_X,
                    y - sub * 0.3,
                    &line,
                    Face::Sans,
                    sub,
                    "muted",
                    0.0,
                    Some(owner),
                );
            }
        }
        w.y = top + height;
    }

    fn block(&self, w: &mut Writer<'_>, block: &Block) {
        let sect = w.elem(StructRole::Sect, 0);
        let mut card = Card::open(w, sect);
        match block {
            Block::Table(table) => {
                card.head(w, &table.title, &table.meta, sect);
                table_body(w, &mut card, table, sect);
            }
            Block::Facts {
                title,
                meta,
                facts,
                note,
            } => {
                card.head(w, title, meta, sect);
                let (left, inner) = (w.left(), w.inner());
                let cols = 3;
                let cell_w = inner / cols as f32;
                let value = 17.0 * PX;
                let sub = 12.0 * PX;
                for (r, row) in facts.chunks(cols).enumerate() {
                    let height = row
                        .iter()
                        .map(|f| {
                            let lines = w
                                .wrap(&f.figure.over, Face::Sans, sub, cell_w - 2.0 * PAD_X)
                                .len();
                            12.0 * PX * 2.0
                                + 10.5 * PX
                                + 4.0
                                + value * 1.3
                                + lines as f32 * sub * 1.45
                        })
                        .fold(0.0, f32::max);
                    card.room(w, height, None);
                    let top = w.y;
                    if r > 0 {
                        w.hline(left, top, inner, "line-soft", 0.75);
                    }
                    for (i, fact) in row.iter().enumerate() {
                        let x = left + i as f32 * cell_w;
                        if i > 0 {
                            w.vline(x, top, top + height, "line-soft");
                        }
                        let owner = w.elem(StructRole::P, sect);
                        let mut y = top + 12.0 * PX + 10.5 * PX;
                        w.eyebrow(x + PAD_X, y - 1.5, &fact.label, Some(owner));
                        y += 4.0 + value * 1.1;
                        let shown = w.fit(
                            &fact.figure.display,
                            Face::MonoMedium,
                            value,
                            cell_w - 2.0 * PAD_X,
                        );
                        w.text(
                            x + PAD_X,
                            y,
                            &shown,
                            Face::MonoMedium,
                            value,
                            "ink",
                            0.0,
                            Some(owner),
                        );
                        y += value * 0.2;
                        for line in w.wrap(&fact.figure.over, Face::Sans, sub, cell_w - 2.0 * PAD_X)
                        {
                            y += sub * 1.45;
                            w.text(
                                x + PAD_X,
                                y - sub * 0.3,
                                &line,
                                Face::Sans,
                                sub,
                                "muted",
                                0.0,
                                Some(owner),
                            );
                        }
                    }
                    w.y = top + height;
                }
                if let Some(note) = note {
                    card.note(w, note, sect);
                }
            }
            Block::Note { text } => card.note(w, text, sect),
            Block::Code {
                title,
                meta,
                comment,
                lines,
            } => {
                card.head(w, title, meta, sect);
                let code = w.elem(StructRole::Code, sect);
                let size = 12.0 * PX;
                let (left, inner) = (w.left(), w.inner());
                let mut all: Vec<(String, &str)> = vec![(format!("# {comment}"), "code-dim")];
                for line in lines {
                    for piece in w.wrap(line, Face::Mono, size, inner - 2.0 * PAD_X) {
                        all.push((piece, "code-ink"));
                    }
                }
                let height = 2.0 * 12.0 * PX + all.len() as f32 * size * 1.6;
                card.room(w, height, None);
                let top = w.y;
                w.fill(left + 0.4, top, inner - 0.8, height, "code-bg");
                let mut y = top + 12.0 * PX;
                for (line, ink) in all {
                    y += size * 1.6;
                    w.text(
                        left + PAD_X,
                        y - size * 0.4,
                        &line,
                        Face::Mono,
                        size,
                        ink,
                        0.0,
                        Some(code),
                    );
                }
                w.y = top + height;
            }
        }
        card.close(w);
    }
}

/// A card being drawn: its border is stroked per page segment once the
/// segment's height is known, square where it continues over a page.
struct Card {
    top: f32,
    first: bool,
}

impl Card {
    fn open(w: &mut Writer<'_>, _sect: usize) -> Self {
        // A card head never sits alone at the foot of a page.
        w.room(80.0);
        Self {
            top: w.y,
            first: true,
        }
    }

    fn head(&mut self, w: &mut Writer<'_>, title: &str, meta: &str, sect: usize) {
        let (left, inner) = (w.left(), w.inner());
        let height = 41.0 * PX;
        w.marks.push(Mark {
            title: title.to_string(),
            page: w.pages.len() - 1,
            top: w.y,
        });
        let h2 = w.elem(StructRole::H2, sect);
        let size = 13.5 * PX;
        let base = w.y + height / 2.0 + size * 0.35;
        w.text(
            left + PAD_X,
            base,
            title,
            Face::SansBold,
            size,
            "ink",
            0.0,
            Some(h2),
        );
        let meta = meta.replace('`', "");
        let meta_size = 11.5 * PX;
        let title_w = w.width(Face::SansBold, size, title);
        let room = inner - 2.0 * PAD_X - title_w - 12.0;
        let shown = w.fit(&meta, Face::Mono, meta_size, room.max(40.0));
        let mw = w.width(Face::Mono, meta_size, &shown);
        w.text(
            left + inner - PAD_X - mw,
            base,
            &shown,
            Face::Mono,
            meta_size,
            "muted",
            0.0,
            None,
        );
        w.y += height;
        w.hline(left, w.y, inner, "line-soft", 0.75);
    }

    /// Make room for `height` more; on a page break, close this page's
    /// segment, and repeat a table's header on the next.
    fn room(
        &mut self,
        w: &mut Writer<'_>,
        height: f32,
        header: Option<&dyn Fn(&mut Writer<'_>)>,
    ) -> bool {
        if w.y + height <= w.bottom() {
            return false;
        }
        self.stroke(w, false);
        w.new_page();
        self.top = w.y;
        self.first = false;
        if let Some(header) = header {
            header(w);
        }
        true
    }

    fn note(&mut self, w: &mut Writer<'_>, text: &str, sect: usize) {
        let (left, inner) = (w.left(), w.inner());
        let size = 12.5 * PX;
        let text = text.replace('`', "");
        let lines = w.wrap(&text, Face::Sans, size, inner - 2.0 * PAD_X);
        let height = 2.0 * 10.0 * PX + lines.len() as f32 * size * 1.5;
        self.room(w, height, None);
        let top = w.y;
        w.fill(left + 0.4, top, inner - 0.8, height - 0.4, "soft");
        w.hline(left, top, inner, "line-soft", 0.75);
        let p = w.elem(StructRole::P, sect);
        let mut y = top + 10.0 * PX;
        for line in lines {
            y += size * 1.5;
            w.text(
                left + PAD_X,
                y - size * 0.4,
                &line,
                Face::Sans,
                size,
                "ink-2",
                0.0,
                Some(p),
            );
        }
        w.y = top + height;
    }

    fn stroke(&self, w: &mut Writer<'_>, last: bool) {
        let (left, inner) = (w.left(), w.inner());
        let height = w.y - self.top;
        w.rounded(
            left,
            self.top,
            inner,
            height,
            RADIUS,
            None,
            Some(("line", 0.75)),
            (self.first, last),
        );
    }

    fn close(self, w: &mut Writer<'_>) {
        self.stroke(w, true);
    }
}

/// How one table fits the page: column widths, the body size and the cell
/// padding. A table too wide for the page at the print size steps both down
/// before anything is cut, as a browser's print shrinks a wide table.
struct Fit {
    cols: Vec<f32>,
    size: f32,
    pad: f32,
}

fn face_of(kind: Kind) -> Face {
    match kind {
        Kind::Num | Kind::Bar | Kind::Id => Face::Mono,
        Kind::Strong => Face::SansBold,
        _ => Face::Sans,
    }
}

/// An id's face is a step smaller than the body, as the HTML's `.id`.
fn cell_size(kind: Kind, size: f32) -> f32 {
    if kind == Kind::Id {
        size * 12.0 / 13.0
    } else {
        size
    }
}

const HEAD: f32 = 10.5 * PX;

fn header_lines(w: &Writer<'_>, label: &str, width: f32) -> Vec<String> {
    w.wrap(&label.to_uppercase(), Face::MonoMedium, HEAD, width)
}

fn fit_table(w: &Writer<'_>, table: &Table) -> Fit {
    let inner = w.inner();
    let flexible = |k: Kind| matches!(k, Kind::Text | Kind::Muted | Kind::Strong);
    let mut size = 13.0 * PX;
    let mut pad = PAD_X;
    loop {
        // A header wraps at its spaces, so its narrowest is its longest word.
        let measure = |i: usize, kind: Kind| {
            let longest_word = table.columns[i]
                .label
                .to_uppercase()
                .split(' ')
                .map(|word| w.fonts.width(Face::MonoMedium, HEAD, HEAD * 0.08, word))
                .fold(0.0, f32::max);
            let cells = table
                .rows
                .iter()
                .chain(table.total.iter())
                .filter_map(|row| row.get(i))
                .map(|cell| w.width(face_of(kind), cell_size(kind, size), &cell.display))
                .fold(0.0, f32::max);
            let bar = if kind == Kind::Bar { 70.0 } else { 0.0 };
            (longest_word, cells + bar)
        };
        let spans: Vec<(f32, f32)> = table
            .columns
            .iter()
            .enumerate()
            .map(|(i, c)| measure(i, c.kind))
            .collect();
        // Numbers never shrink; an id wraps at its own spaces (a stamp's date
        // and time) and a word of it still too wide wraps within eleven
        // characters; prose wraps down to its header's longest word.
        let id_cap = w.width(Face::Mono, cell_size(Kind::Id, size), "01234567890");
        let floor: Vec<f32> = table
            .columns
            .iter()
            .enumerate()
            .zip(&spans)
            .map(|((i, c), (word, cells))| {
                let body = match c.kind {
                    Kind::Num | Kind::Bar => *cells,
                    Kind::Id => table
                        .rows
                        .iter()
                        .filter_map(|row| row.get(i))
                        .flat_map(|cell| cell.display.split(' '))
                        .map(|part| w.width(Face::Mono, cell_size(Kind::Id, size), part))
                        .fold(0.0, f32::max)
                        .min(id_cap),
                    _ => cells.min(60.0),
                };
                word.max(body) + 2.0 * pad
            })
            .collect();
        let natural: Vec<f32> = spans
            .iter()
            .map(|(word, cells)| word.max(*cells) + 2.0 * pad)
            .collect();
        if floor.iter().sum::<f32>() > inner && size > 7.5 {
            size -= 0.5;
            pad = (pad - 0.75).max(5.0);
            continue;
        }
        let mut cols = floor.clone();
        // Still too wide at the smallest size: the widest columns that wrap
        // come down to one level until the table ends inside the margin, so
        // no column is drawn past it. Numbers keep their width.
        let wraps = |k: Kind| !matches!(k, Kind::Num | Kind::Bar);
        if cols.iter().sum::<f32>() > inner {
            let mut widths: Vec<f32> = Vec::new();
            let mut room = inner;
            for (c, width) in table.columns.iter().zip(&cols) {
                if wraps(c.kind) {
                    widths.push(*width);
                } else {
                    room -= width;
                }
            }
            widths.sort_by(f32::total_cmp);
            let mut level = 0.0;
            for (k, width) in widths.iter().enumerate() {
                let rest = (widths.len() - k) as f32;
                if width * rest >= room {
                    level = room / rest;
                    break;
                }
                room -= width;
            }
            let level = level.max(2.0 * pad + size);
            for (c, width) in table.columns.iter().zip(&mut cols) {
                if wraps(c.kind) {
                    *width = width.min(level);
                }
            }
        }
        let mut spare = inner - cols.iter().sum::<f32>();
        // Give back what was taken, ids first, so a stamp or a path is whole
        // wherever the page has room for it, then prose.
        for pass in [Kind::Id, Kind::Strong, Kind::Text, Kind::Muted] {
            for i in 0..cols.len() {
                if table.columns[i].kind == pass && spare > 0.0 {
                    let want = (natural[i] - cols[i]).max(0.0).min(spare);
                    cols[i] += want;
                    spare -= want;
                }
            }
        }
        if spare > 0.0 {
            let target = table
                .columns
                .iter()
                .rposition(|c| flexible(c.kind))
                .unwrap_or(table.columns.len().saturating_sub(1));
            if let Some(width) = cols.get_mut(target) {
                *width += spare;
            }
        }
        return Fit { cols, size, pad };
    }
}

/// An id on as few lines as its own spaces allow. A word too wide for the
/// column wraps rather than losing any of itself, because a session or a path
/// is told apart by its middle as often as by its ends.
fn wrap_id(w: &Writer<'_>, text: &str, size: f32, max: f32) -> Vec<String> {
    if w.width(Face::Mono, size, text) <= max {
        return vec![text.to_string()];
    }
    let mut out: Vec<String> = Vec::new();
    for word in text.split(' ').filter(|p| !p.is_empty()) {
        // A path breaks after a `/`, which is a boundary between its tokens;
        // a segment still too wide, or a word with no `/`, breaks at the
        // character that fills the line.
        let pieces: Vec<String> = if w.width(Face::Mono, size, word) > max {
            word.split_inclusive('/')
                .flat_map(|segment| {
                    if w.width(Face::Mono, size, segment) > max {
                        segment.chars().map(String::from).collect()
                    } else {
                        vec![segment.to_string()]
                    }
                })
                .collect()
        } else {
            vec![word.to_string()]
        };
        for (i, piece) in pieces.into_iter().enumerate() {
            let joiner = if i == 0 { " " } else { "" };
            match out.last_mut() {
                Some(line)
                    if w.width(Face::Mono, size, &format!("{line}{joiner}{piece}")) <= max =>
                {
                    line.push_str(joiner);
                    line.push_str(&piece);
                }
                _ => out.push(piece),
            }
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// A cell's lines in `room` points: prose wraps, ids wrap at their own
/// boundaries, numbers do not.
fn cell_lines(w: &Writer<'_>, kind: Kind, text: &str, size: f32, room: f32) -> Vec<String> {
    match kind {
        Kind::Text | Kind::Muted => w.wrap(text, Face::Sans, size, room),
        Kind::Strong => w.wrap(text, Face::SansBold, size, room),
        Kind::Id => wrap_id(w, text, cell_size(Kind::Id, size), room),
        Kind::Num | Kind::Bar => vec![text.to_string()],
    }
}

fn table_body(w: &mut Writer<'_>, card: &mut Card, table: &Table, sect: usize) {
    let (left, inner) = (w.left(), w.inner());
    if table.rows.is_empty() {
        card.note(w, &table.empty, sect);
        return;
    }
    let Fit { cols, size, pad } = fit_table(w, table);
    let tag = w.elem(StructRole::Table, sect);
    let labels: Vec<Vec<String>> = table
        .columns
        .iter()
        .zip(&cols)
        .map(|(c, width)| header_lines(w, &c.label, width - 2.0 * pad))
        .collect();
    let head_lines = labels.iter().map(Vec::len).max().unwrap_or(1).max(1);
    let head_h = 2.0 * 8.0 * PX + HEAD * 1.4 * head_lines as f32;
    let header = |w: &mut Writer<'_>| {
        let top = w.y;
        w.fill(left + 0.4, top, inner - 0.8, head_h, "soft");
        let tr = w.elem(StructRole::TR, tag);
        let mut x = left;
        for ((column, width), lines) in table.columns.iter().zip(&cols).zip(&labels) {
            let th = w.elem(StructRole::TH, tr);
            let spacing = HEAD * 0.08;
            let mut y = top + 8.0 * PX;
            for line in lines {
                y += HEAD * 1.4;
                let lw = w.fonts.width(Face::MonoMedium, HEAD, spacing, line);
                let tx = if column.kind == Kind::Num {
                    x + width - pad - lw
                } else {
                    x + pad
                };
                w.text(
                    tx,
                    y - HEAD * 0.4,
                    line,
                    Face::MonoMedium,
                    HEAD,
                    "muted",
                    spacing,
                    Some(th),
                );
            }
            x += width;
        }
        w.y = top + head_h;
        w.hline(left, w.y, inner, "line-soft", 0.75);
    };
    card.room(w, head_h + 30.0, None);
    header(w);
    let rows: Vec<(&Vec<Cell>, bool)> = table
        .rows
        .iter()
        .map(|r| (r, false))
        .chain(table.total.iter().map(|r| (r, true)))
        .collect();
    let mut first_bar = true;
    for (index, (row, total)) in rows.into_iter().enumerate() {
        // Lay the row out: prose and ids wrap, numbers do not.
        let laid: Vec<Vec<String>> = table
            .columns
            .iter()
            .zip(&cols)
            .zip(row)
            .map(|((column, width), cell)| {
                cell_lines(w, column.kind, &cell.display, size, width - 2.0 * pad)
            })
            .collect();
        let lines = laid.iter().map(Vec::len).max().unwrap_or(1).max(1);
        let height = 2.0 * 9.0 * PX + lines as f32 * size * 1.45;
        card.room(w, height, Some(&header));
        let top = w.y;
        if total {
            w.fill(left + 0.4, top, inner - 0.8, height - 0.4, "soft");
            w.hline(left, top, inner, "line", 0.75);
        } else if index > 0 {
            w.hline(left, top, inner, "line-soft", 0.75);
        }
        let tr = w.elem(StructRole::TR, tag);
        let mut x = left;
        for (((column, width), cell), pieces) in table.columns.iter().zip(&cols).zip(row).zip(laid)
        {
            let td = w.elem(StructRole::TD, tr);
            let (face, ink) = match (column.kind, total) {
                (Kind::Num, true) => (Face::MonoMedium, "ink"),
                (Kind::Muted, true) => (Face::SansBold, "ink-2"),
                (_, true) => (Face::SansBold, "ink"),
                (Kind::Num, _) => (Face::Mono, "ink"),
                (Kind::Id, _) => (Face::Mono, "ink-2"),
                (Kind::Strong, _) => (Face::SansBold, "ink"),
                (Kind::Muted, _) => (Face::Sans, "muted"),
                (Kind::Bar, _) => (Face::Mono, "ink-2"),
                (Kind::Text, _) => (Face::Sans, "ink"),
            };
            let fs = cell_size(column.kind, size);
            if column.kind == Kind::Bar && !cell.value.is_null() {
                let share = cell.value.as_f64().unwrap_or(0.0).clamp(0.0, 1.0) as f32;
                let bar_w = 60.0;
                let mid = top + height / 2.0;
                w.rounded(
                    x + pad,
                    mid - 2.25,
                    bar_w,
                    4.5,
                    2.25,
                    Some("chip"),
                    None,
                    (true, true),
                );
                if share > 0.0 {
                    let fill = if first_bar { "accent" } else { "blue" };
                    w.rounded(
                        x + pad,
                        mid - 2.25,
                        (bar_w * share).max(4.5),
                        4.5,
                        2.25,
                        Some(fill),
                        None,
                        (true, true),
                    );
                }
                first_bar = false;
                w.text(
                    x + pad + bar_w + 7.5,
                    mid + fs * 0.35,
                    &cell.display,
                    face,
                    fs,
                    ink,
                    0.0,
                    Some(td),
                );
                x += width;
                continue;
            }
            let mut y = top + 9.0 * PX;
            for piece in pieces {
                y += size * 1.45;
                let tx = if column.kind == Kind::Num {
                    x + width - pad - w.width(face, fs, &piece)
                } else {
                    x + pad
                };
                w.text(tx, y - size * 0.4, &piece, face, fs, ink, 0.0, Some(td));
            }
            x += width;
        }
        w.y = top + height;
    }
    if table.more > 0 {
        card.note(
            w,
            &format!("{} more not listed.", super::num::int(table.more as i64)),
            sect,
        );
    }
    if let Some(note) = &table.note {
        card.note(w, note, sect);
    }
}

// ----------------------------------------------------------------- write

fn deflate(bytes: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    let _ = encoder.write_all(bytes);
    encoder.finish().unwrap_or_default()
}

/// Six capital letters naming a subset, from the glyphs in it, so two
/// different subsets of one font never share a name in a merged document.
fn subset_tag(used: &Used) -> String {
    let mut hasher = blake3::Hasher::new();
    for (gid, c) in used.cids.keys() {
        hasher.update(&gid.to_be_bytes());
        hasher.update(c.to_string().as_bytes());
    }
    hasher
        .finalize()
        .as_bytes()
        .iter()
        .take(6)
        .map(|b| (b'A' + b % 26) as char)
        .collect()
}

fn pdf_date(report: &Report) -> Date {
    // `2026-10-14T20:13:04+05:30`
    let s = &report.generated_at;
    let num = |a: usize, b: usize| s.get(a..b).and_then(|v| v.parse::<u16>().ok()).unwrap_or(0);
    let mut date = Date::new(num(0, 4))
        .month(num(5, 7) as u8)
        .day(num(8, 10) as u8)
        .hour(num(11, 13) as u8)
        .minute(num(14, 16) as u8)
        .second(num(17, 19) as u8);
    if let Some(sign) = s.get(19..20) {
        let hours = num(20, 22) as i8;
        date = date
            .utc_offset_hour(if sign == "-" { -hours } else { hours })
            .utc_offset_minute(num(23, 25) as u8);
    }
    date
}

fn write(report: &Report, mut w: Writer<'_>) -> Vec<u8> {
    // The running footer, now that the page count is known.
    let pages = w.pages.len();
    let head = report
        .chain
        .as_ref()
        .and_then(|c| c.head.as_deref())
        .map(|h| format!(" · ledger head {}", super::short_hash(h)))
        .unwrap_or_default();
    let left_text = format!("semlith {} · {}{head}", report.version, report.generated);
    for page in 0..pages {
        let size = 10.0 * PX;
        let y = w.h - BOTTOM + 14.0;
        let right = format!("page {} of {pages}", page + 1);
        let (left, inner) = (w.left(), w.inner());
        let rw = w.width(Face::Mono, size, &right);
        // Drawn on that page: swap it in, draw, swap back.
        let last = w.pages.len() - 1;
        w.pages.swap(page, last);
        w.hline(left, w.h - BOTTOM + 4.0, inner, "line-soft", 0.75);
        let shown = w.fit(&left_text, Face::Mono, size, inner - rw - 20.0);
        w.text(left, y, &shown, Face::Mono, size, "muted", 0.0, None);
        w.text(
            left + inner - rw,
            y,
            &right,
            Face::Mono,
            size,
            "muted",
            0.0,
            None,
        );
        w.pages.swap(page, last);
    }

    let mut pdf = Pdf::new();
    let mut next = 1;
    let mut alloc = || {
        let r = Ref::new(next);
        next += 1;
        r
    };
    let catalog = alloc();
    let tree = alloc();
    let info = alloc();
    let struct_root = alloc();
    let outline = alloc();
    let page_refs: Vec<Ref> = (0..pages).map(|_| alloc()).collect();
    let content_refs: Vec<Ref> = (0..pages).map(|_| alloc()).collect();
    let elem_refs: Vec<Ref> = (0..w.elems.len()).map(|_| alloc()).collect();
    let mark_refs: Vec<Ref> = (0..w.marks.len()).map(|_| alloc()).collect();
    let parent_refs: Vec<Ref> = (0..pages).map(|_| alloc()).collect();
    let keys: Vec<FontKey> = w.fonts.used.keys().copied().collect();
    let font_refs: BTreeMap<FontKey, [Ref; 6]> = keys
        .iter()
        .map(|k| (*k, [alloc(), alloc(), alloc(), alloc(), alloc(), alloc()]))
        .collect();

    // Catalog: tagged, in English, titled, with an outline.
    {
        let mut cat = pdf.catalog(catalog);
        cat.pages(tree);
        cat.lang(TextStr("en"));
        cat.mark_info().marked(true);
        cat.viewer_preferences().display_doc_title(true);
        cat.pair(Name(b"StructTreeRoot"), struct_root);
        if !w.marks.is_empty() {
            cat.outlines(outline);
            cat.page_mode(pdf_writer::types::PageMode::UseOutlines);
        }
    }
    {
        let title = format!("{} — Semlith", report.title);
        let producer = format!("semlith {}", report.version);
        let date = pdf_date(report);
        let mut doc = pdf.document_info(info);
        doc.title(TextStr(&title));
        doc.author(TextStr("Semlith"));
        doc.creator(TextStr(&format!("semlith report {}", report.kind)));
        doc.producer(TextStr(&producer));
        doc.subject(TextStr(&report.lede_plain()));
        doc.creation_date(date);
        doc.modified_date(date);
    }
    pdf.pages(tree)
        .kids(page_refs.iter().copied())
        .count(pages as i32);

    for i in 0..pages {
        let mut p = pdf.page(page_refs[i]);
        p.parent(tree);
        p.media_box(Rect::new(0.0, 0.0, w.w, w.h));
        p.contents(content_refs[i]);
        p.struct_parents(i as i32);
        p.tab_order(pdf_writer::types::TabOrder::StructureOrder);
        let mut resources = p.resources();
        let mut fonts = resources.fonts();
        for key in &keys {
            fonts.pair(Name(&resource_name(*key)), font_refs[key][0]);
        }
    }
    // Each page's content, compressed; its MCID owners stay for the parent tree.
    let mut owners: Vec<Vec<usize>> = Vec::with_capacity(pages);
    for (i, page) in std::mem::take(&mut w.pages).into_iter().enumerate() {
        let packed = deflate(&page.content.finish());
        pdf.stream(content_refs[i], &packed)
            .filter(Filter::FlateDecode);
        owners.push(page.mcids);
    }

    // Fonts: a Type0 over one CID font each, subset, with real widths and a
    // ToUnicode map.
    for key in &keys {
        let [type0, cid, descriptor, file, cmap, gidmap] = font_refs[key];
        let used = &w.fonts.used[key];
        let source = w.fonts.source(*key);
        let base = format!("{}+{}", subset_tag(used), source.name);
        let program =
            subsetter::subset(source.data, source.index, &used.remapper).unwrap_or_default();
        pdf.type0_font(type0)
            .base_font(Name(base.as_bytes()))
            .encoding_predefined(Name(b"Identity-H"))
            .descendant_font(cid)
            .to_unicode(cmap);
        {
            let mut font = pdf.cid_font(cid);
            font.subtype(if source.cff {
                CidFontType::Type0
            } else {
                CidFontType::Type2
            });
            font.base_font(Name(base.as_bytes()));
            font.system_info(SystemInfo {
                registry: Str(b"Adobe"),
                ordering: Str(b"Identity"),
                supplement: 0,
            });
            font.font_descriptor(descriptor);
            font.default_width(0.0);
            if !source.cff {
                font.cid_to_gid_map_stream(gidmap);
            }
            let mut widths = font.widths();
            for (cid, (_, _, width)) in &used.table {
                widths.consecutive(*cid, [*width]);
            }
        }
        let face = &source.face;
        let upem = f32::from(face.units_per_em());
        let scale = |v: i16| f32::from(v) * 1000.0 / upem;
        let bbox = face.global_bounding_box();
        let mut flags = FontFlags::SYMBOLIC;
        if source.mono {
            flags |= FontFlags::FIXED_PITCH;
        }
        {
            let mut d = pdf.font_descriptor(descriptor);
            d.name(Name(base.as_bytes()))
                .flags(flags)
                .bbox(Rect::new(
                    scale(bbox.x_min),
                    scale(bbox.y_min),
                    scale(bbox.x_max),
                    scale(bbox.y_max),
                ))
                .italic_angle(0.0)
                .ascent(scale(face.ascender()))
                .descent(scale(face.descender()))
                .cap_height(scale(face.capital_height().unwrap_or(face.ascender())))
                .stem_v(80.0);
            if source.cff {
                d.font_file3(file);
            } else {
                d.font_file2(file);
            }
        }
        let packed = deflate(&program);
        let mut stream = pdf.stream(file, &packed);
        stream.filter(Filter::FlateDecode);
        if source.cff {
            stream.pair(Name(b"Subtype"), Name(b"OpenType"));
        }
        stream.finish();
        let mut map = UnicodeCmap::new(
            Name(b"Semlith-UCS"),
            SystemInfo {
                registry: Str(b"Adobe"),
                ordering: Str(b"UCS"),
                supplement: 0,
            },
        );
        for (cid, (_, c, _)) in &used.table {
            map.pair(*cid, *c);
        }
        pdf.stream(cmap, &map.finish());
        if !source.cff {
            let top = used.table.keys().next_back().copied().unwrap_or(0);
            let mut bytes = vec![0u8; (usize::from(top) + 1) * 2];
            for (cid, (gid, _, _)) in &used.table {
                let at = usize::from(*cid) * 2;
                bytes[at..at + 2].copy_from_slice(&gid.to_be_bytes());
            }
            pdf.stream(gidmap, &deflate(&bytes))
                .filter(Filter::FlateDecode);
        }
    }

    // Structure tree: the root, the parent tree mapping each page's MCIDs to
    // the elements that own them, and the elements.
    {
        let mut root = pdf
            .indirect(struct_root)
            .start::<pdf_writer::writers::StructTreeRoot>();
        root.child(elem_refs[0]);
        root.parent_tree_next_key(pages as i32);
        let mut tree = root.parent_tree();
        let mut nums = tree.nums();
        for (i, parent) in parent_refs.iter().enumerate() {
            nums.insert(i as i32, *parent);
        }
    }
    for (i, mcids) in owners.iter().enumerate() {
        pdf.indirect(parent_refs[i])
            .array()
            .items(mcids.iter().map(|e| elem_refs[*e]));
    }
    for (i, elem) in w.elems.iter().enumerate() {
        let mut e = pdf.struct_element(elem_refs[i]);
        e.kind(elem.role);
        e.parent(elem.parent.map_or(struct_root, |p| elem_refs[p]));
        let mut children = e.children();
        for child in &elem.children {
            children.struct_element(elem_refs[*child]);
        }
        for (page, mcid) in &elem.content {
            children
                .marked_content_ref()
                .marked_content_id(*mcid)
                .page(page_refs[*page]);
        }
    }

    // Outline: one bookmark per card.
    if !w.marks.is_empty() {
        pdf.outline(outline)
            .first(mark_refs[0])
            .last(*mark_refs.last().unwrap())
            .count(w.marks.len() as i32);
        for (i, mark) in w.marks.iter().enumerate() {
            let mut item = pdf.outline_item(mark_refs[i]);
            item.title(TextStr(&mark.title));
            item.parent(outline);
            if i > 0 {
                item.prev(mark_refs[i - 1]);
            }
            if i + 1 < w.marks.len() {
                item.next(mark_refs[i + 1]);
            }
            item.flags(OutlineItemFlags::empty());
            item.dest()
                .page(page_refs[mark.page])
                .xyz(0.0, w.h - mark.top + 6.0, None);
        }
    }
    pdf.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_plex_face_parses_and_measures() {
        let fonts = Fonts::new(&[]);
        for (face, _, _) in PLEX {
            let width = fonts.width(face, 10.0, 0.0, "Résumé café ŁŒ → ×");
            assert!(width > 50.0, "{face:?} measures {width}");
            // Every character here is Plex's own, so none needs a fallback.
            for c in "Résumé café ŁŒ → ×·…—".chars() {
                assert!(matches!(fonts.glyph(face, c).0, FontKey::Plex(_)), "{c}");
                assert!(c == ' ' || fonts.glyph(face, c).1 != 0, "{c} has no glyph");
            }
        }
    }

    #[test]
    fn an_over_long_id_path_or_query_wraps_whole_inside_the_page() {
        let column = |key: &'static str, label: &str, kind: Kind| super::super::Column {
            key,
            label: label.into(),
            kind,
        };
        let table = |id: &'static str, columns, rows| Table {
            id,
            title: id.into(),
            meta: String::new(),
            columns,
            rows,
            total: None,
            more: 0,
            note: None,
            empty: "Nothing.".into(),
        };
        let id = format!("stdio-{}", "3f2a9c41d07be5".repeat(7));
        let mut path = String::from("/Users/someone/work");
        while path.len() < 170 {
            path.push_str("/generated-module");
        }
        path = format!("{path}/{}", "x".repeat(200 - path.len() - 1));
        let query: String = r"(?i)^(fn|struct|impl)\s+[A-Za-z_][A-Za-z0-9_]*(<[^>]*>)?\(|\.rs$|"
            .repeat(6)
            .chars()
            .take(300)
            .collect();
        assert_eq!((path.len(), query.len()), (200, 300));
        // The access report's own tables, the query once as prose and once as
        // the digest column a redacted report draws it in; and a table of ids
        // too many to fit at any size, so the columns have to give way.
        let mut report = Report::new("access", "Access");
        let mut blocks = vec![Block::Table(table(
            "sessions",
            vec![
                column("last_seen", "Last seen", Kind::Id),
                column("session", "Session", Kind::Id),
                column("agent", "Agent", Kind::Strong),
                column("version", "Version", Kind::Id),
                column("stores", "Stores", Kind::Text),
                column("queries", "Queries", Kind::Num),
                column("zero_hit", "Zero-hit", Kind::Num),
                column("net_tokens", "Net tokens", Kind::Num),
            ],
            vec![vec![
                Cell::text("2026-10-17 09:41:07"),
                Cell::text(&id),
                Cell::text("claude-code"),
                Cell::text("2.1.0"),
                Cell::text(&path),
                Cell::int(12),
                Cell::int(1),
                Cell::int(1_234_567),
            ]],
        ))];
        for kind in [Kind::Text, Kind::Id] {
            blocks.push(Block::Table(table(
                "retrievals",
                vec![
                    column("at", "At", Kind::Id),
                    column("agent", "Agent", Kind::Strong),
                    column("store", "Store", Kind::Text),
                    column("tool", "Tool", Kind::Id),
                    column("query", "Query", kind),
                    column("hits", "Hits", Kind::Num),
                    column("excerpt_tokens", "Excerpt tokens", Kind::Num),
                    column("whole_file_tokens", "Whole-file tokens", Kind::Num),
                ],
                vec![vec![
                    Cell::text("2026-10-17 09:41:07"),
                    Cell::text("claude-code"),
                    Cell::text(&path),
                    Cell::text(&id),
                    Cell::text(&query),
                    Cell::int(8),
                    Cell::int(2_400),
                    Cell::int(48_000),
                ]],
            )));
        }
        let keys = ["c0", "c1", "c2", "c3", "c4", "c5", "c6", "c7", "c8", "c9"];
        blocks.push(Block::Table(table(
            "crowded",
            keys.iter().map(|k| column(k, k, Kind::Id)).collect(),
            vec![keys.iter().map(|_| Cell::text(&path)).collect()],
        )));
        report.blocks = blocks;

        let text = pdf_extract::extract_text_from_mem(&report.pdf()).expect("the PDF parses");
        let squeezed: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        for data in [&id, &path, &query] {
            assert!(
                squeezed.contains(data.as_str()),
                "{data} is not whole in:\n{text}"
            );
        }
        assert!(!text.contains('…'), "an ellipsis in:\n{text}");

        // Every column, and every line in it, ends inside the right margin.
        let w = Writer::new(Fonts::new(&[]));
        for table in report.tables() {
            let Fit { cols, size, pad } = fit_table(&w, table);
            let total: f32 = cols.iter().sum();
            assert!(
                total <= w.inner() + 0.01,
                "{}: {total} > {}",
                table.id,
                w.inner()
            );
            for (i, (column, width)) in table.columns.iter().zip(&cols).enumerate() {
                let room = width - 2.0 * pad;
                for line in header_lines(&w, &column.label, room) {
                    let lw = w.fonts.width(Face::MonoMedium, HEAD, HEAD * 0.08, &line);
                    assert!(lw <= room + 0.01, "{}: header {line}", table.id);
                }
                for row in &table.rows {
                    let fs = cell_size(column.kind, size);
                    for line in cell_lines(&w, column.kind, &row[i].display, size, room) {
                        let lw = w.width(face_of(column.kind), fs, &line);
                        assert!(lw <= room + 0.01, "{}: {line} is {lw} in {room}", table.id);
                    }
                }
            }
        }
    }
}
