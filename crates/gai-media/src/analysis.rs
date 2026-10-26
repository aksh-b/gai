//! Who spoke, in what language, and how it felt.

use std::collections::BTreeMap;

use gai_llm::{ModelRegistry, Segment, Transcript, clock};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Meter, Result, ask_json, pieces, text_field};

/// How something sounds: `positive`, `neutral`, `negative` or `mixed`,
/// with a score from -1 (very negative) to 1 (very positive).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Sentiment {
    pub label: String,
    pub score: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Sentiment {
    fn from_json(v: &Value) -> Option<Sentiment> {
        let score = v["score"]
            .as_f64()
            .or_else(|| v["score"].as_str()?.parse().ok())?
            .clamp(-1.0, 1.0);
        let label = text_field(v, "label")
            .map(|l| l.to_lowercase())
            .filter(|l| ["positive", "neutral", "negative", "mixed"].contains(&l.as_str()))
            .unwrap_or_else(|| label_for(score).into());
        Some(Sentiment {
            label,
            score,
            reason: text_field(v, "reason"),
        })
    }
}

fn label_for(score: f64) -> &'static str {
    if score > 0.25 {
        "positive"
    } else if score < -0.25 {
        "negative"
    } else {
        "neutral"
    }
}

/// One speaker's share of a recording.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SpeakerStats {
    pub name: String,
    /// Seconds spoken.
    pub seconds: f64,
    /// Of all the speaking, 0 to 1.
    pub share: f64,
    pub turns: u32,
    pub words: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sentiment: Option<Sentiment>,
    /// What they mostly said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// A moment worth finding again.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Moment {
    /// Seconds from the start.
    pub at: f64,
    pub what: String,
}

/// What an audio recording is like.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AudioInsights {
    /// ISO-639-1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub speakers: Vec<SpeakerStats>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sentiment: Option<Sentiment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub moments: Vec<Moment>,
}

/// `h:mm:ss`, `m:ss` or plain seconds.
pub fn parse_clock(v: &Value) -> Option<f64> {
    if let Some(n) = v.as_f64() {
        return Some(n.max(0.0));
    }
    let s = v
        .as_str()?
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']');
    let mut secs = 0.0;
    for part in s.split(':') {
        secs = secs * 60.0 + part.trim().parse::<f64>().ok()?;
    }
    Some(secs)
}

/// Segments for a transcript that has none: its sentences, timed by their
/// share of the text.
pub fn ensure_segments(t: &mut Transcript) {
    if !t.segments.is_empty() || t.text.trim().is_empty() {
        return;
    }
    let mut sentences = Vec::new();
    let mut cur = String::new();
    for c in t.text.chars() {
        cur.push(c);
        if matches!(c, '.' | '?' | '!' | '\n') && cur.trim().len() > 1 {
            sentences.push(std::mem::take(&mut cur));
        }
    }
    if !cur.trim().is_empty() {
        sentences.push(cur);
    }
    let total = t.text.len().max(1) as f64;
    let length = t.duration_secs.unwrap_or(total * 0.06);
    let mut at = 0.0;
    t.segments = sentences
        .into_iter()
        .map(|s| {
            let start = at;
            at += s.len() as f64 / total * length;
            Segment {
                start,
                end: at,
                text: s.trim().to_owned(),
                speaker: None,
            }
        })
        .filter(|s| !s.text.is_empty())
        .collect();
}

/// Talk time, turns and words per speaker, most talkative first.
pub fn speaker_stats(t: &Transcript) -> Vec<SpeakerStats> {
    let mut by: BTreeMap<&str, SpeakerStats> = BTreeMap::new();
    let mut last: Option<&str> = None;
    for s in &t.segments {
        let Some(name) = s.speaker.as_deref() else {
            last = None;
            continue;
        };
        let e = by.entry(name).or_insert_with(|| SpeakerStats {
            name: name.to_owned(),
            ..Default::default()
        });
        e.seconds += (s.end - s.start).max(0.0);
        e.words += s.text.split_whitespace().count() as u32;
        if last != Some(name) {
            e.turns += 1;
        }
        last = Some(name);
    }
    let total: f64 = by.values().map(|s| s.seconds).sum();
    let mut out: Vec<SpeakerStats> = by
        .into_values()
        .map(|mut s| {
            s.share = if total > 0.0 { s.seconds / total } else { 0.0 };
            s
        })
        .collect();
    out.sort_by(|a, b| b.seconds.total_cmp(&a.seconds));
    out
}

const SPEAKERS_SYSTEM: &str = "You work out who is speaking in transcripts that have no speaker labels. Use people's names when the conversation shows them (introductions, \"thanks, Ana\", someone being asked a question by name); otherwise call them Speaker 1, Speaker 2 and so on, in the order they first speak. A new speaker usually starts at a question, an answer or a change of view. Answer only with JSON.";

/// Lines of a labelling request: at most this many at a time.
const LINES_PER_CALL: usize = 120;

