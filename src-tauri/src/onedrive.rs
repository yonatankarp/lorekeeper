//! OneDrive backup into the app's own folder (Apps/Lorekeeper, Graph's special/approot), with the
//! Files.ReadWrite.AppFolder permission, so Lorekeeper can't see the rest of the OneDrive.

use crate::{
    cloud::{self, bearer, call, ok, Manifest, Plan, Provider, Uploaded},
    github::AGENT,
};

const P: Provider = Provider::Onedrive;
const GRAPH: &str = "https://graph.microsoft.com/v1.0";

pub fn account(token: &str) -> Result<String, String> {
    let v = ok(P, call(P, || AGENT.get(format!("{GRAPH}/me")).header("Authorization", bearer(token)).call())?)?;
    let name = |k: &str| v[k].as_str().filter(|s| !s.is_empty());
    name("mail").or(name("userPrincipalName")).or(name("displayName")).map(String::from).ok_or_else(|| "OneDrive sent an unexpected reply.".into())
}

/// "NPCs/Mirela & co.md" as a URL path: each name percent-encoded, slashes kept.
pub fn encode_path(rel: &str) -> String {
    let encode = |name: &str| {
        name.bytes()
            .map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
            .collect::<String>()
    };
    rel.split('/').map(encode).collect::<Vec<_>>().join("/")
}

pub fn push(token: &str, plan: &Plan, m: &mut Manifest) -> Result<(), String> {
    // Deletions first: OneDrive ignores case, so a rename from "vex.md" to "Vex.md" must not delete the new upload.
    // Deleted items go to the OneDrive recycle bin.
    for rel in &plan.delete {
        let url = format!("{GRAPH}/me/drive/special/approot:/{}", encode_path(rel));
        let reply = call(P, || AGENT.delete(&url).header("Authorization", bearer(token)).call())?;
        if reply.0 != 404 {
            ok(P, reply)?;
        }
        m.files.remove(rel);
    }
    for f in &plan.upload {
        let (bytes, hash) = cloud::read(f)?;
        // Creates missing folders and replaces an existing file.
        let url = format!("{GRAPH}/me/drive/special/approot:/{}:/content", encode_path(&f.rel));
        ok(P, call(P, || AGENT.put(&url).header("Authorization", bearer(token)).header("Content-Type", "application/octet-stream").send(&bytes[..]))?)?;
        m.files.insert(f.rel.clone(), Uploaded { hash, id: String::new() });
    }
    Ok(())
}
