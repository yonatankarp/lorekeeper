//! Dropbox backup into the app's own folder (Apps/Lorekeeper). With "App folder" access every
//! path is relative to that folder, and Lorekeeper can't see the rest of the Dropbox.

use std::fmt::Write as _;

use serde_json::{json, Value};

use crate::{
    cloud::{self, bearer, call, ok, Manifest, Plan, Provider, Uploaded},
    github::AGENT,
};

const P: Provider = Provider::Dropbox;

/// An API call that takes and returns JSON (`null` for calls without arguments).
fn rpc(token: &str, endpoint: &str, body: &Value) -> Result<(u16, Value), String> {
    call(P, || AGENT.post(format!("https://api.dropboxapi.com/2/{endpoint}")).header("Authorization", bearer(token)).send_json(body))
}

pub fn account(token: &str) -> Result<String, String> {
    let v = ok(P, rpc(token, "users/get_current_account", &Value::Null)?)?;
    v["email"].as_str().or(v["name"]["display_name"].as_str()).map(String::from).ok_or_else(|| "Dropbox sent an unexpected reply.".into())
}

/// JSON for the Dropbox-API-Arg header, which must be ASCII: other characters become \uXXXX.
pub fn header_json(v: &Value) -> String {
    let mut out = String::new();
    for c in v.to_string().chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            for unit in c.encode_utf16(&mut [0; 2]) {
                let _ = write!(out, "\\u{unit:04x}");
            }
        }
    }
    out
}

/// Where a campaign's notes go: the app folder itself for the main campaign (""), else
/// Campaigns/<name> in it.
pub fn root(campaign: &str) -> String {
    if campaign.is_empty() { String::new() } else { format!("/Campaigns/{campaign}") }
}

/// `root` from root(): every path is `{root}/{rel}`.
pub fn push(token: &str, plan: &Plan, m: &mut Manifest, root: &str) -> Result<(), String> {
    // Deletions first: Dropbox ignores case, so a rename from "vex.md" to "Vex.md" must not delete the new upload.
    for rel in &plan.delete {
        let (status, v) = rpc(token, "files/delete_v2", &json!({ "path": format!("{root}/{rel}") }))?;
        let gone = status == 409 && v["error_summary"].as_str().is_some_and(|s| s.starts_with("path_lookup/not_found"));
        if !gone {
            ok(P, (status, v))?;
        }
        m.files.remove(rel);
    }
    for f in &plan.upload {
        let (bytes, hash) = cloud::read(f)?;
        let arg = header_json(&json!({ "path": format!("{root}/{}", f.rel), "mode": "overwrite", "mute": true }));
        ok(P, call(P, || {
            AGENT.post("https://content.dropboxapi.com/2/files/upload")
                .header("Authorization", bearer(token))
                .header("Dropbox-API-Arg", &arg)
                .header("Content-Type", "application/octet-stream")
                .send(&bytes[..])
        })?)?;
        m.files.insert(f.rel.clone(), Uploaded { hash, id: String::new() });
    }
    Ok(())
}