/// Fills in the speaker of segments that have none, with `model` reading
/// the conversation. Labels the transcription model gave are kept.
pub async fn label_speakers(
    models: &ModelRegistry,
    model: &str,
    t: &mut Transcript,
    meter: &Meter,
) -> Result<()> {
    ensure_segments(t);
    if t.segments.is_empty() || t.segments.iter().all(|s| s.speaker.is_some()) {
        return Ok(());
    }
    let mut known: Vec<String> = Vec::new();
    let n = t.segments.len();
    let mut start = 0;
    while start < n {
        let end = (start + LINES_PER_CALL).min(n);
        let lines: Vec<String> = (start..end)
            .map(|i| {
                let s = &t.segments[i];
                format!("{}. [{}] {}", i + 1, clock(s.start), s.text)
            })
            .collect();
        let names = if known.is_empty() {
            String::new()
        } else {
            format!(
                "\nSpeakers found so far (keep using these names): {}.\n",
                known.join(", ")
            )
        };
        let v = ask_json(
            models,
            model,
            SPEAKERS_SYSTEM,
            format!(
                "Say who speaks each numbered line.{names}\nAnswer as {{\"speakers\": [\"names, in order of first speaking\"], \"lines\": {{\"<line number>\": \"<speaker>\"}}}}.\n\nTranscript:\n{}",
                lines.join("\n")
            ),
            meter,
        )
        .await?;
        let labels = &v["lines"];
        for i in start..end {
            if t.segments[i].speaker.is_some() {
                continue;
            }
            let key = (i + 1).to_string();
            let label = labels
                .get(&key)
                .or_else(|| labels.get(i - start))
                .and_then(|l| l.as_str().or_else(|| l["speaker"].as_str()))
                .map(str::trim)
                .filter(|l| !l.is_empty());
            if let Some(l) = label {
                if !known.iter().any(|k| k == l) {
                    known.push(l.to_owned());
                }
                t.segments[i].speaker = Some(l.to_owned());
            }
        }
        start = end;
    }
    Ok(())
}

/// What the transcript alone tells: language, length and talk times.
pub fn basic_insights(t: &Transcript) -> AudioInsights {
    AudioInsights {
        language: t.language.clone().map(|l| short_language(&l)),
        duration_secs: t.duration_secs,
        speakers: speaker_stats(t),
        ..Default::default()
    }
}

const INSIGHTS_SYSTEM: &str = "You analyse conversations from their transcripts. Judge sentiment from what is said and how (agreement, frustration, enthusiasm, hesitation), not from the topic. Be specific and brief. Answer only with JSON.";

/// Characters of transcript read per insights call.
const INSIGHT_CHARS: usize = 40_000;

/// The language, the overall and per-speaker sentiment, and the moments
/// worth finding again, with each speaker's talk time.
pub async fn insights(
    models: &ModelRegistry,
    model: &str,
    t: &Transcript,
    meter: &Meter,
) -> Result<AudioInsights> {
    let mut out = basic_insights(t);
    let text = t.timed_text();
    if text.trim().is_empty() {
        return Ok(out);
    }
    let parts = pieces(&text, INSIGHT_CHARS);
    let n = parts.len();
    let mut scores: Vec<(Sentiment, f64)> = Vec::new();
    let mut reasons: Vec<String> = Vec::new();
    // Each part's reading of each speaker: score, label, summary.
    type Reading = (f64, Option<String>, Option<String>);
    let mut per_speaker: BTreeMap<String, Vec<Reading>> = BTreeMap::new();
    for (i, part) in parts.iter().enumerate() {
        let which = if n > 1 {
            format!(" This is part {} of {n}.", i + 1)
        } else {
            String::new()
        };
        let v = ask_json(
            models,
            model,
            INSIGHTS_SYSTEM,
            format!(
                "Analyse this conversation.{which} Answer as {{\"language\": \"ISO-639-1 code of the main language\", \"sentiment\": {{\"label\": \"positive|neutral|negative|mixed\", \"score\": -1 to 1, \"reason\": \"one sentence\"}}, \"speakers\": {{\"<speaker>\": {{\"label\": \"...\", \"score\": -1 to 1, \"summary\": \"what they mostly said, one sentence\"}}}}, \"moments\": [{{\"at\": \"m:ss\", \"what\": \"a decision, disagreement, question or turning point\"}}]}}. At most 6 moments.\n\nTranscript:\n{part}"
            ),
            meter,
        )
        .await?;
        if out.language.is_none() {
            out.language = text_field(&v, "language").map(|l| short_language(&l));
        }
        if let Some(mut s) = Sentiment::from_json(&v["sentiment"]) {
            reasons.extend(s.reason.take());
            scores.push((s, part.len() as f64));
        }
        if let Some(map) = v["speakers"].as_object() {
            for (name, sv) in map {
                if let Some(s) = Sentiment::from_json(sv) {
                    per_speaker.entry(name.clone()).or_default().push((
                        s.score,
                        Some(s.label),
                        text_field(sv, "summary"),
                    ));
                }
            }
        }
        for m in v["moments"].as_array().into_iter().flatten() {
            if let (Some(at), Some(what)) = (parse_clock(&m["at"]), text_field(m, "what")) {
                out.moments.push(Moment { at, what });
            }
        }
    }
    out.moments.sort_by(|a, b| a.at.total_cmp(&b.at));
    out.moments.truncate(12);
    out.sentiment = merge(&scores, reasons);
    for s in &mut out.speakers {
        if let Some(found) = per_speaker.get(&s.name) {
            let score = found.iter().map(|f| f.0).sum::<f64>() / found.len() as f64;
            s.sentiment = Some(Sentiment {
                label: if found.len() == 1 {
                    found[0]
                        .1
                        .clone()
                        .unwrap_or_else(|| label_for(score).into())
                } else {
                    label_for(score).into()
                },
                score,
                reason: None,
            });
            s.summary = found.iter().find_map(|f| f.2.clone());
        }
    }
    Ok(out)
}

