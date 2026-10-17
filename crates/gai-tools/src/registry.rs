use std::collections::BTreeMap;

use crate::SharedTool;

/// Tools by name.
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, SharedTool>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// A registry with every built-in tool.
    pub fn with_builtins() -> Self {
        let mut r = Self::new();
        for t in crate::builtin::all() {
            r.register(t);
        }
        r
    }

    /// Add or replace a tool.
    pub fn register(&mut self, tool: SharedTool) {
        self.tools.insert(tool.spec().name, tool);
    }

    /// Remove every tool whose name starts with `prefix`; returns how many.
    pub fn unregister_prefix(&mut self, prefix: &str) -> usize {
        let before = self.tools.len();
        self.tools.retain(|name, _| !name.starts_with(prefix));
        before - self.tools.len()
    }

    pub fn get(&self, name: &str) -> Option<&SharedTool> {
        self.tools.get(name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.tools.keys().map(String::as_str)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &SharedTool)> {
        self.tools.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Tools matching an agent's tool list. Entries are exact names, or
    /// prefixes ending in `*` (`fs_*`, `mcp:github/*`, `*`).
    /// Returns the matches and any entries that matched nothing.
    pub fn select(&self, patterns: &[String]) -> (Vec<SharedTool>, Vec<String>) {
        let mut chosen: BTreeMap<&str, SharedTool> = BTreeMap::new();
        let mut unmatched = Vec::new();
        for p in patterns {
            let mut matched = false;
            for (name, t) in &self.tools {
                let hit = match p.strip_suffix('*') {
                    Some(prefix) => name.starts_with(prefix),
                    None => name == p,
                };
                if hit {
                    matched = true;
                    chosen.insert(name, t.clone());
                }
            }
            if !matched {
                unmatched.push(p.clone());
            }
        }
        (chosen.into_values().collect(), unmatched)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_by_name_and_prefix() {
        let r = ToolRegistry::with_builtins();
        let (tools, missing) = r.select(&["fs_*".into(), "calculator".into(), "nope".into()]);
        let names: Vec<String> = tools.iter().map(|t| t.spec().name).collect();
        assert!(names.contains(&"fs_read".to_owned()));
        assert!(names.contains(&"calculator".to_owned()));
        assert!(!names.contains(&"shell".to_owned()));
        assert_eq!(missing, ["nope"]);
    }
}
