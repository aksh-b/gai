//! Markdown documents as Word (DOCX) and PDF files. The Markdown read is
//! what models write: headings, paragraphs, bullet and numbered lists,
//! quotes, code blocks, tables, rules, and bold, italic and code spans.
//! PDFs use the standard Helvetica and Courier fonts (Latin-1 text).

use std::io::Write;

/// A run of text with one style.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Block {
    Heading(u8, Vec<Span>),
    Para(Vec<Span>),
    /// A list item: its depth (0 = top), its number for numbered lists.
    Item(usize, Option<u32>, Vec<Span>),
    Quote(Vec<Span>),
    Code(String),
    Table(Vec<Vec<Vec<Span>>>),
    Rule,
}

/// Bold (`**`/`__`), italic (`*`/`_`), code (`` ` ``) and links
/// (`[text](url)`, kept as "text (url)").
pub(crate) fn spans(s: &str) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    let (mut bold, mut italic) = (false, false);
    let mut cur = String::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    let flush = |cur: &mut String, out: &mut Vec<Span>, bold: bool, italic: bool, code: bool| {
        if !cur.is_empty() {
            out.push(Span {
                text: std::mem::take(cur),
                bold,
                italic,
                code,
            });
        }
    };
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '\\' && next.is_some_and(|n| "*_`[]()#\\|".contains(n)) {
            cur.push(next.unwrap_or_default());
            i += 2;
        } else if c == '`' {
            if let Some(end) = chars[i + 1..].iter().position(|&x| x == '`') {
                flush(&mut cur, &mut out, bold, italic, false);
                cur = chars[i + 1..i + 1 + end].iter().collect();
                flush(&mut cur, &mut out, bold, italic, true);
                i += end + 2;
            } else {
                cur.push(c);
                i += 1;
            }
        } else if (c == '*' || c == '_') && next == Some(c) {
            flush(&mut cur, &mut out, bold, italic, false);
            bold = !bold;
            i += 2;
        } else if (c == '*' || c == '_')
            && (italic || next.is_some_and(|n| !n.is_whitespace()))
            && !(c == '_' && i > 0 && chars[i - 1].is_alphanumeric())
        {
            flush(&mut cur, &mut out, bold, italic, false);
            italic = !italic;
            i += 1;
        } else if c == '['
            && let Some(close) = chars[i..].iter().position(|&x| x == ']')
            && chars.get(i + close + 1) == Some(&'(')
            && let Some(paren) = chars[i + close + 1..].iter().position(|&x| x == ')')
        {
            let text: String = chars[i + 1..i + close].iter().collect();
            let url: String = chars[i + close + 2..i + close + 1 + paren].iter().collect();
            cur.push_str(&text);
            if !url.is_empty() && url != text {
                cur.push_str(&format!(" ({url})"));
            }
            i += close + 2 + paren;
        } else {
            cur.push(c);
            i += 1;
        }
    }
    flush(&mut cur, &mut out, bold, italic, false);
    out
}

fn table_row(line: &str) -> Vec<String> {
    let t = line.trim().trim_start_matches('|').trim_end_matches('|');
    t.split('|').map(|c| c.trim().to_owned()).collect()
}

