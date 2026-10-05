//! Google Drive backup into a "Lorekeeper" folder the app creates in My Drive. The drive.file
//! permission only lets Lorekeeper see files it made. Drive has ids instead of paths, so the
//! manifest keeps the id of every folder and file.

use serde_json::{json, Value};

use crate::{
    cloud::{self, bearer, call, ok, Manifest, Plan, Provider, Uploaded},
    github::AGENT,
};

const P: Provider = Provider::Google;
const FILES: &str = "https://www.googleapis.com/drive/v3/files";
const UPLOAD: &str = "https://www.googleapis.com/upload/drive/v3/files";
const FOLDER: &str = "application/vnd.google-apps.folder";

pub fn account(token: &str) -> Result<String, String> {
    let url = "https://www.googleapis.com/drive/v3/about?fields=user(displayName,emailAddress)";
    let v = ok(P, call(P, || AGENT.get(url).header("Authorization", bearer(token)).call())?)?;
    let user = &v["user"];
    user["emailAddress"].as_str().or(user["displayName"].as_str()).map(String::from).ok_or_else(|| "Google Drive sent an unexpected reply.".into())
}

/// The folder a path is in: "NPCs/Villains" for "NPCs/Villains/Vex.md", "" (Lorekeeper) at the top.
pub fn parent(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// The folders to create before `rel` can be uploaded, outermost first; "" is the Lorekeeper folder.
pub fn missing_folders(rel: &str, known: &std::collections::BTreeMap<String, String>) -> Vec<String> {
    let dir = parent(rel);
    let mut all = vec![String::new()];
    all.extend(dir.match_indices('/').map(|(i, _)| dir[..i].to_string()));
    if !dir.is_empty() {
        all.push(dir.to_string());
    }
    all.retain(|d| !known.contains_key(d));
    all
}

fn id_of(v: &Value) -> Result<String, String> {
    v["id"].as_str().map(String::from).ok_or_else(|| "Google Drive sent an unexpected reply.".into())
}

/// A folder was deleted in Drive: forget every id, so the next backup uploads everything into a new
/// Lorekeeper folder.
fn start_over(m: &mut Manifest) -> String {
    m.files.clear();
    m.folders.clear();
    "A Lorekeeper folder in Google Drive was removed. The next backup will upload all your notes again.".into()
}

/// The folder in My Drive a campaign's notes go in: "Lorekeeper" for the main campaign (""), else
/// "Lorekeeper - <name>".
pub fn top_folder(campaign: &str) -> String {
    if campaign.is_empty() { "Lorekeeper".into() } else { format!("Lorekeeper - {campaign}") }
}

/// `top` from top_folder(): the name of the folder "" when it has to be created.
pub fn push(token: &str, plan: &Plan, m: &mut Manifest, top: &str) -> Result<(), String> {
    // Moved to the Drive trash, never deleted for good. Files without an id were never uploaded.
    for rel in &plan.delete {
        let id = m.files.get(rel).map(|u| u.id.clone()).unwrap_or_default();
        if !id.is_empty() {
            let url = format!("{FILES}/{id}");
            let reply = call(P, || AGENT.patch(&url).header("Authorization", bearer(token)).send_json(json!({ "trashed": true })))?;
            if reply.0 != 404 {
                ok(P, reply)?;
            }
        }
        m.files.remove(rel);
    }
    for f in &plan.upload {
        for dir in missing_folders(&f.rel, &m.folders) {
            let name = dir.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or(top);
            let mut meta = json!({ "name": name, "mimeType": FOLDER });
            if !dir.is_empty() {
                meta["parents"] = json!([m.folders[parent(&dir)]]);
            }
            let (status, v) = call(P, || AGENT.post(format!("{FILES}?fields=id")).header("Authorization", bearer(token)).send_json(&meta))?;
            if status == 404 {
                return Err(start_over(m));
            }
            m.folders.insert(dir, id_of(&ok(P, (status, v))?)?);
        }
        let (bytes, hash) = cloud::read(f)?;
        let known = m.files.get(&f.rel).map(|u| u.id.clone()).filter(|id| !id.is_empty());
        // Replace the content of the file uploaded before; if it was deleted in Drive, upload it again.
        if let Some(id) = &known {
            let url = format!("{UPLOAD}/{id}?uploadType=media&fields=id");
            let reply = call(P, || AGENT.patch(&url).header("Authorization", bearer(token)).header("Content-Type", "application/octet-stream").send(&bytes[..]))?;
            if reply.0 != 404 {
                ok(P, reply)?;
                m.files.insert(f.rel.clone(), Uploaded { hash, id: id.clone() });
                continue;
            }
        }
        let name = f.rel.rsplit('/').next().unwrap_or(&f.rel);
        let meta = json!({ "name": name, "parents": [m.folders[parent(&f.rel)]] });
        let boundary = cloud::random_token();
        let mut body = format!("--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{meta}\r\n--{boundary}\r\nContent-Type: application/octet-stream\r\n\r\n").into_bytes();
        body.extend_from_slice(&bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let (status, v) = call(P, || {
            AGENT.post(format!("{UPLOAD}?uploadType=multipart&fields=id"))
                .header("Authorization", bearer(token))
                .header("Content-Type", format!("multipart/related; boundary={boundary}"))
                .send(&body[..])
        })?;
        if status == 404 {
            return Err(start_over(m)); // the folder it goes in is gone
        }
        let id = id_of(&ok(P, (status, v))?)?;
        m.files.insert(f.rel.clone(), Uploaded { hash, id });
    }
    Ok(())
}
