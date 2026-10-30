//! The language an agent answers in.
//!
//! An agent's `language` (or a run's, or the server's default) is a
//! language code (`fr`, `pt-BR`) or a name (`French`). Its prompt can use
//! `{{ language }}`; when it doesn't, a line asking for replies in that
//! language goes at the end of the system prompt.

/// Language names by code, for the instruction.
const NAMES: &[(&str, &str)] = &[
    ("ar", "Arabic"),
    ("bn", "Bengali"),
    ("cs", "Czech"),
    ("da", "Danish"),
    ("de", "German"),
    ("el", "Greek"),
    ("en", "English"),
    ("es", "Spanish"),
    ("fa", "Persian"),
    ("fi", "Finnish"),
    ("fr", "French"),
    ("he", "Hebrew"),
    ("hi", "Hindi"),
    ("hu", "Hungarian"),
    ("id", "Indonesian"),
    ("it", "Italian"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("mr", "Marathi"),
    ("ms", "Malay"),
    ("nl", "Dutch"),
    ("no", "Norwegian"),
    ("pl", "Polish"),
    ("pt", "Portuguese"),
    ("ro", "Romanian"),
    ("ru", "Russian"),
    ("sv", "Swedish"),
    ("sw", "Swahili"),
    ("ta", "Tamil"),
    ("te", "Telugu"),
    ("th", "Thai"),
    ("tr", "Turkish"),
    ("uk", "Ukrainian"),
    ("ur", "Urdu"),
    ("vi", "Vietnamese"),
    ("zh", "Chinese"),
];

const REGIONS: &[(&str, &str)] = &[
    ("pt-br", "Brazilian Portuguese"),
    ("pt-pt", "European Portuguese"),
    ("zh-cn", "Simplified Chinese"),
    ("zh-hans", "Simplified Chinese"),
    ("zh-tw", "Traditional Chinese"),
    ("zh-hant", "Traditional Chinese"),
    ("en-gb", "British English"),
    ("en-us", "American English"),
    ("es-mx", "Mexican Spanish"),
    ("fr-ca", "Canadian French"),
];

/// A language's name: `fr` → `French`, `pt-BR` → `Brazilian Portuguese`;
/// anything else is taken as a name already.
pub fn language_name(code: &str) -> String {
    let lower = code.trim().to_lowercase().replace('_', "-");
    if let Some((_, n)) = REGIONS.iter().find(|(c, _)| *c == lower) {
        return (*n).to_owned();
    }
    let base = lower.split('-').next().unwrap_or_default();
    match NAMES.iter().find(|(c, _)| *c == base) {
        Some((_, n)) if base.len() == 2 => (*n).to_owned(),
        _ => code.trim().to_owned(),
    }
}

/// Whether a system prompt template places `{{ language }}` itself.
pub fn template_uses_language(template: &str) -> bool {
    let env = minijinja::Environment::new();
    match env.template_from_str(template) {
        Ok(t) => t.undeclared_variables(false).contains("language"),
        Err(_) => false,
    }
}

/// The line added to a system prompt that doesn't place the language.
pub fn instruction(name: &str) -> String {
    format!(
        "Always reply in {name}, whatever language the user writes in, unless they ask you to use another one."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(language_name("fr"), "French");
        assert_eq!(language_name("pt-BR"), "Brazilian Portuguese");
        assert_eq!(language_name("de_AT"), "German");
        assert_eq!(language_name("Klingon"), "Klingon");
        assert!(template_uses_language("Answer in {{ language }}."));
        assert!(!template_uses_language("Be brief."));
    }
}