fn is_separator(line: &str) -> bool {
    let t = line.trim();
    t.contains('-') && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

pub(crate) fn parse(md: &str) -> Vec<Block> {
    let lines: Vec<&str> = md.lines().collect();
    let mut out = Vec::new();
    let mut para: Vec<&str> = Vec::new();
    let end_para = |para: &mut Vec<&str>, out: &mut Vec<Block>| {
        if !para.is_empty() {
            out.push(Block::Para(spans(&para.join(" "))));
            para.clear();
        }
    };
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim();
        let indent = line.len() - line.trim_start().len();
        if t.starts_with("```") || t.starts_with("~~~") {
            end_para(&mut para, &mut out);
            let fence = &t[..3];
            let mut code = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].trim().starts_with(fence) {
                code.push(lines[i]);
                i += 1;
            }
            out.push(Block::Code(code.join("\n")));
            i += 1;
            continue;
        }
        if t.is_empty() {
            end_para(&mut para, &mut out);
        } else if let Some(level) =
            (1..=6).find(|&n| t.starts_with(&"#".repeat(n)) && t[n..].starts_with(' '))
        {
            end_para(&mut para, &mut out);
            out.push(Block::Heading(
                level as u8,
                spans(t[level..].trim().trim_end_matches('#').trim()),
            ));
        } else if t.chars().all(|c| c == '-' || c == ' ') && t.matches('-').count() >= 3
            || t.chars().all(|c| c == '*' || c == ' ') && t.matches('*').count() >= 3
        {
            end_para(&mut para, &mut out);
            out.push(Block::Rule);
        } else if let Some(rest) = t
            .strip_prefix("- ")
            .or(t.strip_prefix("* "))
            .or(t.strip_prefix("+ "))
        {
            end_para(&mut para, &mut out);
            let rest = rest
                .strip_prefix("[ ] ")
                .map(|r| format!("☐ {r}"))
                .or_else(|| rest.strip_prefix("[x] ").map(|r| format!("☑ {r}")))
                .unwrap_or_else(|| rest.to_owned());
            out.push(Block::Item(indent / 2, None, spans(&rest)));
        } else if let Some(dot) = t
            .find(". ")
            .filter(|&d| d > 0 && d <= 3 && t[..d].chars().all(|c| c.is_ascii_digit()))
        {
            end_para(&mut para, &mut out);
            out.push(Block::Item(
                indent / 2,
                t[..dot].parse().ok(),
                spans(&t[dot + 2..]),
            ));
        } else if let Some(q) = t.strip_prefix('>') {
            end_para(&mut para, &mut out);
            out.push(Block::Quote(spans(q.trim())));
        } else if t.starts_with('|') && lines.get(i + 1).is_some_and(|n| is_separator(n)) {
            end_para(&mut para, &mut out);
            let mut rows = vec![table_row(t).iter().map(|c| spans(c)).collect::<Vec<_>>()];
            i += 2;
            while i < lines.len() && lines[i].trim().starts_with('|') {
                rows.push(table_row(lines[i]).iter().map(|c| spans(c)).collect());
                i += 1;
            }
            out.push(Block::Table(rows));
            continue;
        } else {
            para.push(t);
        }
        i += 1;
    }
    end_para(&mut para, &mut out);
    out
}

fn plain(spans: &[Span]) -> String {
    spans.iter().map(|s| s.text.as_str()).collect()
}

/// The document's title: its first heading, else `fallback`.
pub(crate) fn title_of(blocks: &[Block], fallback: &str) -> String {
    blocks
        .iter()
        .find_map(|b| match b {
            Block::Heading(_, s) => Some(plain(s)),
            _ => None,
        })
        .unwrap_or_else(|| fallback.to_owned())
}

// ---- DOCX ----

fn xml(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() || *c == '\t')
        .map(|c| match c {
            '&' => "&amp;".to_owned(),
            '<' => "&lt;".to_owned(),
            '>' => "&gt;".to_owned(),
            '"' => "&quot;".to_owned(),
            c => c.to_string(),
        })
        .collect()
}

fn runs(spans: &[Span]) -> String {
    spans
        .iter()
        .map(|s| {
            let mut props = String::new();
            if s.bold {
                props.push_str("<w:b/>");
            }
            if s.italic {
                props.push_str("<w:i/>");
            }
            if s.code {
                props.push_str("<w:rFonts w:ascii=\"Consolas\" w:hAnsi=\"Consolas\" w:cs=\"Consolas\"/><w:shd w:val=\"clear\" w:fill=\"F2F2F2\"/>");
            }
            let rpr = if props.is_empty() { String::new() } else { format!("<w:rPr>{props}</w:rPr>") };
            format!("<w:r>{rpr}<w:t xml:space=\"preserve\">{}</w:t></w:r>", xml(&s.text))
        })
        .collect()
}