/// One sentiment from each part's, weighted by length: `mixed` when the
/// parts are far apart. A single part keeps the model's label.
fn merge(parts: &[(Sentiment, f64)], reasons: Vec<String>) -> Option<Sentiment> {
    let reason = (!reasons.is_empty()).then(|| reasons.join(" "));
    match parts {
        [] => None,
        [(one, _)] => Some(Sentiment {
            reason,
            ..one.clone()
        }),
        _ => {
            let weight: f64 = parts.iter().map(|(_, w)| w).sum::<f64>().max(1.0);
            let score = parts.iter().map(|(s, w)| s.score * w).sum::<f64>() / weight;
            let spread = parts
                .iter()
                .map(|(s, _)| (s.score - score).abs())
                .fold(0.0, f64::max);
            Some(Sentiment {
                label: if spread > 0.6 {
                    "mixed".into()
                } else {
                    label_for(score).into()
                },
                score,
                reason,
            })
        }
    }
}

/// `en` from `english`, `en-US` or `en`.
fn short_language(l: &str) -> String {
    let l = l.trim().to_lowercase();
    let names = [
        ("english", "en"),
        ("german", "de"),
        ("french", "fr"),
        ("spanish", "es"),
        ("italian", "it"),
        ("portuguese", "pt"),
        ("dutch", "nl"),
        ("hindi", "hi"),
        ("japanese", "ja"),
        ("chinese", "zh"),
        ("korean", "ko"),
        ("russian", "ru"),
        ("arabic", "ar"),
    ];
    if let Some((_, code)) = names.iter().find(|(n, _)| *n == l) {
        return (*code).into();
    }
    l.split(['-', '_']).next().unwrap_or(&l).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start: f64, end: f64, who: &str, text: &str) -> Segment {
        Segment {
            start,
            end,
            text: text.into(),
            speaker: Some(who.into()),
        }
    }

    #[test]
    fn stats_per_speaker() {
        let t = Transcript {
            segments: vec![
                seg(0.0, 6.0, "Ana", "Let us start with the budget."),
                seg(6.0, 8.0, "Ben", "Sure."),
                seg(8.0, 12.0, "Ana", "It went up."),
                seg(12.0, 14.0, "Ana", "By ten percent."),
            ],
            ..Default::default()
        };
        let s = speaker_stats(&t);
        assert_eq!(s[0].name, "Ana");
        assert_eq!((s[0].turns, s[0].words), (2, 12));
        assert!((s[0].share - 12.0 / 14.0).abs() < 1e-9);
        assert_eq!(s[1].turns, 1);
    }

    #[test]
    fn clocks_and_languages() {
        assert_eq!(parse_clock(&"1:02:05".into()), Some(3725.0));
        assert_eq!(parse_clock(&"[12:30]".into()), Some(750.0));
        assert_eq!(parse_clock(&serde_json::json!(42.5)), Some(42.5));
        assert_eq!(short_language("English"), "en");
        assert_eq!(short_language("pt-BR"), "pt");
        let s = |score: f64| Sentiment {
            label: label_for(score).into(),
            score,
            reason: None,
        };
        assert_eq!(
            merge(&[(s(0.9), 1.0), (s(-0.8), 1.0)], vec![])
                .unwrap()
                .label,
            "mixed"
        );
        assert_eq!(
            merge(&[(s(0.5), 3.0), (s(0.3), 1.0)], vec![])
                .unwrap()
                .label,
            "positive"
        );
    }

    #[test]
    fn sentences_become_segments() {
        let mut t = Transcript {
            text: "Hello there. How are you? Fine!".into(),
            duration_secs: Some(31.0),
            ..Default::default()
        };
        ensure_segments(&mut t);
        assert_eq!(t.segments.len(), 3);
        assert_eq!(t.segments[1].text, "How are you?");
        assert!((t.segments[2].end - 31.0).abs() < 1e-6);
    }
}
