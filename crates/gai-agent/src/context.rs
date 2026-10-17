//! Keeping a conversation valid and inside the context window.

use std::collections::HashSet;

use gai_core::{ContentBlock, Message, Role, ToolSpec};
use gai_store::History;

const CLEARED: &str = "[Output of an earlier tool call was cleared to save context. Call the tool again if you still need it.]";

/// Shown in place of history the model can no longer see directly.
pub(crate) const RESUME_NOTE: &str =
    "(Earlier steps are summarized in the system prompt. Continue the task.)";

/// The working conversation for a run: what the model is sent each step.
#[derive(Debug, Default)]
pub(crate) struct Conversation {
    pub summary: Option<String>,
    pub messages: Vec<Message>,
    /// Stored sequence number of each message, when it has one.
    pub seqs: Vec<Option<i64>>,
    /// The provider's real input count at the last call and how many
    /// messages were sent then; estimates build on it.
    calibration: Option<(u64, usize)>,
}

impl Conversation {
    pub fn from_history(h: History) -> Self {
        let mut c = Self {
            summary: h.summary,
            ..Default::default()
        };
        for m in h.messages {
            c.messages.push(m.message);
            c.seqs.push(Some(m.seq));
        }
        c
    }

    /// Append a message. A user message following a user message is merged
    /// into it, so tool results and new text share one turn.
    pub fn push(&mut self, m: Message, seq: Option<i64>) {
        if let Some(last) = self.messages.last_mut()
            && last.role == Role::User
            && m.role == Role::User
        {
            last.content.extend(m.content);
            *self.seqs.last_mut().expect("parallel") = seq.or(*self.seqs.last().expect("parallel"));
            return;
        }
        self.messages.push(m);
        self.seqs.push(seq);
    }

