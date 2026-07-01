//! Sessions: raw `sessions`-table rows (`SessionRow`) and the folded
//! `SessionInfo` projection (see `session_list`). Pure; the store owns SQL.

use ulid::Ulid;

/// One row of the `sessions` table, exactly as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    pub id: Ulid,
    pub name: String,
    pub root_path: String,
    pub created_ts: i64,
    pub last_touched_ts: i64,
}
