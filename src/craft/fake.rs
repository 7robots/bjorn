//! An in-memory Craft space behind the `Transport` trait, for tests: the
//! subset of the API the store uses, with the shapes the real one returns.

use std::collections::HashMap;
use std::sync::Mutex;

use futures::future::BoxFuture;
use serde_json::{Value, json};

use super::api::Transport;
use super::doc;
use crate::bear::{BearError, Result};

#[derive(Debug, Clone)]
pub struct FakeBlock {
    pub id: String,
    pub markdown: String,
}

#[derive(Debug, Clone)]
pub struct FakeDoc {
    pub id: String,
    pub title: String,
    /// Folder id, or `unsorted` / `trash`.
    pub folder: String,
    pub modified: String,
    pub blocks: Vec<FakeBlock>,
}

#[derive(Debug, Clone)]
pub struct FakeFolder {
    pub id: String,
    pub name: String,
    pub parent: Option<String>,
}

#[derive(Default)]
pub struct FakeSpace {
    pub docs: Vec<FakeDoc>,
    pub folders: Vec<FakeFolder>,
    pub log: Vec<String>,
    next: u64,
    clock: u64,
}

/// The fake API. `space` is public so a test can set it up and look inside.
#[derive(Default)]
pub struct FakeCraft {
    pub space: Mutex<FakeSpace>,
}

fn block_json(b: &FakeBlock) -> Value {
    let md = &b.markdown;
    let first = md.lines().next().unwrap_or("");
    let indent = (first.len() - first.trim_start().len()) / 2;
    let trimmed = first.trim_start();
    let mut v = json!({"id": b.id, "type": "text", "markdown": md});
    if trimmed.starts_with("```") {
        v["type"] = json!("code");
    } else if trimmed.starts_with('|') {
        v["type"] = json!("table");
    } else if trimmed.starts_with("![") {
        v["type"] = json!("image");
    } else if trimmed.starts_with('#') && trimmed.trim_start_matches('#').starts_with(' ') {
        v["textStyle"] = json!(format!(
            "h{}",
            trimmed.len() - trimmed.trim_start_matches('#').len()
        ));
    } else if trimmed.starts_with("- [") {
        v["listStyle"] = json!("task");
    } else if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
        v["listStyle"] = json!("bullet");
    } else if trimmed
        .split_once(". ")
        .is_some_and(|(n, _)| n.chars().all(|c| c.is_ascii_digit()) && !n.is_empty())
    {
        v["listStyle"] = json!("numbered");
    }
    if indent > 0 {
        v["indentationLevel"] = json!(indent);
    }
    v
}

impl FakeSpace {
    fn id(&mut self, prefix: &str) -> String {
        self.next += 1;
        format!("{prefix}{}", self.next)
    }

    fn tick(&mut self) -> String {
        self.clock += 1;
        format!(
            "2026-01-01T00:{:02}:{:02}.000Z",
            self.clock / 60 % 60,
            self.clock % 60
        )
    }

    pub fn add_folder(&mut self, name: &str, parent: Option<&str>) -> String {
        let id = self.id("f");
        self.folders.push(FakeFolder {
            id: id.clone(),
            name: name.into(),
            parent: parent.map(str::to_string),
        });
        id
    }

    /// A document whose body is `markdown`, one block per chunk.
    pub fn add_doc(&mut self, title: &str, folder: &str, markdown: &str) -> String {
        let id = self.id("d");
        let modified = self.tick();
        let blocks = self.blocks_from(markdown);
        self.docs.push(FakeDoc {
            id: id.clone(),
            title: title.into(),
            folder: folder.into(),
            modified,
            blocks,
        });
        id
    }

    fn blocks_from(&mut self, markdown: &str) -> Vec<FakeBlock> {
        doc::chunks(markdown, &[])
            .into_iter()
            .map(|md| FakeBlock {
                id: self.id("b"),
                markdown: md,
            })
            .collect()
    }

    pub fn doc(&self, id: &str) -> Option<&FakeDoc> {
        self.docs.iter().find(|d| d.id == id)
    }

    fn within(&self, folder: &str, outer: &str) -> bool {
        let mut at = Some(folder.to_string());
        while let Some(id) = at {
            if id == outer {
                return true;
            }
            at = self
                .folders
                .iter()
                .find(|f| f.id == id)
                .and_then(|f| f.parent.clone());
        }
        false
    }

    fn folder_tree(&self, parent: Option<&str>) -> Vec<Value> {
        self.folders
            .iter()
            .filter(|f| f.parent.as_deref() == parent)
            .map(|f| {
                json!({
                    "id": f.id, "name": f.name,
                    "documentCount": self.docs.iter().filter(|d| d.folder == f.id).count(),
                    "folders": self.folder_tree(Some(&f.id)),
                })
            })
            .collect()
    }