fn para(style: Option<&str>, extra: &str, body: &str) -> String {
    let style = style
        .map(|s| format!("<w:pStyle w:val=\"{s}\"/>"))
        .unwrap_or_default();
    format!("<w:p><w:pPr>{style}{extra}</w:pPr>{body}</w:p>")
}

const STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri" w:hAnsi="Calibri" w:cs="Calibri"/><w:sz w:val="22"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="120" w:line="276" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults>
<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>
<w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:keepNext/><w:spacing w:before="360" w:after="120"/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/><w:sz w:val="36"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:keepNext/><w:spacing w:before="240" w:after="80"/><w:outlineLvl w:val="1"/></w:pPr><w:rPr><w:b/><w:sz w:val="30"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Heading3"><w:name w:val="heading 3"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:keepNext/><w:spacing w:before="200" w:after="60"/><w:outlineLvl w:val="2"/></w:pPr><w:rPr><w:b/><w:sz w:val="26"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Quote"><w:name w:val="Quote"/><w:basedOn w:val="Normal"/><w:pPr><w:ind w:left="567"/></w:pPr><w:rPr><w:i/><w:color w:val="595959"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Code"><w:name w:val="Code"/><w:basedOn w:val="Normal"/><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/><w:shd w:val="clear" w:fill="F2F2F2"/></w:pPr><w:rPr><w:rFonts w:ascii="Consolas" w:hAnsi="Consolas" w:cs="Consolas"/><w:sz w:val="19"/></w:rPr></w:style>
</w:styles>"#;

/// A Word document from Markdown.
pub(crate) fn docx(md: &str, title: &str) -> Result<Vec<u8>, String> {
    let blocks = parse(md);
    let mut body = String::new();
    for b in &blocks {
        body.push_str(&match b {
            Block::Heading(l, s) => para(Some(&format!("Heading{}", (*l).min(3))), "", &runs(s)),
            Block::Para(s) => para(None, "", &runs(s)),
            Block::Item(depth, n, s) => {
                let marker = match n {
                    Some(n) => format!("{n}."),
                    None => "•".into(),
                };
                let left = 360 + 360 * depth;
                para(
                    None,
                    &format!("<w:ind w:left=\"{left}\" w:hanging=\"360\"/><w:spacing w:after=\"40\"/>"),
                    &format!("<w:r><w:t xml:space=\"preserve\">{marker}\t</w:t></w:r>{}", runs(s)),
                )
            }
            Block::Quote(s) => para(Some("Quote"), "", &runs(s)),
            Block::Code(c) => c
                .lines()
                .map(|l| para(Some("Code"), "", &format!("<w:r><w:t xml:space=\"preserve\">{}</w:t></w:r>", xml(l))))
                .collect(),
            Block::Rule => para(
                None,
                "<w:pBdr><w:bottom w:val=\"single\" w:sz=\"6\" w:space=\"1\" w:color=\"BFBFBF\"/></w:pBdr>",
                "",
            ),
            Block::Table(rows) => {
                let border = "<w:tblBorders><w:top w:val=\"single\" w:sz=\"4\" w:color=\"BFBFBF\"/><w:left w:val=\"single\" w:sz=\"4\" w:color=\"BFBFBF\"/><w:bottom w:val=\"single\" w:sz=\"4\" w:color=\"BFBFBF\"/><w:right w:val=\"single\" w:sz=\"4\" w:color=\"BFBFBF\"/><w:insideH w:val=\"single\" w:sz=\"4\" w:color=\"BFBFBF\"/><w:insideV w:val=\"single\" w:sz=\"4\" w:color=\"BFBFBF\"/></w:tblBorders>";
                let mut t = format!("<w:tbl><w:tblPr><w:tblW w:w=\"5000\" w:type=\"pct\"/>{border}</w:tblPr>");
                for (r, row) in rows.iter().enumerate() {
                    t.push_str("<w:tr>");
                    for cell in row {
                        let mut s = cell.clone();
                        if r == 0 {
                            for x in &mut s {
                                x.bold = true;
                            }
                        }
                        let shade = if r == 0 { "<w:tcPr><w:shd w:val=\"clear\" w:fill=\"F2F2F2\"/></w:tcPr>" } else { "" };
                        t.push_str(&format!("<w:tc>{shade}{}</w:tc>", para(None, "<w:spacing w:after=\"0\"/>", &runs(&s))));
                    }
                    t.push_str("</w:tr>");
                }
                t.push_str("</w:tbl>");
                t.push_str(&para(None, "", ""));
                t
            }
        });
    }
    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body>{body}<w:sectPr><w:pgSz w:w=\"11906\" w:h=\"16838\"/><w:pgMar w:top=\"1134\" w:right=\"1134\" w:bottom=\"1134\" w:left=\"1134\" w:header=\"709\" w:footer=\"709\" w:gutter=\"0\"/></w:sectPr></w:body></w:document>"
    );
    let core = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:title>{}</dc:title><dc:creator>gai</dc:creator></cp:coreProperties>",
        xml(title)
    );
    let files: [(&str, String); 6] = [
        ("[Content_Types].xml", "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/><Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/><Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/></Types>".into()),
        ("_rels/.rels", "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/><Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" Target=\"docProps/core.xml\"/></Relationships>".into()),
        ("word/_rels/document.xml.rels", "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/></Relationships>".into()),
        ("word/document.xml", document),
        ("word/styles.xml", STYLES.into()),
        ("docProps/core.xml", core),
    ];
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, body) in files {
        zip.start_file(name, opts).map_err(|e| e.to_string())?;
        zip.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
    }
    Ok(zip.finish().map_err(|e| e.to_string())?.into_inner())
}

