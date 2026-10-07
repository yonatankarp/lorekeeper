//! SQLite storage. One connection behind a mutex; every function here runs inside it, so each one sees and changes a
//! consistent room (a write and a read of who may see it never interleave).

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use sync_protocol::{InviteInfo, MemberInfo, MemberUpdate, PresenceMember, Role};

/// The schema as first shipped; [`migrate`] brings it up to date. `PRAGMA user_version` counts the migrations run.
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS rooms (
    room TEXT PRIMARY KEY,
    seq INTEGER NOT NULL DEFAULT 0,
    bytes INTEGER NOT NULL DEFAULT 0,   -- sum of live blob sizes, private ones included
    files INTEGER NOT NULL DEFAULT 0,   -- live files (tombstones don't count)
    created INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS tokens (
    token_hash BLOB PRIMARY KEY,        -- SHA-256 of the token
    room TEXT NOT NULL,
    member_id TEXT NOT NULL,
    role TEXT NOT NULL,                 -- 'owner', 'dm' or 'player' ('member' before roles)
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

/// Roles, private spaces and re-invites. Existing members become players; every existing file is shared.
const MIGRATION_1: &str = "
ALTER TABLE rooms ADD COLUMN dm_reads_private INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tokens ADD COLUMN manage INTEGER NOT NULL DEFAULT 0;
UPDATE tokens SET role = 'player' WHERE role = 'member';
ALTER TABLE invites ADD COLUMN role TEXT NOT NULL DEFAULT 'player';
ALTER TABLE invites ADD COLUMN manage INTEGER NOT NULL DEFAULT 0;
ALTER TABLE invites ADD COLUMN member_id TEXT;        -- a re-invite: the member it hands over
CREATE TABLE files_v1 (
    room TEXT NOT NULL,
    owner TEXT NOT NULL DEFAULT '',     -- '': shared; else the member_id whose private space it is in
    id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    blob BLOB,                          -- NULL: deleted (tombstone)
    PRIMARY KEY (room, owner, id)
);
INSERT INTO files_v1 (room, owner, id, seq, blob) SELECT room, '', id, seq, blob FROM files;
DROP TABLE files;
ALTER TABLE files_v1 RENAME TO files;
CREATE INDEX files_seq ON files (room, seq);
";

/// The owner's "I'm also the DM" (only they set it, on their own row); every existing owner starts without it.
const MIGRATION_2: &str = "ALTER TABLE tokens ADD COLUMN owner_is_dm INTEGER NOT NULL DEFAULT 0;";

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let mut c = Connection::open(path)?;
    // WAL and SHM files live next to the database; temp tables stay in memory, so /data is the
    // only place the server writes.
    c.query_row("PRAGMA journal_mode = WAL", [], |r| r.get::<_, String>(0))?;
    // journal_size_limit: the WAL shrinks back after a checkpoint instead of keeping the size of
    // the biggest write it ever held.
    c.execute_batch("PRAGMA synchronous = NORMAL; PRAGMA temp_store = MEMORY; PRAGMA journal_size_limit = 67108864;")?;
    c.execute_batch(SCHEMA)?;
    migrate(&mut c)?;
    Ok(c)
}

/// Runs the migrations a database hasn't had yet, each in one transaction with its version bump.
fn migrate(c: &mut Connection) -> rusqlite::Result<()> {
    let version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version < 1 {
        let tx = c.transaction()?;
        tx.execute_batch(MIGRATION_1)?;
        tx.execute_batch("PRAGMA user_version = 1")?;
        tx.commit()?;
    }
    if version < 2 {
        let tx = c.transaction()?;
        tx.execute_batch(MIGRATION_2)?;
        tx.execute_batch("PRAGMA user_version = 2")?;
        tx.commit()?;
    }
    Ok(())
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn role_str(role: Role) -> &'static str {
    match role {
        Role::Owner => "owner",
        Role::Dm => "dm",
        Role::Player => "player",
    }
}

/// Anything unknown is a player, the role that reads the least.
fn parse_role(s: &str) -> Role {
    match s {
        "owner" => Role::Owner,
        "dm" => Role::Dm,
        _ => Role::Player,
    }
}

pub fn create_room(c: &mut Connection, room: &str, owner_hash: &[u8; 32], owner_id: &str) -> rusqlite::Result<()> {
    let tx = c.transaction()?;
    let now = now_ms();
    tx.execute("INSERT INTO rooms (room, created) VALUES (?1, ?2)", params![room, now])?;
    let owner = Member { role: Role::Owner, manage: false, owner_is_dm: false, member: String::new(), created: now };
    insert_token(&tx, room, owner_hash, owner_id, &owner)?;
    tx.commit()
}

/// A member as their token row holds them.
pub struct Member {
    pub role: Role,
    pub manage: bool,
    /// The owner said they're also the DM (false for anyone else).
    pub owner_is_dm: bool,
    pub member: String,
    pub created: i64,
}

fn insert_token(c: &Connection, room: &str, hash: &[u8; 32], member_id: &str, m: &Member) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO tokens (token_hash, room, member_id, role, manage, owner_is_dm, member, created) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![&hash[..], room, member_id, role_str(m.role), m.manage, m.owner_is_dm, m.member, m.created],
    )?;
    Ok(())
}

