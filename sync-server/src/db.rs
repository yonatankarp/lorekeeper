//! SQLite storage. One connection behind a mutex; every function here runs inside it.

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use sync_protocol::{InviteInfo, MemberInfo, Role};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS rooms (
    room TEXT PRIMARY KEY,
    seq INTEGER NOT NULL DEFAULT 0,
    bytes INTEGER NOT NULL DEFAULT 0,   -- sum of live blob sizes
    files INTEGER NOT NULL DEFAULT 0,   -- live files (tombstones don't count)
    created INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS tokens (
    token_hash BLOB PRIMARY KEY,        -- SHA-256 of the token
    room TEXT NOT NULL,
    member_id TEXT NOT NULL,
    role TEXT NOT NULL,                 -- 'owner' or 'member'
    member TEXT NOT NULL DEFAULT '',    -- opaque display id, encrypted by clients
    created INTEGER NOT NULL,
    last_seen INTEGER
);
CREATE INDEX IF NOT EXISTS tokens_room ON tokens (room);
CREATE TABLE IF NOT EXISTS invites (
    invite TEXT PRIMARY KEY,
    room TEXT NOT NULL,
    created INTEGER NOT NULL,
    expires INTEGER NOT NULL,
    used INTEGER
);
CREATE INDEX IF NOT EXISTS invites_room ON invites (room);
CREATE TABLE IF NOT EXISTS files (
    room TEXT NOT NULL,
    id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    blob BLOB,                          -- NULL: deleted (tombstone)
    PRIMARY KEY (room, id)
);
CREATE INDEX IF NOT EXISTS files_seq ON files (room, seq);
";

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let c = Connection::open(path)?;
    // WAL and SHM files live next to the database; temp tables stay in memory, so /data is the
    // only place the server writes.
    c.query_row("PRAGMA journal_mode = WAL", [], |r| r.get::<_, String>(0))?;
    // journal_size_limit: the WAL shrinks back after a checkpoint instead of keeping the size of
    // the biggest write it ever held.
    c.execute_batch("PRAGMA synchronous = NORMAL; PRAGMA temp_store = MEMORY; PRAGMA journal_size_limit = 67108864;")?;
    c.execute_batch(SCHEMA)?;
    Ok(c)
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn role_str(role: Role) -> &'static str {
    match role {
        Role::Owner => "owner",
        Role::Member => "member",
    }
}

fn parse_role(s: &str) -> Role {
    if s == "owner" {
        Role::Owner
    } else {
        Role::Member
    }
}

pub fn create_room(c: &mut Connection, room: &str, owner_hash: &[u8; 32], owner_id: &str) -> rusqlite::Result<()> {
    let tx = c.transaction()?;
    let now = now_ms();
    tx.execute("INSERT INTO rooms (room, created) VALUES (?1, ?2)", params![room, now])?;
    insert_token(&tx, room, owner_hash, owner_id, Role::Owner, "", now)?;
    tx.commit()
}

fn insert_token(
    c: &Connection,
    room: &str,
    hash: &[u8; 32],
    member_id: &str,
    role: Role,
    member: &str,
    now: i64,
) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO tokens (token_hash, room, member_id, role, member, created) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![&hash[..], room, member_id, role_str(role), member, now],
    )?;
    Ok(())
}

/// Every token of the room as (hash, member_id, role), for a constant-time scan.
pub fn room_tokens(c: &Connection, room: &str) -> rusqlite::Result<Vec<(Vec<u8>, String, Role)>> {
    let mut st = c.prepare_cached("SELECT token_hash, member_id, role FROM tokens WHERE room = ?1")?;
    let rows = st.query_map([room], |r| Ok((r.get(0)?, r.get(1)?, parse_role(&r.get::<_, String>(2)?))))?;
    rows.collect()
}

/// Bumps `last_seen` (and the display id when given); false when the member is gone (revoked).
pub fn touch_member(c: &Connection, room: &str, member_id: &str, member: Option<&str>) -> rusqlite::Result<bool> {
    let n = match member {
        Some(m) => c.execute(
            "UPDATE tokens SET member = ?3, last_seen = ?4 WHERE room = ?1 AND member_id = ?2",
            params![room, member_id, m, now_ms()],
        )?,
        None => c.execute(
            "UPDATE tokens SET last_seen = ?3 WHERE room = ?1 AND member_id = ?2",
            params![room, member_id, now_ms()],
        )?,
    };
    Ok(n > 0)
}

