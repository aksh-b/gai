//! Splitting documents into chunks small enough to embed and to quote back
//! to a model.
//!
//! Markdown headings start new sections, and each chunk remembers the
//! heading path it sits under. Inside a section, text is split at the
//! largest boundary that works (paragraph, line, sentence, clause, word),
//! then packed greedily up to the size limit, with a little overlap so a
//! fact cut in two still appears whole in one chunk.

/// Characters per token, the same rough rate the agent's context manager
/// uses. Good enough for sizing chunks.
pub const CHARS_PER_TOKEN: usize = 4;

#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub text: String,
    /// `Guide > Install > Linux` for a chunk under those headings.
    pub heading: Option<String>,
}

/// Split `text` into chunks of about `max_tokens`, repeating about
/// `overlap_tokens` of each chunk at the start of the next.
pub fn chunk_text(text: &str, max_tokens: usize, overlap_tokens: usize) -> Vec<Piece> {
    let max = (max_tokens * CHARS_PER_TOKEN).max(200);
    let overlap = (overlap_tokens * CHARS_PER_TOKEN).min(max / 2);
    let mut out = Vec::new();
    for (heading, body) in sections(text) {
        let mut cur = String::new();
        for atom in atoms(&body, max, 0) {
            if len(&cur) + len(&atom) > max && !cur.trim().is_empty() {
                let tail = tail(&cur, overlap);
                push(&mut out, &cur, &heading);
                cur = if len(&tail) + len(&atom) <= max {
                    tail
                } else {
                    String::new()
                };
            }
            cur.push_str(&atom);
        }
        push(&mut out, &cur, &heading);
    }
    out
}

fn len(s: &str) -> usize {
    s.chars().count()
}

fn push(out: &mut Vec<Piece>, text: &str, heading: &Option<String>) {
    let t = text.trim();
    if !t.is_empty() {
        out.push(Piece {
            text: t.to_owned(),
            heading: heading.clone(),
        });
    }
}

/// The last `n` characters of `s`, starting at a word boundary.
fn tail(s: &str, n: usize) -> String {
    if n == 0 {
        return String::new();
    }
    let total = len(s);
    if total <= n {
        return s.to_owned();
    }
    let start = s.char_indices().nth(total - n).map(|(i, _)| i).unwrap_or(0);
    let rest = &s[start..];
    match rest.find(char::is_whitespace) {
        Some(ws) => rest[ws..].trim_start().to_owned() + " ",
        None => String::new(),
    }
}

/// Markdown sections with their heading paths. Headings inside fenced code
/// blocks are not headings.
fn sections(text: &str) -> Vec<(Option<String>, String)> {
    let mut out: Vec<(Option<String>, String)> = Vec::new();
    let mut path: Vec<(usize, String)> = Vec::new();
    let mut body = String::new();
    let mut fence = false;
    let heading_of = |p: &[(usize, String)]| {
        (!p.is_empty()).then(|| {
            p.iter()
                .map(|(_, t)| t.as_str())
                .collect::<Vec<_>>()
                .join(" > ")
        })
    };
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fence = !fence;
        }
        let level = trimmed.chars().take_while(|c| *c == '#').count();
        let is_heading = !fence
            && (1..=6).contains(&level)
            && trimmed[level..].starts_with([' ', '\t'])
            && !trimmed[level..].trim().is_empty();
        if is_heading {
            if !body.trim().is_empty() {
                out.push((heading_of(&path), std::mem::take(&mut body)));
            }
            body.clear();
            while path.last().is_some_and(|(l, _)| *l >= level) {
                path.pop();
            }
            path.push((
                level,
                trimmed[level..]
                    .trim()
                    .trim_end_matches('#')
                    .trim()
                    .to_owned(),
            ));
        }
        body.push_str(line);
    }
    if !body.trim().is_empty() {
        out.push((heading_of(&path), body));
    }
    out
}

const SEPARATORS: [&str; 8] = ["\n\n", "\n", ". ", "? ", "! ", "; ", ", ", " "];

/// Pieces of `s` no longer than `max`, keeping their separators so that
/// joining them gives `s` back.
fn atoms(s: &str, max: usize, from: usize) -> Vec<String> {
    if len(s) <= max {
        return vec![s.to_owned()];
    }
    for (i, sep) in SEPARATORS.iter().enumerate().skip(from) {
        if !s.contains(sep) {
            continue;
        }
        let mut out = Vec::new();
        for part in s.split_inclusive(sep) {
            if len(part) <= max {
                out.push(part.to_owned());
            } else {
                out.extend(atoms(part, max, i + 1));
            }
        }
        return out;
    }
    // One enormous word: cut it.
    let chars: Vec<char> = s.chars().collect();
    chars.chunks(max).map(|c| c.iter().collect()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_one_chunk() {
        let c = chunk_text("Hello world.", 100, 10);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].text, "Hello world.");
        assert_eq!(c[0].heading, None);
        assert!(chunk_text("  \n\n ", 100, 10).is_empty());
    }

    #[test]
    fn headings_make_sections_with_paths() {
        let md = "# Guide\nIntro.\n\n## Install\nRun it.\n\n```sh\n# not a heading\n```\n### Linux\napt install\n## Use\nGo.\n";
        let c = chunk_text(md, 100, 0);
        let headings: Vec<_> = c.iter().map(|p| p.heading.clone().unwrap()).collect();
        assert_eq!(
            headings,
            [
                "Guide",
                "Guide > Install",
                "Guide > Install > Linux",
                "Guide > Use"
            ]
        );
        assert!(c[1].text.contains("# not a heading"));
        assert!(c[1].text.starts_with("## Install"));
    }

    #[test]
    fn long_text_splits_at_boundaries_within_the_limit_with_overlap() {
        let para = "Alpha beta gamma delta. ".repeat(40); // ~960 chars
        let text = format!("{para}\n\n{para}\n\n{para}");
        let c = chunk_text(&text, 100, 10); // 400 chars, 40 overlap
        assert!(c.len() >= 7, "{}", c.len());
        for p in &c {
            assert!(len(&p.text) <= 400 + 40, "{}", len(&p.text));
            assert!(
                p.text.starts_with(char::is_alphabetic),
                "{:?}",
                &p.text[..20]
            );
        }
        // Overlap: each chunk after the first starts with words that ended
        // the previous one.
        let first_words: String = c[1].text.split(' ').take(2).collect::<Vec<_>>().join(" ");
        assert!(c[0].text.contains(&first_words));
        // Nothing is lost.
        let words = text.split_whitespace().count();
        let seen: usize = c.iter().map(|p| p.text.split_whitespace().count()).sum();
        assert!(seen >= words);
    }

    #[test]
    fn giant_words_are_cut() {
        let c = chunk_text(&"x".repeat(1000), 50, 0); // 200 chars
        assert_eq!(c.len(), 5);
        assert!(c.iter().all(|p| len(&p.text) == 200));
    }

    #[test]
    fn multibyte_text_is_safe() {
        let text = "héllo wörld ünïcode ".repeat(100);
        let c = chunk_text(&text, 50, 10);
        assert!(c.len() > 1);
    }
}