// ---- PDF ----

/// Helvetica's widths (per 1000 units) for ASCII 32..=126.
const HELVETICA: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667,
    611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500,
    222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];

#[derive(Clone, Copy, PartialEq)]
enum Font {
    Regular,
    Bold,
    Italic,
    BoldItalic,
    Mono,
}

impl Font {
    fn of(s: &Span) -> Self {
        match (s.code, s.bold, s.italic) {
            (true, _, _) => Font::Mono,
            (_, true, true) => Font::BoldItalic,
            (_, true, false) => Font::Bold,
            (_, false, true) => Font::Italic,
            _ => Font::Regular,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Font::Regular => "F1",
            Font::Bold => "F2",
            Font::Italic => "F3",
            Font::BoldItalic => "F4",
            Font::Mono => "F5",
        }
    }
    fn width(self, c: char, size: f32) -> f32 {
        let w = match self {
            Font::Mono => 600.0,
            _ => {
                let base = match c as u32 {
                    32..=126 => HELVETICA[c as usize - 32] as f32,
                    _ => 556.0,
                };
                if matches!(self, Font::Bold | Font::BoldItalic) {
                    base * 1.06
                } else {
                    base
                }
            }
        };
        w * size / 1000.0
    }
}

/// A character as a WinAnsi byte (Latin-1, plus a few typographic marks).
fn ansi(c: char) -> u8 {
    match c {
        '\u{2018}' => 0x91,
        '\u{2019}' => 0x92,
        '\u{201c}' => 0x93,
        '\u{201d}' => 0x94,
        '\u{2022}' => 0x95,
        '\u{2013}' => 0x96,
        '\u{2014}' => 0x97,
        '\u{20ac}' => 0x80,
        '\u{2026}' => 0x85,
        c if (c as u32) < 256 => c as u8,
        '☐' | '☑' => b'-',
        _ => b'?',
    }
}