pub fn members(c: &Connection, room: &str) -> rusqlite::Result<Vec<MemberInfo>> {
    let mut st = c.prepare_cached(
        "SELECT member_id, member, role, created, last_seen FROM tokens WHERE room = ?1 ORDER BY created, member_id",
    )?;
    let rows = st.query_map([room], |r| {
        Ok(MemberInfo {
            member_id: r.get(0)?,
            member: r.get(1)?,
            role: parse_role(&r.get::<_, String>(2)?),
            created: r.get(3)?,
            last_seen: r.get(4)?,
        })
    })?;
    rows.collect()
}

pub enum Removed {
    Yes,
    Owner,
    NotFound,
}

pub fn remove_member(c: &Connection, room: &str, member_id: &str) -> rusqlite::Result<Removed> {
    let role: Option<String> = c
        .query_row("SELECT role FROM tokens WHERE room = ?1 AND member_id = ?2", [room, member_id], |r| r.get(0))
        .optional()?;
    Ok(match role.as_deref() {
        None => Removed::NotFound,
        Some("owner") => Removed::Owner,
        Some(_) => {
            c.execute("DELETE FROM tokens WHERE room = ?1 AND member_id = ?2", [room, member_id])?;
            Removed::Yes
        }
    })
}

/// Members plus pending invites, for the per-room cap.
pub fn seats(c: &Connection, room: &str) -> rusqlite::Result<u64> {
    c.query_row(
        "SELECT (SELECT COUNT(*) FROM tokens WHERE room = ?1)
              + (SELECT COUNT(*) FROM invites WHERE room = ?1 AND used IS NULL AND expires > ?2)",
        params![room, now_ms()],
        |r| r.get(0),
    )
}

/// Adds an invite, dropping the room's expired ones (used ones stay until they expire, so a second
/// redeem still learns `invite_used`).
pub fn create_invite(c: &Connection, room: &str, invite: &str, expires: i64) -> rusqlite::Result<()> {
    c.execute("DELETE FROM invites WHERE room = ?1 AND expires <= ?2", params![room, now_ms()])?;
    c.execute(
        "INSERT INTO invites (invite, room, created, expires) VALUES (?1, ?2, ?3, ?4)",
        params![invite, room, now_ms(), expires],
    )?;
    Ok(())
}

pub fn pending_invites(c: &Connection, room: &str) -> rusqlite::Result<Vec<InviteInfo>> {
    let mut st = c.prepare_cached(
        "SELECT invite, expires FROM invites WHERE room = ?1 AND used IS NULL AND expires > ?2 ORDER BY created, invite",
    )?;
    let rows = st.query_map(params![room, now_ms()], |r| Ok(InviteInfo { invite: r.get(0)?, expires: r.get(1)? }))?;
    rows.collect()
}

/// Deletes a pending invite; false when there was none.
pub fn revoke_invite(c: &Connection, room: &str, invite: &str) -> rusqlite::Result<bool> {
    let n = c.execute("DELETE FROM invites WHERE room = ?1 AND invite = ?2 AND used IS NULL", [room, invite])?;
    Ok(n > 0)
}

pub enum Redeemed {
    Yes,
    NotFound,
    Used,
    Expired,
}