    /// Make the history acceptable to every provider: it starts with a user
    /// turn, and every tool call is answered by a result in the next turn.
    /// Returns how many results had to be invented (an interrupted run).
    pub fn normalize(&mut self) -> usize {
        if self
            .messages
            .first()
            .is_some_and(|m| m.role == Role::Assistant)
        {
            self.messages.insert(0, Message::user(RESUME_NOTE));
            self.seqs.insert(0, None);
        }
        let mut added = 0;
        let mut i = 0;
        while i < self.messages.len() {
            if self.messages[i].role != Role::Assistant {
                i += 1;
                continue;
            }
            let ids: Vec<String> = self.messages[i]
                .tool_uses()
                .map(|(id, _, _)| id.to_owned())
                .collect();
            if ids.is_empty() {
                i += 1;
                continue;
            }
            let next_is_user = self
                .messages
                .get(i + 1)
                .is_some_and(|m| m.role == Role::User);
            if !next_is_user {
                self.messages.insert(
                    i + 1,
                    Message {
                        role: Role::User,
                        content: vec![],
                    },
                );
                self.seqs.insert(i + 1, None);
            }
            let next = &mut self.messages[i + 1];
            let answered: HashSet<String> = next
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
                    _ => None,
                })
                .collect();
            let missing: Vec<ContentBlock> = ids
                .iter()
                .filter(|id| !answered.contains(*id))
                .map(|id| {
                    ContentBlock::tool_result(
                        id.clone(),
                        "The run was interrupted before this tool call finished.",
                        true,
                    )
                })
                .collect();
            added += missing.len();
            // Results must come before any other content in the turn.
            let rest = std::mem::take(&mut next.content);
            next.content = missing;
            next.content.extend(rest);
            i += 2;
        }
        added
    }

    /// Rough token count of what would be sent, anchored to the provider's
    /// last real count when there is one.
    pub fn estimate(&self, system: &str, tools: &[ToolSpec]) -> u64 {
        match self.calibration {
            Some((actual, count)) if count <= self.messages.len() => {
                actual
                    + self.messages[count..]
                        .iter()
                        .map(message_tokens)
                        .sum::<u64>()
            }
            _ => {
                text_tokens(system)
                    + tools
                        .iter()
                        .map(|t| {
                            text_tokens(&t.description) + text_tokens(&t.input_schema.to_string())
                        })
                        .sum::<u64>()
                    + self.messages.iter().map(message_tokens).sum::<u64>()
            }
        }
    }

    pub fn calibrate(&mut self, actual_input_tokens: u64, messages_sent: usize) {
        if actual_input_tokens > 0 {
            self.calibration = Some((actual_input_tokens, messages_sent));
        }
    }

    /// Replace all but the `keep` most recent tool results with a stub.
    /// Small results are left alone. Returns how many were cleared.
    pub fn clear_tool_results(&mut self, keep: usize) -> usize {
        let positions: Vec<(usize, usize)> = self
            .messages
            .iter()
            .enumerate()
            .flat_map(|(mi, m)| {
                m.content
                    .iter()
                    .enumerate()
                    .filter_map(move |(bi, b)| match b {
                        ContentBlock::ToolResult { .. } => Some((mi, bi)),
                        _ => None,
                    })
            })
            .collect();
        let mut cleared = 0;
        for &(mi, bi) in positions.iter().take(positions.len().saturating_sub(keep)) {
            if let ContentBlock::ToolResult { content, .. } = &mut self.messages[mi].content[bi] {
                let size: usize = content.iter().map(block_chars).sum();
                if size > 400 {
                    *content = vec![ContentBlock::text(CLEARED)];
                    cleared += 1;
                }
            }
        }
        if cleared > 0 {
            self.calibration = None;
            self.drop_stale_reasoning();
        }
        cleared
    }

    /// Where to cut for compaction: as many leading messages as possible
    /// while keeping at least `keep_recent`. Prefers to cut just before a
    /// user turn that is not a tool result, so the kept history is valid on
    /// its own.
    pub fn compaction_cut(&self, keep_recent: usize) -> Option<usize> {
        let max = self.messages.len().checked_sub(keep_recent.max(2))?;
        let clean_user = |j: usize| {
            let m = &self.messages[j];
            m.role == Role::User
                && !m
                    .content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::ToolResult { .. }))
        };
        let assistant_after_results = |j: usize| self.messages[j].role == Role::Assistant;
        (1..=max)
            .rev()
            .find(|&j| clean_user(j))
            .or_else(|| (1..=max).rev().find(|&j| assistant_after_results(j)))
    }

    /// Drop the first `cut` messages in favour of `summary`.
    pub fn apply_compaction(&mut self, cut: usize, summary: String) {
        self.messages.drain(..cut);
        self.seqs.drain(..cut);
        self.summary = Some(summary);
        self.calibration = None;
        self.normalize();
        self.drop_stale_reasoning();
    }

    /// Once history has been edited, reasoning from earlier turns is no
    /// longer valid for providers that bind it to the exact transcript.
    /// Keep only the reasoning of the turn in progress.
    fn drop_stale_reasoning(&mut self) {
        let current = self
            .messages
            .iter()
            .rposition(|m| {
                m.role == Role::User
                    && m.content
                        .iter()
                        .any(|b| matches!(b, ContentBlock::Text { .. }))
            })
            .unwrap_or(0);
        for m in &mut self.messages[..current] {
            m.content.retain(|b| {
                !matches!(
                    b,
                    ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. }
                )
            });
        }
    }
}

fn block_chars(b: &ContentBlock) -> usize {
    match b {
        ContentBlock::Text { text } => text.len(),
        ContentBlock::Thinking { text, .. } => text.len(),
        ContentBlock::RedactedThinking { data, .. } => data.len() / 2,
        ContentBlock::Image { .. } => 6_000,
        ContentBlock::Audio { .. } => 8_000,
        ContentBlock::ToolUse { input, name, .. } => name.len() + input.to_string().len(),
        ContentBlock::ToolResult { content, .. } => content.iter().map(block_chars).sum(),
    }
}

/// About 3.5 characters per token across English text and code; errs high.
fn text_tokens(s: &str) -> u64 {
    (s.len() as u64 * 2).div_ceil(7)
}

fn message_tokens(m: &Message) -> u64 {
    4 + m
        .content
        .iter()
        .map(|b| (block_chars(b) as u64 * 2).div_ceil(7))
        .sum::<u64>()
}

