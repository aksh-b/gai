//! Files in a folder on the server.

use std::path::PathBuf;

use async_trait::async_trait;

use super::{Connector, Fetched, Item, invalid};
use crate::Result;

pub struct Folder {
    root: PathBuf,
    recursive: bool,
    /// Pick up images too (OCR is available).
    images: bool,
}

impl Folder {
    pub fn new(path: &str, recursive: bool, images: bool) -> Self {
        Self {
            root: PathBuf::from(path),
            recursive,
            images,
        }
    }
}

#[async_trait]
impl Connector for Folder {
    async fn list(&self) -> Result<Vec<Item>> {
        let (root, recursive, images) = (self.root.clone(), self.recursive, self.images);
        tokio::task::spawn_blocking(move || {
            if !root.is_dir() {
                return Err(invalid(format!("{} is not a folder", root.display())));
            }
            let mut files = crate::knowledge::supported_files(&root, images);
            if !recursive {
                files.retain(|p| p.parent() == Some(root.as_path()));
            }
            Ok(files
                .into_iter()
                .filter_map(|p| {
                    let meta = std::fs::metadata(&p).ok()?;
                    let modified = meta
                        .modified()
                        .ok()?
                        .duration_since(std::time::UNIX_EPOCH)
                        .ok()?
                        .as_millis();
                    Some(Item {
                        key: p.to_string_lossy().replace('\\', "/"),
                        version: format!("{}-{modified}", meta.len()),
                        title: p.file_stem().map(|s| s.to_string_lossy().into_owned()),
                        url: None,
                        acl: Vec::new(),
                        content: None,
                    })
                })
                .collect())
        })
        .await
        .map_err(invalid)?
    }

    async fn fetch(&self, item: &Item) -> Result<Fetched> {
        let path = PathBuf::from(&item.key);
        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|e| invalid(format!("{}: {e}", item.key)))?;
        Ok(Fetched {
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file".into()),
            content_type: None,
            bytes,
        })
    }
}
