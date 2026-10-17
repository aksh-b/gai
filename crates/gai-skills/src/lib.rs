//! Agent Skills.
//!
//! A skill is a folder with a `SKILL.md` (YAML front matter with `name` and
//! `description`, then instructions) and any files the instructions refer
//! to: scripts, templates, reference notes. This is the Anthropic Agent
//! Skills format, so existing skills work unchanged.
//!
//! Skills are offered by progressive disclosure: an agent's system prompt
//! lists only each skill's name and description ([`index`]); the agent
//! calls `load_skill` ([`load_skill_tool`]) to read the full instructions
//! when a task needs them, which also copies the skill's files into its
//! workspace so its tools can use them.
//!
//! Skills come from folders on disk (`skills_dirs`) or are uploaded through
//! the API and kept in the store. A folder skill wins over a stored skill
//! with the same name.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use gai_store::{SharedStore, StoreError};
use gai_tools::{SharedTool, ToolCtx, ToolError, ToolOutput, typed_tool};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("{0}")]
    Io(String),
}

pub type Result<T, E = SkillError> = std::result::Result<T, E>;

/// Where a skill came from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SkillSource {
    /// A folder on disk.
    Dir { path: PathBuf },
    /// Uploaded through the API.
    Stored { version: u32 },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// The instructions: SKILL.md without its front matter.
    pub instructions: String,
    /// The whole SKILL.md.
    pub raw: String,
    pub source: SkillSource,
    /// Bundled files, as paths relative to the skill folder.
    pub files: Vec<String>,
    /// Front matter fields other than name and description.
    pub metadata: Map<String, Value>,
    /// Contents of bundled files for stored skills (folder skills read
    /// theirs from disk).
    #[serde(skip)]
    stored_files: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct FrontMatter {
    name: String,
    description: String,
    #[serde(flatten)]
    rest: Map<String, Value>,
}

/// Split a SKILL.md into its front matter and instructions, checking the
/// name and description.
pub fn parse(raw: &str) -> Result<(String, String, Map<String, Value>, String)> {
    let text = raw
        .strip_prefix('\u{feff}')
        .unwrap_or(raw)
        .replace("\r\n", "\n");
    let rest = text.strip_prefix("---\n").ok_or_else(|| {
        SkillError::Invalid("SKILL.md must start with YAML front matter between --- lines".into())
    })?;
    let end = rest.find("\n---").ok_or_else(|| {
        SkillError::Invalid("SKILL.md front matter is not closed with ---".into())
    })?;
    let fm: FrontMatter = serde_yaml::from_str(&rest[..end])
        .map_err(|e| SkillError::Invalid(format!("SKILL.md front matter: {e}")))?;
    let body = rest[end + 4..]
        .trim_start_matches(|c| c != '\n')
        .trim()
        .to_owned();
    validate_name(&fm.name)?;
    let description = fm.description.trim().to_owned();
    if description.is_empty() || description.chars().count() > 1024 {
        return Err(SkillError::Invalid(
            "a skill description must be 1 to 1024 characters".into(),
        ));
    }
    Ok((fm.name, description, fm.rest, body))
}

pub fn validate_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-');
    if ok {
        Ok(())
    } else {
        Err(SkillError::Invalid(format!(
            "skill name {name:?}: use lowercase letters, digits and '-' (at most 64)"
        )))
    }
}

/// A relative path inside a skill, or an error for anything that could
/// escape it.
fn clean_rel(rel: &str) -> Result<PathBuf> {
    let p = Path::new(rel.trim());
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(s) => out.push(s),
            Component::CurDir => {}
            _ => {
                return Err(SkillError::Invalid(format!(
                    "{rel:?} is not a path inside the skill"
                )));
            }
        }
    }
    if out.as_os_str().is_empty() {
        return Err(SkillError::Invalid("empty file path".into()));
    }
    Ok(out)
}

const MAX_FILES: usize = 200;
const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;

