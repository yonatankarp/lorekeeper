//! Google Drive backup into a "Lorekeeper" folder the app creates in My Drive, with a folder per
//! campaign in it. The drive.file permission only lets Lorekeeper see files it made. Drive has ids
//! instead of paths, so the manifest keeps the id of every folder and file.

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

/// The folder a path is in: "NPCs/Villains" for "NPCs/Villains/Vex.md", "" (the campaign's folder) at the top.
pub fn parent(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// The folders to create before `rel` can be uploaded, outermost first; "" is the campaign's folder.
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
/// campaign folder.
fn start_over(m: &mut Manifest) -> String {
    m.files.clear();
    m.folders.clear();
    "A Lorekeeper folder in Google Drive was removed. The next backup will upload all your notes again.".into()
}

/// The folder in My Drive that holds a folder per campaign.
pub const SHARED: &str = "Lorekeeper";

/// Every file matching a Drive query, page by page.
pub fn list(token: &str, query: &str, fields: &str) -> Result<Vec<Value>, String> {
    let (mut out, mut page) = (Vec::new(), String::new());
    loop {
        let mut params = vec![("q", query), ("fields", fields), ("pageSize", "1000")];
        if !page.is_empty() {
            params.push(("pageToken", page.as_str()));
        }
        let url = tauri::Url::parse_with_params(FILES, &params).expect("valid URL");
        let v = ok(P, call(P, || AGENT.get(url.as_str()).header("Authorization", bearer(token)).call())?)?;
        out.extend(v["files"].as_array().cloned().unwrap_or_default());
        match v["nextPageToken"].as_str() {
            Some(t) if !t.is_empty() => page = t.to_string(),
            _ => return Ok(out),
        }
    }
}

/// The `name` folders the app made in `parent` ("root" = My Drive, else a folder id), newest first.
pub fn folders(token: &str, name: &str, parent: &str) -> Result<Vec<Value>, String> {
    let name = name.replace('\\', "\\\\").replace('\'', "\\'"); // quoted for Drive's query language
    let q = format!("name = '{name}' and mimeType = '{FOLDER}' and '{parent}' in parents and trashed = false");
    let mut found = list(token, &q, "nextPageToken,files(id,createdTime)")?;
    found.sort_by(|a, b| b["createdTime"].as_str().cmp(&a["createdTime"].as_str()));
    Ok(found)
}

/// Creates a folder (in My Drive without a parent): (status, reply).
fn create_folder(token: &str, name: &str, parent: Option<&str>) -> Result<(u16, Value), String> {
    let mut meta = json!({ "name": name, "mimeType": FOLDER });
    if let Some(parent) = parent {
        meta["parents"] = json!([parent]);
    }
    call(P, || AGENT.post(format!("{FILES}?fields=id")).header("Authorization", bearer(token)).send_json(&meta))
}

/// The id of the Lorekeeper folder in My Drive: the oldest when there are several, else a new one.
fn shared_folder(token: &str) -> Result<String, String> {
    match folders(token, SHARED, "root")?.last() {
        Some(f) => id_of(f),
        None => id_of(&ok(P, create_folder(token, SHARED, None)?)?),
    }
}

/// Uploads a campaign's changes into its folder `campaign` (folder "") in the Lorekeeper folder.
pub fn push(token: &str, plan: &Plan, m: &mut Manifest, campaign: &str) -> Result<(), String> {
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
            let (status, v) = if dir.is_empty() {
                create_folder(token, campaign, Some(&shared_folder(token)?))?
            } else {
                create_folder(token, dir.rsplit('/').next().unwrap_or(&dir), Some(&m.folders[parent(&dir)]))?
            };
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
