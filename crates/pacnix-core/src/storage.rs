// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use rusqlite::{Connection, params};

use crate::model::InstalledPackage;

pub struct Storage {
    conn: Connection,
}

impl Storage {
    pub fn open(path: &str) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS packages (
                id             INTEGER PRIMARY KEY,
                canonical_name TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS installed_instances (
                id             INTEGER PRIMARY KEY,
                package_id     INTEGER NOT NULL REFERENCES packages(id),
                backend        TEXT NOT NULL,
                backend_ref    TEXT NOT NULL,
                version        TEXT,
                scope          TEXT,
                installed_at   INTEGER,
                last_seen_at   INTEGER NOT NULL,
                UNIQUE (backend, backend_ref)
             );
             CREATE TABLE IF NOT EXISTS aliases (
                query          TEXT PRIMARY KEY,
                backend        TEXT NOT NULL,
                backend_ref    TEXT NOT NULL
             );",
        )
        .map_err(|e| e.to_string())?;
        Ok(Self { conn })
    }

    pub fn remember_alias(&self, query: &str, backend: &str, backend_ref: &str) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO aliases (query, backend, backend_ref)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(query) DO UPDATE SET backend = ?2, backend_ref = ?3",
                params![query, backend, backend_ref],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    pub fn alias(&self, query: &str) -> Result<Option<(String, String)>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT backend, backend_ref FROM aliases WHERE query = ?1")
            .map_err(|e| e.to_string())?;
        let mut rows = stmt
            .query_map(params![query], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| e.to_string())?;
        match rows.next() {
            Some(Ok(v)) => Ok(Some(v)),
            Some(Err(e)) => Err(e.to_string()),
            None => Ok(None),
        }
    }

    pub fn upsert_instance(&self, pkg: &InstalledPackage) -> Result<(), String> {
        let tx = self.conn.unchecked_transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO packages (canonical_name) VALUES (?1)
             ON CONFLICT DO NOTHING",
            params![pkg.name],
        )
        .map_err(|e| e.to_string())?;
        let package_id: i64 = tx
            .query_row(
                "SELECT id FROM packages WHERE canonical_name = ?1",
                params![pkg.name],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        tx.execute(
            "INSERT INTO installed_instances
             (package_id, backend, backend_ref, version, scope, installed_at, last_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(backend, backend_ref) DO UPDATE SET
                version = excluded.version,
                scope = excluded.scope,
                last_seen_at = excluded.last_seen_at",
            params![
                package_id,
                pkg.source_name(),
                pkg.backend_ref,
                pkg.version,
                pkg.scope,
                pkg.installed_at,
                now
            ],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }
}

impl InstalledPackage {
    fn source_name(&self) -> &'static str {
        match self.source {
            crate::Source::Alpm => "alpm",
            crate::Source::Aur => "aur",
            crate::Source::Nix => "nix",
        }
    }
}