    fn locate(&self, block: &str) -> Option<(usize, usize)> {
        self.docs.iter().enumerate().find_map(|(d, doc)| {
            doc.blocks
                .iter()
                .position(|b| b.id == block)
                .map(|b| (d, b))
        })
    }

    fn handle(
        &mut self,
        method: &str,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<Value> {
        let q = |key: &str| {
            query
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.clone())
        };
        let body = body.cloned().unwrap_or(Value::Null);
        self.log.push(format!("{method} {path}"));
        let not_found = || BearError::with_code(format!("Craft {path}: HTTP 404"), "not_found");
        match (method, path) {
            ("GET", "/connection") => Ok(json!({"space": {"id": "space1", "name": "Fake"}})),
            ("GET", "/folders") => {
                let count = |loc: &str| self.docs.iter().filter(|d| d.folder == loc).count();
                let mut items = vec![
                    json!({"id": "unsorted", "name": "Unsorted", "documentCount": count("unsorted"), "folders": []}),
                    json!({"id": "trash", "name": "Recently Deleted", "documentCount": count("trash"), "folders": []}),
                    json!({"id": "templates", "name": "Templates", "documentCount": 0, "folders": []}),
                ];
                items.extend(self.folder_tree(None));
                Ok(json!({ "items": items }))
            }
            ("GET", "/documents") => {
                let docs: Vec<&FakeDoc> = match (q("location"), q("folderId")) {
                    (Some(loc), _) => self.docs.iter().filter(|d| d.folder == loc).collect(),
                    (None, Some(f)) => self
                        .docs
                        .iter()
                        .filter(|d| self.within(&d.folder, &f))
                        .collect(),
                    (None, None) => self.docs.iter().collect(),
                };
                let meta = q("fetchMetadata").as_deref() == Some("true");
                let items: Vec<Value> = docs
                    .iter()
                    .map(|d| {
                        let mut v = json!({"id": d.id, "title": d.title});
                        if meta {
                            v["lastModifiedAt"] = json!(d.modified);
                            v["createdAt"] = json!("2026-01-01T00:00:00.000Z");
                        }
                        v
                    })
                    .collect();
                Ok(json!({ "items": items }))
            }
            ("GET", "/documents/search") => {
                let words: Vec<String> = query
                    .iter()
                    .filter(|(k, _)| *k == "include")
                    .map(|(_, v)| v.to_lowercase())
                    .collect();
                let items: Vec<Value> = self
                    .docs
                    .iter()
                    .filter(|d| d.folder != "trash")
                    .filter(|d| {
                        let text = d
                            .blocks
                            .iter()
                            .map(|b| b.markdown.to_lowercase())
                            .collect::<Vec<_>>()
                            .join("\n");
                        words
                            .iter()
                            .all(|w| text.contains(w) || d.title.to_lowercase().contains(w))
                    })
                    .take(20)
                    .map(|d| json!({"documentId": d.id, "markdown": ""}))
                    .collect();
                Ok(json!({ "items": items }))
            }
            ("GET", "/blocks") => {
                let id = q("id").unwrap_or_default();
                let doc = self.doc(&id).ok_or_else(not_found)?;
                let mut v = json!({"id": doc.id, "type": "page", "textStyle": "page", "markdown": doc.title});
                if q("maxDepth").as_deref() != Some("0") {
                    v["content"] = Value::Array(doc.blocks.iter().map(block_json).collect());
                }
                Ok(v)
            }
            ("PUT", "/blocks") => {
                let stamp = self.tick();
                for b in body["blocks"].as_array().cloned().unwrap_or_default() {
                    let id = b["id"].as_str().unwrap_or_default().to_string();
                    let md = b["markdown"].as_str().unwrap_or_default().to_string();
                    if let Some(doc) = self.docs.iter_mut().find(|d| d.id == id) {
                        doc.title = md;
                        doc.modified = stamp.clone();
                    } else {
                        let (d, i) = self.locate(&id).ok_or_else(not_found)?;
                        self.docs[d].blocks[i].markdown = md;
                        self.docs[d].modified = stamp.clone();
                    }
                }
                Ok(json!({"items": []}))
            }
            ("POST", "/blocks") => {
                let md = body["markdown"].as_str().unwrap_or_default().to_string();
                let pos = &body["position"];
                let new = self.blocks_from(&md);
                let ids: Vec<Value> = new.iter().map(|b| json!({"id": b.id})).collect();
                let (d, at) = if let Some(page) = pos["pageId"].as_str() {
                    let d = self
                        .docs
                        .iter()
                        .position(|x| x.id == page)
                        .ok_or_else(not_found)?;
                    (
                        d,
                        if pos["position"] == "start" {
                            0
                        } else {
                            self.docs[d].blocks.len()
                        },
                    )
                } else {
                    let sib = pos["siblingId"].as_str().unwrap_or_default();
                    let (d, i) = self.locate(sib).ok_or_else(not_found)?;
                    (
                        d,
                        if pos["position"] == "before" {
                            i
                        } else {
                            i + 1
                        },
                    )
                };
                let stamp = self.tick();
                let doc = &mut self.docs[d];
                doc.blocks.splice(at..at, new);
                doc.modified = stamp;
                Ok(json!({ "items": ids }))
            }
            ("DELETE", "/blocks") => {
                let stamp = self.tick();
                for id in body["blockIds"].as_array().cloned().unwrap_or_default() {
                    let (d, i) = self
                        .locate(id.as_str().unwrap_or_default())
                        .ok_or_else(not_found)?;
                    self.docs[d].blocks.remove(i);
                    self.docs[d].modified = stamp.clone();
                }
                Ok(json!({"items": []}))
            }
            ("POST", "/documents") => {
                let dest = &body["destination"];
                let folder = dest["folderId"].as_str().unwrap_or("unsorted").to_string();
                let mut items = Vec::new();
                for d in body["documents"].as_array().cloned().unwrap_or_default() {
                    let id = self.add_doc(d["title"].as_str().unwrap_or_default(), &folder, "");
                    items.push(json!({"id": id, "title": d["title"]}));
                }
                Ok(json!({ "items": items }))
            }
            ("DELETE", "/documents") => {
                for id in body["documentIds"].as_array().cloned().unwrap_or_default() {
                    let doc = self
                        .docs
                        .iter_mut()
                        .find(|d| d.id == id.as_str().unwrap_or_default())
                        .ok_or_else(not_found)?;
                    doc.folder = "trash".into();
                }
                Ok(json!({"items": []}))
            }
            ("PUT", "/documents/move") => {
                let folder = body["destination"]["folderId"]
                    .as_str()
                    .unwrap_or("unsorted")
                    .to_string();
                for id in body["documentIds"].as_array().cloned().unwrap_or_default() {
                    let doc = self
                        .docs
                        .iter_mut()
                        .find(|d| d.id == id.as_str().unwrap_or_default())
                        .ok_or_else(not_found)?;
                    doc.folder = folder.clone();
                }
                Ok(json!({"items": []}))
            }
            ("POST", "/folders") => {
                let mut items = Vec::new();
                for f in body["folders"].as_array().cloned().unwrap_or_default() {
                    let id = self.add_folder(
                        f["name"].as_str().unwrap_or_default(),
                        f["parentFolderId"].as_str(),
                    );
                    items.push(json!({"id": id, "name": f["name"]}));
                }
                Ok(json!({ "items": items }))
            }
            _ => Err(BearError::new(format!("fake Craft has no {method} {path}"))),
        }
    }
}