/// A token row: (hash, member_id, role, manage).
pub type TokenRow = (Vec<u8>, String, Role, bool);

/// Every token of the room, for a constant-time scan.
pub fn room_tokens(c: &Connection, room: &str) -> rusqlite::Result<Vec<TokenRow>> {
    let mut st = c.prepare_cached("SELECT token_hash, member_id, role, manage FROM tokens WHERE room = ?1")?;
    let rows = st.query_map([room], |r| Ok((r.get(0)?, r.get(1)?, parse_role(&r.get::<_, String>(2)?), r.get(3)?)))?;
    rows.collect()
}

/// A member's role and manage flag; None when they aren't in the room.
pub fn member(c: &Connection, room: &str, member_id: &str) -> rusqlite::Result<Option<Member>> {
    c.query_row(
        "SELECT role, manage, owner_is_dm, member, created FROM tokens WHERE room = ?1 AND member_id = ?2 LIMIT 1",
        [room, member_id],
        |r| {
            let role = parse_role(&r.get::<_, String>(0)?);
            Ok(Member { role, manage: r.get(1)?, owner_is_dm: role == Role::Owner && r.get(2)?, member: r.get(3)?, created: r.get(4)? })
        },
    )
    .optional()
}

/// Bumps `last_seen` for the token `hash`; false when that token is gone (its member removed, or re-invited onto
/// another computer: the member id lives on, the token doesn't).
pub fn touch_token(c: &Connection, room: &str, hash: &[u8; 32]) -> rusqlite::Result<bool> {
    let n = c.execute("UPDATE tokens SET last_seen = ?3 WHERE room = ?1 AND token_hash = ?2", params![room, &hash[..], now_ms()])?;
    Ok(n > 0)
}

/// Sets the display id from `hello` and bumps `last_seen`.
pub fn touch_member(c: &Connection, room: &str, member_id: &str, member: &str) -> rusqlite::Result<()> {
    c.execute(
        "UPDATE tokens SET member = ?3, last_seen = ?4 WHERE room = ?1 AND member_id = ?2",
        params![room, member_id, member, now_ms()],
    )?;
    Ok(())
}

pub fn members(c: &Connection, room: &str) -> rusqlite::Result<Vec<MemberInfo>> {
    let mut st = c.prepare_cached(
        "SELECT member_id, member, role, manage, owner_is_dm, created, last_seen FROM tokens WHERE room = ?1 ORDER BY created, member_id",
    )?;
    let rows = st.query_map([room], |r| {
        let role = parse_role(&r.get::<_, String>(2)?);
        Ok(MemberInfo {
            member_id: r.get(0)?,
            member: r.get(1)?,
            role,
            manage: r.get(3)?,
            owner_is_dm: role == Role::Owner && r.get::<_, bool>(4)?,
            created: r.get(5)?,
            last_seen: r.get(6)?,
        })
    })?;
    rows.collect()
}

/// What a connection is told about itself (the `access` message).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Access {
    pub role: Role,
    pub manage: bool,
    pub owner_is_dm: bool,
    pub dm_reads_private: bool,
    /// The room's members, only when this member reads others' private notes.
    pub members: Vec<PresenceMember>,
}