fn pdf_string(s: &str) -> Vec<u8> {
    let mut out = vec![b'('];
    for c in s.chars() {
        match ansi(c) {
            b'(' | b')' | b'\\' => {
                out.push(b'\\');
                out.push(ansi(c));
            }
            b => out.push(b),
        }
    }
    out.push(b')');
    out
}

const PAGE_W: f32 = 595.0;
const PAGE_H: f32 = 842.0;
const MARGIN: f32 = 56.0;

struct Pdf {
    pages: Vec<Vec<u8>>,
    cur: Vec<u8>,
    y: f32,
}

/// A word (or piece of one) and its font.
type Word = (String, Font);

impl Pdf {
    fn new() -> Self {
        Self {
            pages: Vec::new(),
            cur: Vec::new(),
            y: PAGE_H - MARGIN,
        }
    }

    fn need(&mut self, h: f32) {
        if self.y - h < MARGIN {
            self.pages.push(std::mem::take(&mut self.cur));
            self.y = PAGE_H - MARGIN;
        }
    }

    fn text(&mut self, x: f32, y: f32, font: Font, size: f32, s: &str) {
        self.cur
            .extend(format!("BT /{} {size} Tf {x:.1} {y:.1} Td ", font.name()).bytes());
        self.cur.extend(pdf_string(s));
        self.cur.extend(b" Tj ET\n");
    }

    /// Lay out spans in `width` from `x`, wrapping at spaces.
    fn lines(spans: &[Span], size: f32, width: f32) -> Vec<Vec<(String, Font, f32)>> {
        let mut words: Vec<(Word, bool)> = Vec::new();
        for s in spans {
            let f = Font::of(s);
            let mut first = true;
            for w in s.text.split(' ') {
                // `true`: a space comes before this word.
                words.push(((w.to_owned(), f), !first || s.text.starts_with(' ')));
                first = false;
            }
        }
        let mut lines: Vec<Vec<(String, Font, f32)>> = vec![Vec::new()];
        let mut used = 0.0;
        for ((w, f), space) in words {
            if w.is_empty() {
                continue;
            }
            let ww: f32 = w.chars().map(|c| f.width(c, size)).sum();
            let sw = if space && used > 0.0 {
                f.width(' ', size)
            } else {
                0.0
            };
            if used > 0.0 && used + sw + ww > width {
                lines.push(Vec::new());
                used = 0.0;
            }
            let line = lines.last_mut().expect("a line");
            let x = used + if used > 0.0 { sw } else { 0.0 };
            // Words in the same font share one string, spaces included.
            match line.last_mut() {
                Some((text, font, _)) if *font == f && sw > 0.0 => {
                    text.push(' ');
                    text.push_str(&w);
                }
                _ => line.push((w, f, x)),
            }
            used = x + ww;
        }
        lines
    }

    fn block(&mut self, spans: &[Span], size: f32, x: f32, width: f32, after: f32) {
        let lead = size * 1.35;
        for line in Self::lines(spans, size, width) {
            self.need(lead);
            self.y -= lead;
            for (w, f, dx) in line {
                let y = self.y;
                self.text(x + dx, y, f, size, &w);
            }
        }
        self.y -= after;
    }