pub fn redeem(
    c: &mut Connection,
    room: &str,
    invite: &str,
    hash: &[u8; 32],
    member_id: &str,
    member: &str,
) -> rusqlite::Result<Redeemed> {
    let tx = c.transaction()?;
    let row: Option<(i64, Option<i64>)> = tx
        .query_row("SELECT expires, used FROM invites WHERE room = ?1 AND invite = ?2", [room, invite], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?;
    let now = now_ms();
    let result = match row {
        None => Redeemed::NotFound,
        Some((_, Some(_))) => Redeemed::Used,
        Some((expires, None)) if expires <= now => Redeemed::Expired,
        Some(_) => {
            // Conditional, so the invite is used once even if two redeems ever overlap.
            let n = tx.execute(
                "UPDATE invites SET used = ?3 WHERE room = ?1 AND invite = ?2 AND used IS NULL",
                params![room, invite, now],
            )?;
            if n == 1 {
                insert_token(&tx, room, hash, member_id, Role::Member, member, now)?;
                Redeemed::Yes
            } else {
                Redeemed::Used
            }
        }
    };
    tx.commit()?;
    Ok(result)
}

pub struct Limits {
    pub max_room_bytes: u64,
    pub max_files: u64,
}

pub enum Written {
    Ack(u64),
    Conflict(u64, Option<Vec<u8>>),
    RoomFull,
    TooManyFiles,
}

/// Puts (`Some(blob)`) or deletes (`None`) a file when `base` matches its current `seq`. A missing
/// file or a tombstone also matches `base` 0. Deleting a missing or deleted file changes nothing.
pub fn write(
    c: &mut Connection,
    room: &str,
    id: &str,
    base: u64,
    blob: Option<&[u8]>,
    limits: &Limits,
) -> rusqlite::Result<Written> {
    let tx = c.transaction()?;
    let current: Option<(u64, Option<Vec<u8>>)> = tx
        .query_row("SELECT seq, blob FROM files WHERE room = ?1 AND id = ?2", [room, id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?;
    let new_id = current.is_none();
    let (cur_seq, cur_blob) = current.unwrap_or((0, None));
    if base != cur_seq && !(base == 0 && cur_blob.is_none()) {
        return Ok(Written::Conflict(cur_seq, cur_blob));
    }
    if blob.is_none() && cur_blob.is_none() {
        return Ok(Written::Ack(cur_seq));
    }
    let (bytes, files): (u64, u64) =
        tx.query_row("SELECT bytes, files FROM rooms WHERE room = ?1", [room], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let old_len = cur_blob.as_ref().map_or(0, |b| b.len() as u64);
    let new_len = blob.map_or(0, |b| b.len() as u64);
    let new_bytes = bytes - old_len.min(bytes) + new_len;
    let new_files = files - u64::from(cur_blob.is_some()).min(files) + u64::from(blob.is_some());
    // Only growth is refused, so a room over a lowered limit can still shrink.
    if new_len > old_len && new_bytes > limits.max_room_bytes {
        return Ok(Written::RoomFull);
    }
    if new_files > files && new_files > limits.max_files {
        return Ok(Written::TooManyFiles);
    }
    // Tombstones don't count as files but are rows forever; a new id may add at most as many
    // tombstones again as there can be files.
    if new_id {
        let rows: u64 = tx.query_row("SELECT COUNT(*) FROM files WHERE room = ?1", [room], |r| r.get(0))?;
        if rows >= limits.max_files.saturating_mul(2) {
            return Ok(Written::TooManyFiles);
        }
    }
    let seq: u64 = tx.query_row(
        "UPDATE rooms SET seq = seq + 1, bytes = ?2, files = ?3 WHERE room = ?1 RETURNING seq",
        params![room, new_bytes, new_files],
        |r| r.get(0),
    )?;
    tx.execute(
        "INSERT INTO files (room, id, seq, blob) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (room, id) DO UPDATE SET seq = excluded.seq, blob = excluded.blob",
        params![room, id, seq, blob],
    )?;
    tx.commit()?;
    Ok(Written::Ack(seq))
}

/// Files changed after `since`, oldest first: at most 500, and at most `max_bytes` of blobs
/// (always at least one). Returns (room seq, changes, more).
#[allow(clippy::type_complexity)]
pub fn changes(
    c: &Connection,
    room: &str,
    since: u64,
    max_bytes: usize,
) -> rusqlite::Result<(u64, Vec<(String, u64, Option<Vec<u8>>)>, bool)> {
    let seq: u64 = c.query_row("SELECT seq FROM rooms WHERE room = ?1", [room], |r| r.get(0))?;
    let mut st = c.prepare_cached("SELECT id, seq, blob FROM files WHERE room = ?1 AND seq > ?2 ORDER BY seq LIMIT 501")?;
    let mut rows = st.query(params![room, since])?;
    let (mut out, mut bytes, mut more) = (Vec::new(), 0usize, false);
    while let Some(r) = rows.next()? {
        let blob: Option<Vec<u8>> = r.get(2)?;
        let len = blob.as_ref().map_or(0, Vec::len);
        if out.len() == 500 || (!out.is_empty() && bytes + len > max_bytes) {
            more = true;
            break;
        }
        bytes += len;
        out.push((r.get(0)?, r.get(1)?, blob));
    }
    Ok((seq, out, more))
}
