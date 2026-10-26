//! Meeting notes from a transcript: what was decided, who does what by
//! when, and what is still open.

use std::collections::BTreeMap;

use futures::StreamExt;
use gai_core::ContentBlock;
use gai_llm::{ModelRegistry, Transcript, clock};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::analysis::{AudioInsights, parse_clock};
use crate::video::Chapter;
use crate::{Meter, Result, ask, ask_json, invalid, pieces, strings, text_field};

/// What notes to write: the usual ones and, per template, more sections.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct NotesTemplate {
    pub name: String,
    /// Extra guidance for the writer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// Sections to write besides the summary, decisions and action items:
    /// key to what goes in it.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub sections: BTreeMap<String, String>,
}

impl NotesTemplate {
    /// The built-in templates' names.
    pub const BUILTIN: [&'static str; 7] = [
        "general",
        "standup",
        "one_on_one",
        "sales_call",
        "interview",
        "retro",
        "lecture",
    ];

    pub fn builtin(name: &str) -> Option<NotesTemplate> {
        let t = |instructions: &str, sections: &[(&str, &str)]| NotesTemplate {
            name: name.to_owned(),
            instructions: (!instructions.is_empty()).then(|| instructions.to_owned()),
            sections: sections
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        };
        Some(match name {
            "general" => t("", &[]),
            "standup" => t(
                "A daily standup: keep it short.",
                &[
                    (
                        "updates",
                        "for each person: what they did, what they'll do next, and anything blocking them",
                    ),
                    (
                        "blockers",
                        "everything blocking someone, and who can unblock it",
                    ),
                ],
            ),
            "one_on_one" => t(
                "A one-to-one between a manager and someone on their team.",
                &[
                    ("feedback", "feedback given in either direction"),
                    ("growth", "goals, career and learning discussed"),
                    (
                        "wellbeing",
                        "workload and how they're doing, only as far as it was raised",
                    ),
                ],
            ),
            "sales_call" => t(
                "A call with a customer or prospect.",
                &[
                    (
                        "customer",
                        "who the customer is, their situation and what they need",
                    ),
                    (
                        "pain_points",
                        "the problems they described, in their words where possible",
                    ),
                    (
                        "objections",
                        "concerns and objections, and how they were answered",
                    ),
                    (
                        "budget_and_timeline",
                        "budget, decision makers and timing, as far as said",
                    ),
                ],
            ),
            "interview" => t(
                "A job interview. Stick to what the candidate said and did; no judgments about protected characteristics.",
                &[
                    (
                        "candidate",
                        "background and experience the candidate described",
                    ),
                    ("strengths", "strengths shown, with examples"),
                    ("concerns", "gaps or concerns, with examples"),
                    ("questions_asked", "questions the candidate asked"),
                ],
            ),
            "retro" => t(
                "A team retrospective.",
                &[
                    ("went_well", "what went well"),
                    ("to_improve", "what didn't go well or could improve"),
                ],
            ),
            "lecture" => t(
                "A lecture or talk: notes for someone studying it.",
                &[
                    (
                        "key_concepts",
                        "the main ideas, each explained in a sentence or two",
                    ),
                    ("examples", "examples and demonstrations given"),
                    ("references", "books, papers, tools or links mentioned"),
                ],
            ),
            _ => return None,
        })
    }

    /// A built-in by name, `general` when none is given.
    pub fn named(name: Option<&str>) -> Result<NotesTemplate> {
        let n = name.unwrap_or("general");
        Self::builtin(n).ok_or_else(|| {
            invalid(format!(
                "no notes template {n:?}; built in: {}",
                Self::BUILTIN.join(", ")
            ))
        })
    }
}

/// Something to do, from a meeting.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ActionItem {
    pub task: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// As said ("Friday", "end of Q3") or a date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    /// When in the recording it was agreed, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MeetingNotes {
    pub title: String,
    /// Markdown.
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub action_items: Vec<ActionItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub open_questions: Vec<String>,
    /// What was discussed when.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub topics: Vec<Chapter>,
    /// The template's own sections, in Markdown.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sections: BTreeMap<String, String>,
    pub template: String,
}

