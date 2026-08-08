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
                canonical_name TEXT NOT NULL UNIQUE
             );
             CREATE TABLE IF NOT EXISTS installed_instances (
                id             INTEGER PRIMARY KEY,
                package_id     INTEGER NOT NULL REFERENCES packages(id),
                backend        TEXT NOT NULL,
                backend_ref    TEXT NOT NULL,
                version        TEXT,
                scope          TEXT,
                provenance     TEXT,
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
        match conn.execute_batch("ALTER TABLE installed_instances ADD COLUMN provenance TEXT") {
            Ok(()) => {}
            Err(e) if e.to_string().contains("duplicate column name") => {}
            Err(e) => return Err(e.to_string()),
        }
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
             (package_id, backend, backend_ref, version, scope, provenance, installed_at, last_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(backend, backend_ref) DO UPDATE SET
                version = excluded.version,
                scope = excluded.scope,
                provenance = excluded.provenance,
                last_seen_at = excluded.last_seen_at",
            params![
                package_id,
                pkg.source.as_str(),
                pkg.backend_ref,
                pkg.version,
                pkg.scope,
                provenance_str(&pkg.provenance),
                pkg.installed_at,
                now
            ],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }
}

fn provenance_str(provenance: &crate::model::Provenance) -> &'static str {
    match provenance {
        crate::model::Provenance::Native => "native",
        crate::model::Provenance::ForeignUnknown => "foreign-unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::InstalledPackage;
    use crate::Source;

    fn tmp_db() -> Storage {
        let path = format!(
            "/tmp/pacnix-test-{}.db",
            std::process::id()
        );
        let _ = std::fs::remove_file(&path);
        Storage::open(&path).unwrap()
    }

    #[test]
    fn alias_roundtrip() {
        let storage = tmp_db();
        storage
            .remember_alias("firefox", "alpm", "extra/firefox")
            .unwrap();
        assert_eq!(
            storage.alias("firefox").unwrap(),
            Some(("alpm".to_string(), "extra/firefox".to_string()))
        );
        storage
            .remember_alias("firefox", "aur", "aur/firefox-bin")
            .unwrap();
        assert_eq!(
            storage.alias("firefox").unwrap(),
            Some(("aur".to_string(), "aur/firefox-bin".to_string()))
        );
        assert_eq!(storage.alias("unknown").unwrap(), None);
    }

    #[test]
    fn upsert_instance_is_idempotent() {
        let storage = tmp_db();
        let pkg = InstalledPackage {
            source: Source::Alpm,
            backend_ref: "extra/firefox".into(),
            name: "firefox".into(),
            version: Some("1.0-1".into()),
            scope: None,
            installed_at: Some(1),
            provenance: crate::Provenance::Native,
        };
        storage.upsert_instance(&pkg).unwrap();
        let updated = InstalledPackage {
            version: Some("2.0-1".into()),
            ..pkg.clone()
        };
        storage.upsert_instance(&updated).unwrap();

        let conn = &storage.conn;
        let packages: i64 = conn
            .query_row("SELECT COUNT(*) FROM packages", [], |r| r.get(0))
            .unwrap();
        let instances: i64 = conn
            .query_row("SELECT COUNT(*) FROM installed_instances", [], |r| r.get(0))
            .unwrap();
        assert_eq!(packages, 1, "logical package must not be duplicated");
        assert_eq!(instances, 1, "instance must be updated, not duplicated");
        let version: String = conn
            .query_row(
                "SELECT version FROM installed_instances WHERE backend = 'alpm' AND backend_ref = 'extra/firefox'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version, "2.0-1");
    }
}