/// A plain-text rendering of messages for a summarizer to read.
pub(crate) fn transcript(messages: &[Message]) -> String {
    let mut out = String::new();
    for m in messages {
        let who = match m.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
        };
        for b in &m.content {
            match b {
                ContentBlock::Text { text } => out.push_str(&format!("{who}: {text}\n\n")),
                ContentBlock::ToolUse { name, input, .. } => out.push_str(&format!(
                    "[Assistant called {name} with {}]\n\n",
                    clip(&input.to_string(), 1_000)
                )),
                ContentBlock::ToolResult {
                    content, is_error, ..
                } => {
                    let text: String = content.iter().filter_map(ContentBlock::as_text).collect();
                    let label = if *is_error {
                        "Tool error"
                    } else {
                        "Tool result"
                    };
                    out.push_str(&format!("[{label}: {}]\n\n", clip(&text, 1_500)));
                }
                ContentBlock::Image { .. } => out.push_str(&format!("[{who} shared an image]\n\n")),
                ContentBlock::Audio { .. } => out.push_str(&format!(
                    "[{who} shared audio]

"
                )),
                ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. } => {}
            }
        }
    }
    out
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool_call(id: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: id.into(),
                name: "t".into(),
                input: json!({}),
            }],
        }
    }

    fn tool_result(id: &str, text: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::tool_result(id, text, false)],
        }
    }

    #[test]
    fn normalize_answers_interrupted_calls_and_fixes_the_start() {
        let mut c = Conversation::default();
        c.push(Message::assistant("hello"), Some(1));
        c.push(tool_call("a"), Some(2));
        c.push(Message::user("next question"), Some(3));
        c.push(tool_call("b"), Some(4));
        assert_eq!(c.normalize(), 2);
        assert_eq!(c.messages[0].text(), RESUME_NOTE);
        // "a" is answered inside the following user turn, before its text.
        assert!(
            matches!(&c.messages[3].content[0], ContentBlock::ToolResult { tool_use_id, is_error: true, .. } if tool_use_id == "a")
        );
        assert_eq!(c.messages[3].text(), "next question");
        assert!(
            matches!(&c.messages[5].content[0], ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "b")
        );
        assert_eq!(c.normalize(), 0, "idempotent");
    }

    #[test]
    fn user_turns_merge() {
        let mut c = Conversation::default();
        c.push(tool_call("a"), None);
        c.push(tool_result("a", "x"), Some(2));
        c.push(Message::user("more"), Some(3));
        assert_eq!(c.messages.len(), 2);
        assert_eq!(c.messages[1].content.len(), 2);
        assert_eq!(c.seqs[1], Some(3));
    }

    #[test]
    fn clears_old_large_results_only() {
        let mut c = Conversation::default();
        c.push(Message::user("go"), None);
        let big = "x".repeat(1_000);
        for id in ["1", "2", "3"] {
            c.push(tool_call(id), None);
            c.push(tool_result(id, &big), None);
        }
        assert_eq!(c.clear_tool_results(1), 2);
        assert!(transcript(&c.messages).contains("cleared"));
        assert_eq!(c.clear_tool_results(1), 0, "already cleared");
    }

    #[test]
    fn compaction_cuts_before_a_clean_user_turn() {
        let mut c = Conversation::default();
        c.push(Message::user("q1"), Some(1));
        c.push(Message::assistant("a1"), Some(2));
        c.push(Message::user("q2"), Some(3));
        c.push(tool_call("t"), Some(4));
        c.push(tool_result("t", "r"), Some(5));
        c.push(Message::assistant("a2"), Some(6));
        let cut = c.compaction_cut(3).unwrap();
        assert_eq!(cut, 2);
        c.apply_compaction(cut, "q1 was answered".into());
        assert_eq!(c.messages[0].text(), "q2");
        assert_eq!(c.summary.as_deref(), Some("q1 was answered"));

        // A single long tool loop has no clean user turn: cut at an
        // assistant turn and let normalize add a user turn in front.
        let mut loop_only = Conversation::default();
        loop_only.push(Message::user("do it"), None);
        for id in ["1", "2", "3", "4"] {
            loop_only.push(tool_call(id), None);
            loop_only.push(tool_result(id, "ok"), None);
        }
        let cut = loop_only.compaction_cut(2).unwrap();
        assert_eq!(loop_only.messages[cut].role, Role::Assistant);
        loop_only.apply_compaction(cut, "s".into());
        assert_eq!(loop_only.messages[0].text(), RESUME_NOTE);
    }

    #[test]
    fn estimates_use_calibration() {
        let mut c = Conversation::default();
        c.push(Message::user("x".repeat(700)), None);
        let rough = c.estimate("", &[]);
        assert!((190..=220).contains(&rough), "{rough}");
        c.calibrate(5_000, 1);
        c.push(Message::assistant("y".repeat(70)), None);
        assert_eq!(c.estimate("", &[]), 5_000 + 4 + 20);
    }
}