/// A member's access, read now (never cached); None when they aren't in the room.
pub fn access(c: &Connection, room: &str, member_id: &str) -> rusqlite::Result<Option<Access>> {
    let Some(m) = member(c, room, member_id)? else { return Ok(None) };
    let Some(dm_reads_private) = dm_reads_private(c, room)? else { return Ok(None) };
    let members = if m.role.reads_private(m.owner_is_dm, dm_reads_private) {
        members(c, room)?.into_iter().map(|i| PresenceMember { member_id: i.member_id, member: i.member, role: i.role }).collect()
    } else {
        Vec::new()
    };
    Ok(Some(Access { role: m.role, manage: m.manage, owner_is_dm: m.owner_is_dm, dm_reads_private, members }))
}

/// None when the room is gone (deleted by its owner while a socket was reading).
fn dm_reads_private(c: &Connection, room: &str) -> rusqlite::Result<Option<bool>> {
    c.query_row("SELECT dm_reads_private FROM rooms WHERE room = ?1", [room], |r| r.get(0)).optional()
}

pub fn set_dm_reads_private(c: &Connection, room: &str, on: bool) -> rusqlite::Result<()> {
    c.execute("UPDATE rooms SET dm_reads_private = ?2 WHERE room = ?1", params![room, on])?;
    Ok(())
}

pub enum Updated {
    Yes,
    Owner,
    NotFound,
    /// Asked for a second owner, manage for a player, or "also the DM" for anyone but the owner.
    Invalid,
}

/// The owner's request (only the owner may call it): a member's role and manage flag (a player never manages), or on
/// the owner's own row only `owner_is_dm`, their "I'm also the DM".
pub fn update_member(c: &Connection, room: &str, member_id: &str, req: &MemberUpdate) -> rusqlite::Result<Updated> {
    let Some(m) = member(c, room, member_id)? else { return Ok(Updated::NotFound) };
    if m.role == Role::Owner {
        if req.role.is_some() || req.manage.is_some() {
            return Ok(Updated::Owner);
        }
        if let Some(on) = req.owner_is_dm {
            c.execute("UPDATE tokens SET owner_is_dm = ?3 WHERE room = ?1 AND member_id = ?2", params![room, member_id, on])?;
        }
        return Ok(Updated::Yes);
    }
    if req.owner_is_dm.is_some() {
        return Ok(Updated::Invalid);
    }
    let (role, manage) = (req.role, req.manage);
    let role = role.unwrap_or(m.role);
    if role == Role::Owner || (role == Role::Player && manage == Some(true)) {
        return Ok(Updated::Invalid);
    }
    let manage = role == Role::Dm && manage.unwrap_or(m.manage);
    c.execute(
        "UPDATE tokens SET role = ?3, manage = ?4 WHERE room = ?1 AND member_id = ?2",
        params![room, member_id, role_str(role), manage],
    )?;
    Ok(Updated::Yes)
}