    fn finish(mut self, title: &str) -> Vec<u8> {
        if !self.cur.is_empty() || self.pages.is_empty() {
            self.pages.push(std::mem::take(&mut self.cur));
        }
        let fonts = [
            "Helvetica",
            "Helvetica-Bold",
            "Helvetica-Oblique",
            "Helvetica-BoldOblique",
            "Courier",
        ];
        // 1 catalog, 2 pages, 3 info, 4..9 fonts, then page and content pairs.
        let first_page = 4 + fonts.len();
        let mut objects: Vec<Vec<u8>> = Vec::new();
        objects.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
        let kids: Vec<String> = (0..self.pages.len())
            .map(|i| format!("{} 0 R", first_page + i * 2))
            .collect();
        objects.push(
            format!(
                "<< /Type /Pages /Kids [{}] /Count {} >>",
                kids.join(" "),
                self.pages.len()
            )
            .into_bytes(),
        );
        let mut info = b"<< /Producer (gai) /Title ".to_vec();
        info.extend(pdf_string(title));
        info.extend(b" >>");
        objects.push(info);
        for f in fonts {
            objects.push(
                format!(
                    "<< /Type /Font /Subtype /Type1 /BaseFont /{f} /Encoding /WinAnsiEncoding >>"
                )
                .into_bytes(),
            );
        }
        let font_refs: String = (0..fonts.len())
            .map(|i| format!("/F{} {} 0 R ", i + 1, 4 + i))
            .collect();
        for (i, content) in self.pages.iter().enumerate() {
            objects.push(
                format!(
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PAGE_W} {PAGE_H}] /Contents {} 0 R /Resources << /Font << {font_refs}>> >> >>",
                    first_page + i * 2 + 1
                )
                .into_bytes(),
            );
            let mut stream = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
            stream.extend(content);
            stream.extend(b"\nendstream");
            objects.push(stream);
        }
        let mut out = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n", i + 1).bytes());
            out.extend(o);
            out.extend(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for off in offsets {
            out.extend(format!("{off:010} 00000 n \n").bytes());
        }
        out.extend(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R /Info 3 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .bytes(),
        );
        out
    }
}