impl Transport for FakeCraft {
    fn request<'a>(
        &'a self,
        method: &'a str,
        path: &'a str,
        query: &'a [(&'a str, String)],
        body: Option<&'a Value>,
    ) -> BoxFuture<'a, Result<Value>> {
        let out = self.space.lock().unwrap().handle(method, path, query, body);
        Box::pin(async move { out })
    }
}

/// Lets one `Arc<FakeCraft>` serve as both of the store's transports.
pub struct Shared(pub std::sync::Arc<FakeCraft>);

impl Transport for Shared {
    fn request<'a>(
        &'a self,
        method: &'a str,
        path: &'a str,
        query: &'a [(&'a str, String)],
        body: Option<&'a Value>,
    ) -> BoxFuture<'a, Result<Value>> {
        self.0.request(method, path, query, body)
    }
}

impl FakeCraft {
    pub fn requests(&self) -> Vec<String> {
        self.space.lock().unwrap().log.clone()
    }

    pub fn clear_log(&self) {
        self.space.lock().unwrap().log.clear();
    }

    pub fn body_of(&self, id: &str) -> Vec<String> {
        self.space
            .lock()
            .unwrap()
            .doc(id)
            .map(|d| d.blocks.iter().map(|b| b.markdown.clone()).collect())
            .unwrap_or_default()
    }

    pub fn blocks(&self, id: &str) -> HashMap<String, String> {
        self.space
            .lock()
            .unwrap()
            .doc(id)
            .map(|d| {
                d.blocks
                    .iter()
                    .map(|b| (b.id.clone(), b.markdown.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }
}