/// Removes a member (never the owner): their token stops working, their pending re-invites go, and so does their
/// whole private space (rows, not tombstones: nobody else may learn of them), with the room's size and file counts.
/// The caller checks who may remove whom.
pub fn remove_member(c: &mut Connection, room: &str, member_id: &str) -> rusqlite::Result<()> {
    let tx = c.transaction()?;
    let (bytes, files): (u64, u64) = tx.query_row(
        "SELECT COALESCE(SUM(LENGTH(blob)), 0), COUNT(blob) FROM files WHERE room = ?1 AND owner = ?2",
        [room, member_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    tx.execute("DELETE FROM files WHERE room = ?1 AND owner = ?2", [room, member_id])?;
    tx.execute(
        "UPDATE rooms SET bytes = MAX(bytes - ?2, 0), files = MAX(files - ?3, 0) WHERE room = ?1",
        params![room, bytes, files],
    )?;
    tx.execute("DELETE FROM tokens WHERE room = ?1 AND member_id = ?2", [room, member_id])?;
    tx.execute("DELETE FROM invites WHERE room = ?1 AND member_id = ?2", [room, member_id])?;
    tx.commit()
}

/// Deletes a room and everything in it (shared and private files, members and their tokens, invites) in one
/// transaction: its tokens stop working at once and its bytes and files no longer count anywhere. The caller checks
/// that the owner asked.
pub fn delete_room(c: &mut Connection, room: &str) -> rusqlite::Result<()> {
    let tx = c.transaction()?;
    for table in ["files", "tokens", "invites", "rooms"] {
        tx.execute(&format!("DELETE FROM {table} WHERE room = ?1"), [room])?;
    }
    tx.commit()
}

/// Members plus pending invites for new members (re-invites hand over a seat, they don't take one), for the cap.
pub fn seats(c: &Connection, room: &str) -> rusqlite::Result<u64> {
    c.query_row(
        "SELECT (SELECT COUNT(DISTINCT member_id) FROM tokens WHERE room = ?1)
              + (SELECT COUNT(*) FROM invites WHERE room = ?1 AND used IS NULL AND expires > ?2 AND member_id IS NULL)",
        params![room, now_ms()],
        |r| r.get(0),
    )
}

/// Adds an invite, dropping the room's expired ones (used ones stay until they expire, so a second
/// redeem still learns `invite_used`).
/// Stores an invite; false when the room is gone (its owner deleted it after the caller's token was checked), so no
/// invite outlives its room.
pub fn create_invite(c: &Connection, room: &str, info: &InviteInfo) -> rusqlite::Result<bool> {
    c.execute("DELETE FROM invites WHERE room = ?1 AND expires <= ?2", params![room, now_ms()])?;
    let n = c.execute(
        "INSERT INTO invites (invite, room, created, expires, role, manage, member_id)
         SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7 WHERE EXISTS (SELECT 1 FROM rooms WHERE room = ?2)",
        params![info.invite, room, now_ms(), info.expires, role_str(info.role), info.manage, info.member_id],
    )?;
    Ok(n == 1)
}

pub fn pending_invites(c: &Connection, room: &str) -> rusqlite::Result<Vec<InviteInfo>> {
    let mut st = c.prepare_cached(
        "SELECT invite, expires, role, manage, member_id FROM invites
         WHERE room = ?1 AND used IS NULL AND expires > ?2 ORDER BY created, invite",
    )?;
    let rows = st.query_map(params![room, now_ms()], |r| {
        Ok(InviteInfo {
            invite: r.get(0)?,
            expires: r.get(1)?,
            role: parse_role(&r.get::<_, String>(2)?),
            manage: r.get(3)?,
            member_id: r.get(4)?,
        })
    })?;
    rows.collect()
}

/// A pending invite's role and target member, for checking who may cancel it.
pub fn invite(c: &Connection, room: &str, invite: &str) -> rusqlite::Result<Option<(Role, bool, Option<String>)>> {
    c.query_row(
        "SELECT role, manage, member_id FROM invites WHERE room = ?1 AND invite = ?2 AND used IS NULL",
        [room, invite],
        |r| Ok((parse_role(&r.get::<_, String>(0)?), r.get(1)?, r.get(2)?)),
    )
    .optional()
}

/// Deletes a pending invite; false when there was none.
pub fn revoke_invite(c: &Connection, room: &str, invite: &str) -> rusqlite::Result<bool> {
    let n = c.execute("DELETE FROM invites WHERE room = ?1 AND invite = ?2 AND used IS NULL", [room, invite])?;
    Ok(n > 0)
}

pub enum Redeemed {
    /// A new member.
    Joined,
    /// A re-invite: this member's old tokens are revoked (their sockets are to be closed).
    Replaced(String),
    NotFound,
    Used,
    Expired,
}

/// Redeems an invite for the token `hash`. A new member gets `member_id` and the invite's role; a re-invite gives the
/// token the existing member's identity (id, role, manage, display id unless a new one is given, private space) and
/// revokes their other tokens.
pub fn redeem(
    c: &mut Connection,
    room: &str,
    invite: &str,
    hash: &[u8; 32],
    member_id: &str,
    member: &str,
) -> rusqlite::Result<Redeemed> {
    let tx = c.transaction()?;
    #[allow(clippy::type_complexity)]
    let row: Option<(i64, Option<i64>, String, bool, Option<String>)> = tx
        .query_row(
            // Only while the room is there: a token for a deleted room would be a member of nothing.
            "SELECT expires, used, role, manage, member_id FROM invites
             WHERE room = ?1 AND invite = ?2 AND EXISTS (SELECT 1 FROM rooms WHERE room = ?1)",
            [room, invite],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let now = now_ms();
    let result = match row {
        None => Redeemed::NotFound,
        Some((_, Some(_), ..)) => Redeemed::Used,
        Some((expires, None, ..)) if expires <= now => Redeemed::Expired,
        Some((_, None, role, manage, target)) => {
            // The member a re-invite hands over must still be there (removing them also drops it).
            // It hands over the member as they were when it was made: one whose role or manage changed since (made a
            // manager after a manager re-invited them) needs a new re-invite from someone allowed to hand them over.
            let old = match &target {
                Some(id) => match self::member(&tx, room, id)? {
                    Some(m) if m.role != Role::Owner && m.role == parse_role(&role) && m.manage == manage => Some((id.clone(), m)),
                    _ => return Ok(Redeemed::NotFound),
                },
                None => None,
            };
            // Conditional, so the invite is used once even if two redeems ever overlap.
            let n = tx.execute(
                "UPDATE invites SET used = ?3 WHERE room = ?1 AND invite = ?2 AND used IS NULL",
                params![room, invite, now],
            )?;
            if n != 1 {
                Redeemed::Used
            } else if let Some((id, m)) = old {
                tx.execute("DELETE FROM tokens WHERE room = ?1 AND member_id = ?2", [room, &id])?;
                let member = if member.is_empty() { m.member.clone() } else { member.to_string() };
                insert_token(&tx, room, hash, &id, &Member { member, ..m })?;
                Redeemed::Replaced(id)
            } else {
                let role = parse_role(&role);
                let m = Member { role, manage: manage && role == Role::Dm, owner_is_dm: false, member: member.to_string(), created: now };
                insert_token(&tx, room, hash, member_id, &m)?;
                Redeemed::Joined
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
    /// The writer isn't a member anymore (removed while the write was on its way).
    Gone,
}

/// Puts (`Some(blob)`) or deletes (`None`) a file of the shared space (`owner` "") or of member `owner`'s private
/// space, when `base` matches its current `seq`. A missing file or a tombstone also matches `base` 0. Deleting a missing
/// or deleted file changes nothing. A conflict answers with the current version of that same (owner, id), so it can
/// only ever hold the writer's own private file.
#[allow(clippy::too_many_arguments)]
pub fn write(
    c: &mut Connection,
    room: &str,
    writer: &str,
    owner: &str,
    id: &str,
    base: u64,
    blob: Option<&[u8]>,
    limits: &Limits,
) -> rusqlite::Result<Written> {
    let tx = c.transaction()?;
    // Checked in the same transaction as a removal's delete of the private space, so a write racing it can't
    // leave rows behind for a member who's gone.
    if member(&tx, room, writer)?.is_none() {
        return Ok(Written::Gone);
    }
    let current: Option<(u64, Option<Vec<u8>>)> = tx
        .query_row("SELECT seq, blob FROM files WHERE room = ?1 AND owner = ?2 AND id = ?3", [room, owner, id], |r| {
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
        "INSERT INTO files (room, owner, id, seq, blob) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (room, owner, id) DO UPDATE SET seq = excluded.seq, blob = excluded.blob",
        params![room, owner, id, seq, blob],
    )?;
    tx.commit()?;
    Ok(Written::Ack(seq))
}

/// Which files a viewer may see (see [`changes`]): `?1` room, `?2` viewer, `?3` the lower of `?4` and `?6`, `?4` since,
/// `?5` whether the viewer reads others' private files, `?6` dm_since. One predicate, so [`count_changes`] counts exactly
/// what a replay sends (a count of anything else would tell a player about others' private notes).
macro_rules! visible {
    () => {
        "room = ?1 AND seq > ?3
           AND (((owner = '' OR owner = ?2) AND seq > ?4) OR (?5 AND owner <> '' AND owner <> ?2 AND seq > ?6))"
    };
}

/// One changed file: (id, seq, blob, author) with author "" for a shared file.
pub type Row = (String, u64, Option<Vec<u8>>, String);

/// The files member `viewer` may see that changed after `since`, oldest first: at most 500, and at most `max_bytes` of
/// blobs (always at least one). Returns (room seq, changes, more).
///
/// The one place visibility is decided, for the replay, the live stream and `GET /changes` alike: shared files, the
/// viewer's own private files, and other members' private files only when the viewer's role reads them right now
/// (`Role::reads_private` with the owner's flag and the room's setting, all read here, never cached), and then after `dm_since`. A viewer
/// who isn't a member sees nothing.
pub fn changes(
    c: &Connection,
    room: &str,
    viewer: &str,
    since: u64,
    dm_since: u64,
    max_bytes: usize,
) -> rusqlite::Result<(u64, Vec<Row>, bool)> {
    // A room deleted meanwhile has no row: nothing to see, not a database error.
    let Some(seq) = c.query_row("SELECT seq FROM rooms WHERE room = ?1", [room], |r| r.get(0)).optional()? else {
        return Ok((0, Vec::new(), false));
    };
    let Some(m) = member(c, room, viewer)? else { return Ok((seq, Vec::new(), false)) };
    let reads = m.role.reads_private(m.owner_is_dm, dm_reads_private(c, room)?.unwrap_or(false));
    // Only a reader's dm_since means anything; anyone else's would only make the scan start further back.
    let dm_since = if reads { dm_since } else { since };
    let mut st = c.prepare_cached(concat!("SELECT id, seq, blob, owner FROM files WHERE ", visible!(), " ORDER BY seq LIMIT 501"))?;
    let mut rows = st.query(params![room, viewer, since.min(dm_since), since, reads, dm_since])?;
    let (mut out, mut bytes, mut more) = (Vec::new(), 0usize, false);
    while let Some(r) = rows.next()? {
        let blob: Option<Vec<u8>> = r.get(2)?;
        let len = blob.as_ref().map_or(0, Vec::len);
        if out.len() == 500 || (!out.is_empty() && bytes + len > max_bytes) {
            more = true;
            break;
        }
        bytes += len;
        out.push((r.get(0)?, r.get(1)?, blob, r.get(3)?));
    }
    Ok((seq, out, more))
}

/// How many changes a replay from `since` (and `dm_since`) holds: the rows [`changes`] would page through.
pub fn count_changes(c: &Connection, room: &str, viewer: &str, since: u64, dm_since: u64) -> rusqlite::Result<u64> {
    let Some(m) = member(c, room, viewer)? else { return Ok(0) };
    let reads = m.role.reads_private(m.owner_is_dm, dm_reads_private(c, room)?.unwrap_or(false));
    let dm_since = if reads { dm_since } else { since };
    let mut st = c.prepare_cached(concat!("SELECT COUNT(*) FROM files WHERE ", visible!()))?;
    st.query_row(params![room, viewer, since.min(dm_since), since, reads, dm_since], |r| r.get(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A re-invite keeps the member id but not the token: the old token no longer counts as connected, which is what a
    /// socket upgrade racing the re-invite checks (it got past authentication just before).
    #[test]
    fn a_reinvite_revokes_the_old_token_not_just_the_id() {
        let mut c = Connection::open_in_memory().unwrap();
        c.execute_batch(SCHEMA).unwrap();
        migrate(&mut c).unwrap();
        create_room(&mut c, "r", &[1; 32], "o").unwrap();
        let invite = |c: &Connection, id: &str, target: Option<&str>| {
            let info = InviteInfo { invite: id.into(), expires: i64::MAX, role: Role::Player, manage: false, member_id: target.map(Into::into) };
            assert!(create_invite(c, "r", &info).unwrap());
        };
        invite(&c, "i1", None);
        assert!(matches!(redeem(&mut c, "r", "i1", &[2; 32], "m", "").unwrap(), Redeemed::Joined));
        assert!(touch_token(&c, "r", &[2; 32]).unwrap());
        invite(&c, "i2", Some("m"));
        assert!(matches!(redeem(&mut c, "r", "i2", &[3; 32], "unused", "").unwrap(), Redeemed::Replaced(id) if id == "m"));
        assert!(!touch_token(&c, "r", &[2; 32]).unwrap(), "the old computer's token is gone");
        assert!(touch_token(&c, "r", &[3; 32]).unwrap());
        assert!(member(&c, "r", "m").unwrap().is_some(), "while the member lives on");
    }

    /// Deleting a room leaves no row of it in any table, and no other room's.
    #[test]
    fn deleting_a_room_leaves_nothing_of_it() {
        let mut c = Connection::open_in_memory().unwrap();
        c.execute_batch(SCHEMA).unwrap();
        migrate(&mut c).unwrap();
        let limits = Limits { max_room_bytes: 1 << 20, max_files: 10 };
        for room in ["r", "keep"] {
            create_room(&mut c, room, &[room.len() as u8; 32], "o").unwrap();
            let info = InviteInfo { invite: format!("i{room}"), expires: i64::MAX, role: Role::Player, manage: false, member_id: None };
            assert!(create_invite(&c, room, &info).unwrap());
            redeem(&mut c, room, &info.invite, &[room.len() as u8 + 10; 32], "m", "").unwrap();
            assert!(matches!(write(&mut c, room, "o", "", "f", 0, Some(&[1; 40]), &limits).unwrap(), Written::Ack(_)));
            assert!(matches!(write(&mut c, room, "m", "m", "p", 0, Some(&[2; 40]), &limits).unwrap(), Written::Ack(_)));
        }
        delete_room(&mut c, "r").unwrap();
        // A manager's invite, checked before the delete and stored after it, isn't stored; one stored before can't be
        // redeemed into the deleted room (its rows went with it, and a redeem needs the room).
        let late = InviteInfo { invite: "late".into(), expires: i64::MAX, role: Role::Player, manage: false, member_id: None };
        assert!(!create_invite(&c, "r", &late).unwrap());
        c.execute("INSERT INTO invites (invite, room, created, expires) VALUES ('orphan', 'r', 0, ?1)", [i64::MAX]).unwrap();
        assert!(matches!(redeem(&mut c, "r", "orphan", &[9; 32], "x", "").unwrap(), Redeemed::NotFound));
        c.execute("DELETE FROM invites WHERE invite = 'orphan'", []).unwrap();
        let count = |table: &str, room: &str| -> i64 {
            c.query_row(&format!("SELECT COUNT(*) FROM {table} WHERE room = ?1"), [room], |r| r.get(0)).unwrap()
        };
        for table in ["files", "tokens", "invites", "rooms"] {
            assert_eq!(count(table, "r"), 0, "{table}");
            assert!(count(table, "keep") > 0, "{table}");
        }
        assert_eq!(changes(&c, "r", "m", 0, 0, 1 << 20).unwrap(), (0, Vec::new(), false), "a deleted room reads as empty, not an error");
        assert_eq!(access(&c, "r", "m").unwrap(), None);
    }

    /// A database from before roles and private notes keeps its rooms, members, invites and files: members become
    /// players, files are shared, and it works as a new one does.
    #[test]
    fn an_old_database_migrates() {
        let path = std::env::temp_dir().join(format!("lorekeeper-sync-migrate-{}.db", sync_protocol::random_id()));
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch(SCHEMA).unwrap();
            c.execute_batch(
                "INSERT INTO rooms (room, seq, bytes, files, created) VALUES ('r', 2, 7, 1, 1);
                 INSERT INTO tokens (token_hash, room, member_id, role, member, created) VALUES (x'01', 'r', 'o', 'owner', 'A', 1);
                 INSERT INTO tokens (token_hash, room, member_id, role, member, created) VALUES (x'02', 'r', 'm', 'member', 'B', 2);
                 INSERT INTO invites (invite, room, created, expires) VALUES ('i', 'r', 1, 99999999999999);
                 INSERT INTO files (room, id, seq, blob) VALUES ('r', 'f', 1, x'00112233445566');
                 INSERT INTO files (room, id, seq, blob) VALUES ('r', 'g', 2, NULL);",
            )
            .unwrap();
        }
        let mut c = open(&path).unwrap();
        let list = members(&c, "r").unwrap();
        assert_eq!(list.iter().map(|m| (m.member_id.as_str(), m.role, m.manage)).collect::<Vec<_>>(), [("o", Role::Owner, false), ("m", Role::Player, false)]);
        let invites = pending_invites(&c, "r").unwrap();
        assert_eq!((invites[0].role, invites[0].manage, invites[0].member_id.clone()), (Role::Player, false, None));
        let (seq, rows, _) = changes(&c, "r", "m", 0, 0, 1 << 20).unwrap();
        assert_eq!(seq, 2);
        assert_eq!(rows, [("f".to_string(), 1, Some(vec![0, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66]), String::new()), ("g".to_string(), 2, None, String::new())]);
        assert_eq!(dm_reads_private(&c, "r").unwrap(), Some(false));
        // Writing works on the new key, and opening again runs nothing twice.
        let limits = Limits { max_room_bytes: 1 << 20, max_files: 10 };
        assert!(matches!(write(&mut c, "r", "m", "m", "f", 0, Some(&[1; 40]), &limits).unwrap(), Written::Ack(3)));
        drop(c);
        let c = open(&path).unwrap();
        assert_eq!(c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(), 2);
        assert!(!member(&c, "r", "o").unwrap().unwrap().owner_is_dm, "an existing owner isn't the DM");
        assert_eq!(changes(&c, "r", "m", 0, 0, 1 << 20).unwrap().1.len(), 3);
        drop(c);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }
}