fn heading(key: &str) -> String {
    let words = key.replace(['_', '-'], " ");
    let mut c = words.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

impl MeetingNotes {
    /// The notes as one Markdown document, with who spoke how much when
    /// `insights` are given.
    pub fn markdown(&self, insights: Option<&AudioInsights>) -> String {
        let mut md = format!("# {}\n\n{}\n", self.title, self.summary.trim());
        if let Some(i) = insights
            && !i.speakers.is_empty()
        {
            md.push_str("\n**Speakers:** ");
            md.push_str(
                &i.speakers
                    .iter()
                    .map(|s| format!("{} ({:.0}%)", s.name, s.share * 100.0))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            md.push('\n');
        }
        if !self.decisions.is_empty() {
            md.push_str("\n## Decisions\n\n");
            for d in &self.decisions {
                md.push_str(&format!("- {d}\n"));
            }
        }
        if !self.action_items.is_empty() {
            md.push_str("\n## Action items\n\n");
            for a in &self.action_items {
                md.push_str(&format!("- [ ] {}", a.task));
                let mut tail = Vec::new();
                if let Some(o) = &a.owner {
                    tail.push(format!("**{o}**"));
                }
                if let Some(d) = &a.due {
                    tail.push(format!("due {d}"));
                }
                if !tail.is_empty() {
                    md.push_str(&format!(" ({})", tail.join(", ")));
                }
                md.push('\n');
            }
        }
        for (k, v) in &self.sections {
            if !v.trim().is_empty() {
                md.push_str(&format!("\n## {}\n\n{}\n", heading(k), v.trim()));
            }
        }
        if !self.open_questions.is_empty() {
            md.push_str("\n## Open questions\n\n");
            for q in &self.open_questions {
                md.push_str(&format!("- {q}\n"));
            }
        }
        if !self.topics.is_empty() {
            md.push_str("\n## Topics\n\n");
            for t in &self.topics {
                md.push_str(&format!("- **{}** {}\n", clock(t.start), t.title));
            }
        }
        md
    }
}

const NOTES_SYSTEM: &str = "You take meeting notes from transcripts. Keep names, numbers, dates and commitments exact; never add anything that wasn't said. A decision is something the group agreed; an action item is something a person will do. Answer only with JSON.";
/// Characters of transcript read at once; longer meetings are noted in
/// parts first.
const WHOLE_CHARS: usize = 60_000;
const PART_CHARS: usize = 30_000;

/// Notes on a meeting from its transcript (with speakers and times where
/// known), written with `model` to `template`. `context` is what else is
/// known: the agenda, who attended.
pub async fn meeting_notes(
    models: &ModelRegistry,
    model: &str,
    transcript: &Transcript,
    template: &NotesTemplate,
    context: Option<&str>,
    meter: &Meter,
) -> Result<MeetingNotes> {
    let mut text = transcript.timed_text();
    if text.trim().is_empty() {
        return Err(invalid("the recording has no speech to take notes on"));
    }
    if text.len() > WHOLE_CHARS {
        let parts = pieces(&text, PART_CHARS);
        let n = parts.len();
        let notes: Vec<(usize, Result<String>)> = futures::stream::iter(parts.into_iter().enumerate())
            .map(|(i, p)| async move {
                let r = ask(
                    models,
                    model,
                    "You take detailed notes on parts of long meetings, keeping every name, number, date, decision and commitment, each with its time.",
                    vec![ContentBlock::text(format!(
                        "Part {} of {n} of a meeting transcript. Note what was discussed, decided and agreed (who will do what, by when), and what was left open, as a list; start each point with its time in [m:ss].\n\n{p}",
                        i + 1
                    ))],
                    false,
                    meter,
                )
                .await;
                (i, r)
            })
            .buffer_unordered(4)
            .collect()
            .await;
        let mut notes = notes;
        notes.sort_by_key(|(i, _)| *i);
        let mut joined = Vec::with_capacity(n);
        for (i, r) in notes {
            joined.push(format!("## Part {}\n{}", i + 1, r?));
        }
        text = format!(
            "(Notes on the meeting's {n} parts, in order.)\n\n{}",
            joined.join("\n\n")
        );
    }
    let mut shape = vec![
        "\"title\": \"a short title\"".to_owned(),
        "\"summary\": \"Markdown, about 150 words\"".to_owned(),
        "\"decisions\": [\"...\"]".to_owned(),
        "\"action_items\": [{\"task\": \"...\", \"owner\": \"name or null\", \"due\": \"as said, or null\", \"at\": \"m:ss\"}]".to_owned(),
        "\"open_questions\": [\"...\"]".to_owned(),
        "\"topics\": [{\"start\": \"m:ss\", \"title\": \"...\"}]".to_owned(),
    ];
    if !template.sections.is_empty() {
        shape.push(format!(
            "\"sections\": {{{}}}",
            template
                .sections
                .iter()
                .map(|(k, v)| format!("\"{k}\": \"Markdown: {}\"", v.replace('"', "'")))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let guidance = template
        .instructions
        .as_deref()
        .map(|i| format!(" {i}"))
        .unwrap_or_default();
    let context = context
        .filter(|c| !c.trim().is_empty())
        .map(|c| format!("\n\nWhat else is known about the meeting:\n{c}"))
        .unwrap_or_default();
    let v = ask_json(
        models,
        model,
        NOTES_SYSTEM,
        format!(
            "Write notes on this meeting.{guidance} Answer as {{{}}}. Leave lists empty when there is nothing for them.{context}\n\nTranscript:\n{text}",
            shape.join(", ")
        ),
        meter,
    )
    .await?;
    Ok(notes_from_json(&v, template))
}

fn notes_from_json(v: &Value, template: &NotesTemplate) -> MeetingNotes {
    let mut topics: Vec<Chapter> = v["topics"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| {
            Some(Chapter {
                start: parse_clock(&t["start"])?,
                title: text_field(t, "title")?,
                summary: text_field(t, "summary").unwrap_or_default(),
            })
        })
        .collect();
    topics.sort_by(|a, b| a.start.total_cmp(&b.start));
    let sections = template
        .sections
        .keys()
        .filter_map(|k| {
            let s = &v["sections"][k];
            let text = match s {
                Value::String(t) => t.trim().to_owned(),
                Value::Array(_) => strings(s)
                    .iter()
                    .map(|x| format!("- {x}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                Value::Null => return None,
                other => other.to_string(),
            };
            (!text.is_empty()).then(|| (k.clone(), text))
        })
        .collect();
    MeetingNotes {
        title: text_field(v, "title").unwrap_or_else(|| "Meeting".into()),
        summary: text_field(v, "summary").unwrap_or_default(),
        decisions: strings(&v["decisions"]),
        action_items: v["action_items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| match a {
                Value::String(task) => Some(ActionItem {
                    task: task.clone(),
                    ..Default::default()
                }),
                _ => Some(ActionItem {
                    task: text_field(a, "task")?,
                    owner: text_field(a, "owner"),
                    due: text_field(a, "due"),
                    at: parse_clock(&a["at"]),
                }),
            })
            .collect(),
        open_questions: strings(&v["open_questions"]),
        topics,
        sections,
        template: template.name.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn notes_read_loose_json_and_render() {
        let t = NotesTemplate::named(Some("standup")).unwrap();
        let n = notes_from_json(
            &json!({
                "title": "Standup",
                "summary": "Short one.",
                "decisions": ["Ship on Friday", ""],
                "action_items": [
                    {"task": "Fix login", "owner": "Ben", "due": "Thursday", "at": "2:10"},
                    "Book the room",
                    {"owner": "nobody"}
                ],
                "topics": [{"start": "3:00", "title": "Release"}, {"start": "0:30", "title": "Login bug"}],
                "sections": {"updates": ["Ana: tests", "Ben: login"], "blockers": null}
            }),
            &t,
        );
        assert_eq!(n.decisions, vec!["Ship on Friday"]);
        assert_eq!(n.action_items.len(), 2);
        assert_eq!(n.action_items[0].at, Some(130.0));
        assert_eq!(n.topics[0].title, "Login bug");
        assert_eq!(n.sections["updates"], "- Ana: tests\n- Ben: login");
        assert!(!n.sections.contains_key("blockers"));
        let md = n.markdown(None);
        assert!(
            md.contains("- [ ] Fix login (**Ben**, due Thursday)"),
            "{md}"
        );
        assert!(md.contains("## Updates"), "{md}");
        assert!(NotesTemplate::named(Some("nope")).is_err());
    }
}