/// An A4 PDF from Markdown.
pub(crate) fn pdf(md: &str, title: &str) -> Vec<u8> {
    let width = PAGE_W - 2.0 * MARGIN;
    let mut p = Pdf::new();
    for b in parse(md) {
        match b {
            Block::Heading(l, s) => {
                let size = match l {
                    1 => 20.0,
                    2 => 16.0,
                    3 => 13.5,
                    _ => 12.0,
                };
                let bold: Vec<Span> = s.into_iter().map(|x| Span { bold: true, ..x }).collect();
                p.y -= size * 0.6;
                p.need(size * 3.0);
                p.block(&bold, size, MARGIN, width, 4.0);
            }
            Block::Para(s) => p.block(&s, 10.5, MARGIN, width, 6.0),
            Block::Item(depth, n, s) => {
                let x = MARGIN + 14.0 + 16.0 * depth as f32;
                let marker = n
                    .map(|n| format!("{n}."))
                    .unwrap_or_else(|| "\u{2022}".into());
                p.need(10.5 * 1.35);
                let y = p.y - 10.5 * 1.35;
                p.text(x - 12.0, y, Font::Regular, 10.5, &marker);
                p.block(&s, 10.5, x + 2.0, width - (x - MARGIN) - 2.0, 2.0);
            }
            Block::Quote(s) => {
                let it: Vec<Span> = s.into_iter().map(|x| Span { italic: true, ..x }).collect();
                p.block(&it, 10.5, MARGIN + 18.0, width - 18.0, 6.0);
            }
            Block::Code(c) => {
                for line in c.lines() {
                    let span = Span {
                        text: line.to_owned(),
                        bold: false,
                        italic: false,
                        code: true,
                    };
                    p.block(&[span], 9.0, MARGIN + 8.0, width - 8.0, 0.0);
                }
                p.y -= 6.0;
            }
            Block::Rule => {
                p.need(12.0);
                p.y -= 6.0;
                let y = p.y;
                p.cur.extend(
                    format!(
                        "0.75 G {MARGIN} {y:.1} m {:.1} {y:.1} l S 0 G\n",
                        PAGE_W - MARGIN
                    )
                    .bytes(),
                );
                p.y -= 6.0;
            }
            Block::Table(rows) => {
                let cols = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
                let cw = width / cols as f32;
                let size = 9.5;
                let lead = size * 1.35;
                for (r, row) in rows.iter().enumerate() {
                    let laid: Vec<_> = (0..cols)
                        .map(|c| {
                            let mut s = row.get(c).cloned().unwrap_or_default();
                            if r == 0 {
                                for x in &mut s {
                                    x.bold = true;
                                }
                            }
                            Pdf::lines(&s, size, cw - 8.0)
                        })
                        .collect();
                    let n = laid.iter().map(Vec::len).max().unwrap_or(1).max(1);
                    let h = n as f32 * lead + 6.0;
                    p.need(h);
                    let top = p.y;
                    for (c, lines) in laid.into_iter().enumerate() {
                        let x0 = MARGIN + c as f32 * cw;
                        for (li, line) in lines.into_iter().enumerate() {
                            for (w, f, dx) in line {
                                p.text(
                                    x0 + 4.0 + dx,
                                    top - 3.0 - (li as f32 + 1.0) * lead + 2.0,
                                    f,
                                    size,
                                    &w,
                                );
                            }
                        }
                        p.cur.extend(
                            format!("0.75 G {x0:.1} {:.1} {cw:.1} {h:.1} re S 0 G\n", top - h)
                                .bytes(),
                        );
                    }
                    p.y -= h;
                }
                p.y -= 8.0;
            }
        }
    }
    p.finish(title)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_what_models_write() {
        let md = "# Report\n\nIntro with **bold**, *it* and `code`, see [docs](https://x.y).\n\n- one\n  - nested\n1. first\n\n> quoted\n\n```\nlet x = 1;\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n---\n";
        let b = parse(md);
        assert_eq!(b[0], Block::Heading(1, spans("Report")));
        let Block::Para(p) = &b[1] else {
            panic!("{b:?}")
        };
        assert!(p.iter().any(|s| s.bold && s.text == "bold"));
        assert!(p.iter().any(|s| s.italic && s.text == "it"));
        assert!(p.iter().any(|s| s.code && s.text == "code"));
        assert!(plain(p).contains("docs (https://x.y)"));
        assert!(matches!(&b[2], Block::Item(0, None, _)));
        assert!(matches!(&b[3], Block::Item(1, None, _)));
        assert!(matches!(&b[4], Block::Item(0, Some(1), _)));
        assert!(matches!(&b[5], Block::Quote(_)));
        assert_eq!(b[6], Block::Code("let x = 1;".into()));
        let Block::Table(rows) = &b[7] else {
            panic!("{b:?}")
        };
        assert_eq!(rows.len(), 2);
        assert_eq!(b[8], Block::Rule);
        assert_eq!(title_of(&b, "x"), "Report");
        assert_eq!(plain(&spans("snake_case_name")), "snake_case_name");
    }

    #[test]
    fn writes_docx_and_pdf() {
        let md = "# Contract summary\n\nThe **fee** is €4,200 (monthly).\n\n| Party | Role |\n|---|---|\n| Acme | Supplier |\n\n- Term: 24 months\n";
        let d = docx(md, "Contract summary").unwrap();
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(d)).unwrap();
        let mut doc = String::new();
        std::io::Read::read_to_string(&mut z.by_name("word/document.xml").unwrap(), &mut doc)
            .unwrap();
        assert!(
            doc.contains("Heading1") && doc.contains("<w:tbl>") && doc.contains("€4,200"),
            "{doc}"
        );
        let p = pdf(&md.repeat(40), "Contract summary");
        assert!(p.starts_with(b"%PDF-1.4"));
        let s = String::from_utf8_lossy(&p);
        assert!(
            s.contains("/Count ") && !s.contains("/Count 1 "),
            "a long document spans pages"
        );
        assert!(s.contains("(Contract summary) Tj"));
        // A Euro sign in WinAnsi, and parentheses escaped.
        assert!(p.windows(3).any(|w| w == b"\x804,"));
        assert!(s.contains("\\(monthly\\)"));
        // A PDF reader gets the text back.
        let read = gai_memory::load::load_bytes("x.pdf", None, &pdf(md, "x")).unwrap();
        assert!(read.text.contains("Contract summary"), "{}", read.text);
        assert!(read.text.contains("Supplier"), "{}", read.text);
    }
}