fn dir_files(root: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            if out.len() >= MAX_FILES {
                return;
            }
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name == "__pycache__" || name == "node_modules" {
                continue;
            }
            if p.is_dir() {
                walk(root, &p, out);
            } else if let Ok(rel) = p.strip_prefix(root) {
                let rel = rel.to_string_lossy().replace('\\', "/");
                if rel != "SKILL.md" {
                    out.push(rel);
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

impl Skill {
    /// Read a skill folder.
    pub fn from_dir(dir: &Path) -> Result<Self> {
        let md = dir.join("SKILL.md");
        let raw = std::fs::read_to_string(&md)
            .map_err(|e| SkillError::Io(format!("{}: {e}", md.display())))?;
        let (name, description, metadata, instructions) =
            parse(&raw).map_err(|e| SkillError::Invalid(format!("{}: {e}", md.display())))?;
        Ok(Self {
            name,
            description,
            instructions,
            raw,
            source: SkillSource::Dir {
                path: dir.to_path_buf(),
            },
            files: dir_files(dir),
            metadata,
            stored_files: BTreeMap::new(),
        })
    }

    fn from_stored(s: gai_store::StoredSkill) -> Result<Self> {
        let (name, description, metadata, instructions) = parse(&s.body)?;
        Ok(Self {
            name,
            description,
            instructions,
            raw: s.body,
            source: SkillSource::Stored { version: s.version },
            files: s.files.keys().cloned().collect(),
            metadata,
            stored_files: s.files,
        })
    }

    /// A bundled file's bytes.
    pub fn read_file(&self, rel: &str) -> Result<Vec<u8>> {
        let clean = clean_rel(rel)?;
        let key = clean.to_string_lossy().replace('\\', "/");
        if !self.files.contains(&key) {
            return Err(SkillError::Invalid(format!(
                "{} has no file {key:?}; files: {}",
                self.name,
                self.files.join(", ")
            )));
        }
        match &self.source {
            SkillSource::Dir { path } => {
                std::fs::read(path.join(&clean)).map_err(|e| SkillError::Io(format!("{key}: {e}")))
            }
            SkillSource::Stored { .. } => Ok(self.stored_files[&key].clone().into_bytes()),
        }
    }

    /// Copy the skill's files into `dest` (in an agent's workspace).
    pub fn materialize(&self, dest: &Path) -> Result<usize> {
        let mut n = 0;
        for rel in &self.files {
            let target = dest.join(clean_rel(rel)?);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| SkillError::Io(e.to_string()))?;
            }
            match &self.source {
                SkillSource::Dir { path } => {
                    let src = path.join(clean_rel(rel)?);
                    if std::fs::metadata(&src).map(|m| m.len()).unwrap_or(0) > MAX_FILE_BYTES {
                        continue;
                    }
                    std::fs::copy(&src, &target)
                        .map_err(|e| SkillError::Io(format!("{rel}: {e}")))?;
                }
                SkillSource::Stored { .. } => {
                    std::fs::write(&target, &self.stored_files[rel])
                        .map_err(|e| SkillError::Io(format!("{rel}: {e}")))?;
                }
            }
            n += 1;
        }
        Ok(n)
    }
}

/// Every skill gai knows: folders first, then stored skills.
#[derive(Clone)]
pub struct Skills {
    dirs: Vec<PathBuf>,
    store: SharedStore,
}

impl Skills {
    pub fn new(dirs: Vec<PathBuf>, store: SharedStore) -> Self {
        Self { dirs, store }
    }

    pub fn dirs(&self) -> &[PathBuf] {
        &self.dirs
    }

    fn dir_skills(&self) -> Vec<Skill> {
        let mut out: Vec<Skill> = Vec::new();
        for root in &self.dirs {
            let Ok(entries) = std::fs::read_dir(root) else {
                continue;
            };
            let mut dirs: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.join("SKILL.md").is_file())
                .collect();
            dirs.sort();
            for d in dirs {
                match Skill::from_dir(&d) {
                    Ok(s) if out.iter().any(|o| o.name == s.name) => {
                        tracing::warn!(skill = %s.name, dir = %d.display(), "duplicate skill name; keeping the first");
                    }
                    Ok(s) => out.push(s),
                    Err(e) => tracing::warn!(error = %e, "skipping invalid skill"),
                }
            }
        }
        out
    }

    /// All skills, sorted by name.
    pub async fn list(&self) -> Result<Vec<Skill>> {
        let mut out = self.dir_skills();
        for s in self.store.list_skills().await? {
            if out.iter().any(|o| o.name == s.name) {
                continue;
            }
            match Skill::from_stored(s) {
                Ok(s) => out.push(s),
                Err(e) => tracing::warn!(error = %e, "skipping invalid stored skill"),
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    pub async fn get(&self, name: &str) -> Result<Option<Skill>> {
        Ok(self.list().await?.into_iter().find(|s| s.name == name))
    }

    /// Skills matching an agent's list: names, or prefixes ending in `*`.
    /// Returns the matches and the entries that matched nothing.
    pub async fn select(&self, patterns: &[String]) -> Result<(Vec<Skill>, Vec<String>)> {
        if patterns.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        let all = self.list().await?;
        let mut chosen: BTreeMap<String, Skill> = BTreeMap::new();
        let mut missing = Vec::new();
        for p in patterns {
            let mut hit = false;
            for s in &all {
                let m = match p.strip_suffix('*') {
                    Some(prefix) => s.name.starts_with(prefix),
                    None => &s.name == p,
                };
                if m {
                    hit = true;
                    chosen.insert(s.name.clone(), s.clone());
                }
            }
            if !hit {
                missing.push(p.clone());
            }
        }
        Ok((chosen.into_values().collect(), missing))
    }

    /// Store an uploaded skill. Returns its name and version.
    pub async fn save(&self, raw: &str, files: BTreeMap<String, String>) -> Result<(String, u32)> {
        let (name, description, _, _) = parse(raw)?;
        if files.len() > MAX_FILES {
            return Err(SkillError::Invalid(format!(
                "a skill may bundle at most {MAX_FILES} files"
            )));
        }
        let mut clean = BTreeMap::new();
        for (k, v) in files {
            let key = clean_rel(&k)?.to_string_lossy().replace('\\', "/");
            if key == "SKILL.md" {
                continue;
            }
            clean.insert(key, v);
        }
        if self.dir_skills().iter().any(|s| s.name == name) {
            return Err(SkillError::Invalid(format!(
                "skill {name} comes from a skills folder; change it there"
            )));
        }
        let version = self
            .store
            .put_skill(&name, &description, raw, &clean)
            .await?;
        Ok((name, version))
    }

    pub async fn delete(&self, name: &str) -> Result<bool> {
        Ok(self.store.delete_skill(name).await?)
    }
}

/// The system prompt section listing skills an agent can load.
pub fn index(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut s = String::from(
        "<skills>\nYou have skills: packaged instructions (and sometimes scripts or templates) for particular kinds of task. When a task matches a skill's description, call load_skill with its name before starting, then follow its instructions.\n",
    );
    for k in skills {
        s.push_str(&format!("- {}: {}\n", k.name, k.description));
    }
    s.push_str("</skills>");
    s
}

#[derive(Deserialize, JsonSchema)]
struct LoadArgs {
    /// The skill's name.
    name: String,
    /// A file bundled with the skill, to read instead of the instructions.
    #[serde(default)]
    file: Option<String>,
}

/// `load_skill`, offering exactly `skills`.
pub fn load_skill_tool(skills: Vec<Skill>) -> SharedTool {
    let names: Vec<String> = skills.iter().map(|s| s.name.clone()).collect();
    let skills = Arc::new(skills);
    Arc::new(
        typed_tool(
            "load_skill",
            &format!(
                "Load a skill's full instructions (skills: {}). Its bundled files are copied into your workspace under .skills/<name>/. Pass `file` to read one of those files instead.",
                names.join(", ")
            ),
            move |ctx: ToolCtx, a: LoadArgs| {
                let skills = skills.clone();
                async move {
                    let skill = skills.iter().find(|s| s.name == a.name).ok_or_else(|| {
                        ToolError::InvalidInput(format!(
                            "no skill {:?}; available: {}",
                            a.name,
                            skills.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ")
                        ))
                    })?;
                    if let Some(file) = a.file {
                        let bytes = skill.read_file(&file).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
                        return match String::from_utf8(bytes) {
                            Ok(text) => Ok(ToolOutput::text(text)),
                            Err(_) => Ok(ToolOutput::text(format!(
                                "{file} is a binary file; it is in your workspace at .skills/{}/{file}",
                                skill.name
                            ))),
                        };
                    }
                    let mut out = format!("# Skill: {}\n\n{}\n", skill.name, skill.instructions);
                    if !skill.files.is_empty() {
                        let dest = ctx.workspace.join(".skills").join(&skill.name);
                        let s = skill.clone();
                        let copied = tokio::task::spawn_blocking(move || s.materialize(&dest))
                            .await
                            .map_err(|e| ToolError::Failed(e.to_string()))?
                            .map_err(|e| ToolError::Failed(e.to_string()))?;
                        out.push_str(&format!(
                            "\nBundled files ({copied} copied to .skills/{}/ in your workspace): {}\nRead one with load_skill(name, file) or the file tools; run scripts with the shell tool from that folder.",
                            skill.name,
                            skill.files.join(", ")
                        ));
                    }
                    Ok(ToolOutput::text(out))
                }
            },
        )
        .idempotent(),
    )
}

#[cfg(test)]
mod tests {
    use gai_store::SqliteStore;
    use serde_json::json;

    use super::*;

    const PDF_SKILL: &str = "---\nname: pdf-report\ndescription: Build PDF reports from data. Use when asked for a report.\nlicense: MIT\n---\n# PDF reports\n\nRun `python make.py` with the data file.\n";

    fn write_skill(root: &Path) {
        let d = root.join("pdf-report");
        std::fs::create_dir_all(d.join("templates")).unwrap();
        std::fs::write(d.join("SKILL.md"), PDF_SKILL).unwrap();
        std::fs::write(d.join("make.py"), "print('report')").unwrap();
        std::fs::write(d.join("templates/base.html"), "<h1>{{title}}</h1>").unwrap();
        let bad = root.join("broken");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join("SKILL.md"), "no front matter").unwrap();
    }

    #[test]
    fn parses_front_matter() {
        let (name, desc, meta, body) = parse(PDF_SKILL).unwrap();
        assert_eq!(name, "pdf-report");
        assert!(desc.starts_with("Build PDF"));
        assert_eq!(meta["license"], "MIT");
        assert!(body.starts_with("# PDF reports"));
        assert!(parse("---\nname: Bad Name\ndescription: x\n---\nbody").is_err());
        assert!(parse("---\nname: ok\ndescription: ''\n---\nbody").is_err());
        assert!(parse("---\nname: ok\n").is_err());
    }

    #[tokio::test]
    async fn folders_and_store_with_progressive_disclosure() {
        let root = tempfile::tempdir().unwrap();
        write_skill(root.path());
        let store: SharedStore = Arc::new(SqliteStore::temporary().await.unwrap());
        let skills = Skills::new(vec![root.path().to_path_buf()], store);

        let files = BTreeMap::from([("notes/tips.md".to_owned(), "Be brief.".to_owned())]);
        let (name, v) = skills
            .save(
                "---\nname: summarize\ndescription: Summarize long text.\n---\nKeep it short.",
                files,
            )
            .await
            .unwrap();
        assert_eq!((name.as_str(), v), ("summarize", 1));
        assert!(
            skills.save(PDF_SKILL, BTreeMap::new()).await.is_err(),
            "folder skill wins"
        );
        assert!(
            skills
                .save(
                    "---\nname: evil\ndescription: x\n---\nx",
                    BTreeMap::from([("../x".to_owned(), String::new())])
                )
                .await
                .is_err()
        );

        let all = skills.list().await.unwrap();
        assert_eq!(
            all.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["pdf-report", "summarize"]
        );
        assert_eq!(all[0].files, ["make.py", "templates/base.html"]);
        let (sel, missing) = skills
            .select(&["pdf-*".into(), "nope".into()])
            .await
            .unwrap();
        assert_eq!((sel.len(), missing), (1, vec!["nope".to_owned()]));

        let idx = index(&all);
        assert!(idx.contains("- pdf-report: Build PDF reports"));
        assert!(
            !idx.contains("python make.py"),
            "instructions stay out of the prompt"
        );

        let ws = tempfile::tempdir().unwrap();
        let ctx = ToolCtx::for_workspace(ws.path());
        let tool = load_skill_tool(all);
        let out = tool
            .call(&ctx, json!({"name": "pdf-report"}))
            .await
            .unwrap()
            .as_text();
        assert!(
            out.contains("python make.py") && out.contains("2 copied"),
            "{out}"
        );
        assert!(
            ws.path()
                .join(".skills/pdf-report/templates/base.html")
                .is_file()
        );
        let f = tool
            .call(&ctx, json!({"name": "summarize", "file": "notes/tips.md"}))
            .await
            .unwrap();
        assert_eq!(f.as_text(), "Be brief.");
        assert!(
            tool.call(
                &ctx,
                json!({"name": "pdf-report", "file": "../../etc/passwd"})
            )
            .await
            .is_err()
        );
        assert!(tool.call(&ctx, json!({"name": "other"})).await.is_err());
    }
}
