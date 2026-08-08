// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use rusqlite::{params, Connection};

use crate::model::InstalledPackage;

pub struct Storage {
    conn: Connection,
    #[cfg(test)]
    fail_upserts: std::sync::atomic::AtomicBool,
}

const SCHEMA_VERSION: i64 = 2;

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
                provenance_source TEXT,
                installed_at   INTEGER,
                last_seen_at   INTEGER NOT NULL,
                seen_generation INTEGER,
                UNIQUE (backend, backend_ref)
             );
             CREATE TABLE IF NOT EXISTS aliases (
                query          TEXT PRIMARY KEY,
                backend        TEXT NOT NULL,
                backend_ref    TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS install_receipts (
                id                  INTEGER PRIMARY KEY,
                package_name        TEXT NOT NULL,
                installed_backend   TEXT NOT NULL,
                installed_backend_ref TEXT NOT NULL,
                source              TEXT NOT NULL,
                source_ref          TEXT NOT NULL,
                version             TEXT,
                installed_at        INTEGER NOT NULL
             );",
        )
        .map_err(|e| e.to_string())?;
        Self::migrate(&conn)?;
        #[cfg(test)]
        {
            Ok(Self {
                conn,
                fail_upserts: std::sync::atomic::AtomicBool::new(false),
            })
        }
        #[cfg(not(test))]
        {
            Ok(Self { conn })
        }
    }

    fn migrate(conn: &Connection) -> Result<(), String> {
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if version < SCHEMA_VERSION {
            let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
            if version < 1 {
                for (column, column_type) in [
                    ("provenance", "TEXT"),
                    ("provenance_source", "TEXT"),
                    ("seen_generation", "INTEGER"),
                ] {
                    let sql = format!(
                        "ALTER TABLE installed_instances ADD COLUMN {column} {column_type}"
                    );
                    match tx.execute_batch(&sql) {
                        Ok(()) => {}
                        Err(e) if e.to_string().contains("duplicate column name") => {}
                        Err(e) => return Err(e.to_string()),
                    }
                }
                tx.execute_batch("DELETE FROM installed_instances WHERE backend = 'aur'")
                    .map_err(|e| e.to_string())?;
                let legacy: Vec<(String, String, String)> = {
                    let mut stmt = tx
                        .prepare(
                            "SELECT query, backend, backend_ref FROM aliases
                             WHERE backend NOT IN ('alpm', 'aur', 'nix')
                                OR backend_ref NOT LIKE '%/%'",
                        )
                        .map_err(|e| e.to_string())?;
                    let rows = stmt
                        .query_map([], |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, String>(2)?,
                            ))
                        })
                        .map_err(|e| e.to_string())?;
                    let mut out = Vec::new();
                    for row in rows {
                        out.push(row.map_err(|e| e.to_string())?);
                    }
                    out
                };
                for (query, backend, backend_ref) in legacy {
                    let new_backend = if backend == "aur" { "aur" } else { "alpm" };
                    let new_ref = format!("{backend}/{backend_ref}");
                    tx.execute(
                        "UPDATE aliases SET backend = ?2, backend_ref = ?3 WHERE query = ?1",
                        params![query, new_backend, new_ref],
                    )
                    .map_err(|e| e.to_string())?;
                }
            }
            if version < 2 {
                tx.execute_batch(
                    "CREATE TABLE installed_instances_new (
                        id             INTEGER PRIMARY KEY,
                        package_id     INTEGER NOT NULL REFERENCES packages(id),
                        backend        TEXT NOT NULL,
                        backend_ref    TEXT NOT NULL,
                        version        TEXT,
                        scope          TEXT,
                        provenance     TEXT,
                        provenance_source TEXT,
                        installed_at   INTEGER,
                        last_seen_at   INTEGER NOT NULL,
                        seen_generation INTEGER,
                        UNIQUE (backend, backend_ref)
                     );
                     INSERT INTO installed_instances_new
                        (id, package_id, backend, backend_ref, version, scope,
                         provenance, provenance_source, installed_at,
                         last_seen_at, seen_generation)
                        SELECT id, package_id, backend, backend_ref, version, scope,
                               provenance, provenance_source, installed_at,
                               last_seen_at, seen_generation
                        FROM installed_instances;
                     DROP TABLE installed_instances;
                     ALTER TABLE installed_instances_new RENAME TO installed_instances;",
                )
                .map_err(|e| e.to_string())?;
            }
            tx.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))
                .map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn remember_alias(
        &self,
        query: &str,
        backend: &str,
        backend_ref: &str,
    ) -> Result<(), String> {
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

    pub fn upsert_and_sweep(
        &self,
        pkgs: &[InstalledPackage],
        generation: u64,
        sweep_backend: &str,
    ) -> Result<usize, String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| e.to_string())?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        for pkg in pkgs {
            #[cfg(test)]
            if self.fail_upserts.load(std::sync::atomic::Ordering::SeqCst) {
                return Err("forced upsert failure (test hook)".into());
            }
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
            tx.execute(
                "INSERT INTO installed_instances
                 (package_id, backend, backend_ref, version, scope, provenance,
                  provenance_source, installed_at, last_seen_at, seen_generation)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(backend, backend_ref) DO UPDATE SET
                    version = excluded.version,
                    scope = excluded.scope,
                    provenance = excluded.provenance,
                    provenance_source = excluded.provenance_source,
                    installed_at = excluded.installed_at,
                    last_seen_at = excluded.last_seen_at,
                    seen_generation = excluded.seen_generation",
                params![
                    package_id,
                    pkg.source.as_str(),
                    pkg.backend_ref,
                    pkg.version,
                    pkg.scope,
                    provenance_str(&pkg.provenance),
                    provenance_source(&pkg.provenance),
                    pkg.installed_at,
                    now,
                    generation as i64
                ],
            )
            .map_err(|e| e.to_string())?;
        }
        let removed = tx
            .execute(
                "DELETE FROM installed_instances
                 WHERE backend = ?1
                   AND (seen_generation IS NULL OR seen_generation != ?2)",
                params![sweep_backend, generation as i64],
            )
            .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(removed)
    }

    pub fn upsert_instance_with_generation(
        &self,
        pkg: &InstalledPackage,
        generation: u64,
    ) -> Result<(), String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| e.to_string())?;
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
             (package_id, backend, backend_ref, version, scope, provenance,
              provenance_source, installed_at, last_seen_at, seen_generation)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(backend, backend_ref) DO UPDATE SET
                version = excluded.version,
                scope = excluded.scope,
                provenance = excluded.provenance,
                provenance_source = excluded.provenance_source,
                installed_at = excluded.installed_at,
                last_seen_at = excluded.last_seen_at,
                seen_generation = excluded.seen_generation",
            params![
                package_id,
                pkg.source.as_str(),
                pkg.backend_ref,
                pkg.version,
                pkg.scope,
                provenance_str(&pkg.provenance),
                provenance_source(&pkg.provenance),
                pkg.installed_at,
                now,
                generation as i64
            ],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }

    pub fn upsert_instance(&self, pkg: &InstalledPackage) -> Result<(), String> {
        self.upsert_instance_with_generation(pkg, 0)
    }

    pub fn record_receipt(&self, receipt: &crate::model::InstallReceipt) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO install_receipts
                 (package_name, installed_backend, installed_backend_ref, source, source_ref, version, installed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    receipt.package_name,
                    receipt.installed_backend,
                    receipt.installed_backend_ref,
                    receipt.source,
                    receipt.source_ref,
                    receipt.version,
                    receipt.installed_at
                ],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    pub fn known_source_for(
        &self,
        package_name: &str,
        installed_backend: &str,
        installed_backend_ref: &str,
        version: Option<&str>,
        installed_at: Option<i64>,
    ) -> Result<Option<String>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT source FROM install_receipts
                 WHERE package_name = ?1
                   AND installed_backend = ?2
                   AND installed_backend_ref = ?3
                   AND version IS ?4
                   AND (?5 IS NULL OR installed_at = ?5)
                 ORDER BY installed_at DESC LIMIT 1",
            )
            .map_err(|e| e.to_string())?;
        let mut rows = stmt
            .query_map(
                params![
                    package_name,
                    installed_backend,
                    installed_backend_ref,
                    version,
                    installed_at,
                ],
                |row| row.get::<_, String>(0),
            )
            .map_err(|e| e.to_string())?;
        match rows.next() {
            Some(Ok(source)) => Ok(Some(source)),
            Some(Err(e)) => Err(e.to_string()),
            None => Ok(None),
        }
    }
}

fn provenance_str(provenance: &crate::model::Provenance) -> &'static str {
    match provenance {
        crate::model::Provenance::Unknown => "unknown",
        crate::model::Provenance::SyncKnown => "sync-known",
        crate::model::Provenance::Foreign => "foreign",
        crate::model::Provenance::PacnixInstalled { .. } => "pacnix-installed",
    }
}

fn provenance_source(provenance: &crate::model::Provenance) -> Option<String> {
    match provenance {
        crate::model::Provenance::PacnixInstalled { source } => Some(source.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::InstalledPackage;
    use crate::Source;

    static TEST_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn tmp_db() -> Storage {
        let path = format!(
            "/tmp/pacnix-test-{}-{}.db",
            std::process::id(),
            TEST_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        );
        let _ = std::fs::remove_file(&path);
        Storage::open(&path).unwrap()
    }

    #[test]
    fn migrates_legacy_alias_format() {
        let path = format!("/tmp/pacnix-test-legacy-{}.db", std::process::id());
        let _ = std::fs::remove_file(&path);
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE aliases (
                    query TEXT PRIMARY KEY,
                    backend TEXT NOT NULL,
                    backend_ref TEXT NOT NULL
                 );",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO aliases (query, backend, backend_ref) VALUES
                 ('firefox', 'extra', 'firefox'),
                 ('hiddify', 'aur', 'hiddify-bin')",
                [],
            )
            .unwrap();
        }
        let storage = Storage::open(&path).unwrap();
        assert_eq!(
            storage.alias("firefox").unwrap(),
            Some(("alpm".to_string(), "extra/firefox".to_string()))
        );
        assert_eq!(
            storage.alias("hiddify").unwrap(),
            Some(("aur".to_string(), "aur/hiddify-bin".to_string()))
        );
        let version: i64 = storage
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn migrates_v1_to_v2_seen_generation_type() {
        let path = format!(
            "/tmp/pacnix-test-v1-{}.db",
            std::sync::atomic::AtomicU64::fetch_add(
                &TEST_COUNTER,
                1,
                std::sync::atomic::Ordering::SeqCst
            )
        );
        let _ = std::fs::remove_file(&path);
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE packages (
                    id             INTEGER PRIMARY KEY,
                    canonical_name TEXT NOT NULL UNIQUE
                 );
                 CREATE TABLE installed_instances (
                    id             INTEGER PRIMARY KEY,
                    package_id     INTEGER NOT NULL REFERENCES packages(id),
                    backend        TEXT NOT NULL,
                    backend_ref    TEXT NOT NULL,
                    version        TEXT,
                    scope          TEXT,
                    provenance     TEXT,
                    provenance_source TEXT,
                    installed_at   INTEGER,
                    last_seen_at   INTEGER NOT NULL,
                    seen_generation TEXT
                 );
                 CREATE TABLE aliases (
                    query          TEXT PRIMARY KEY,
                    backend        TEXT NOT NULL,
                    backend_ref    TEXT NOT NULL
                 );
                 CREATE TABLE install_receipts (
                    id                  INTEGER PRIMARY KEY,
                    package_name        TEXT NOT NULL,
                    installed_backend   TEXT NOT NULL,
                    installed_backend_ref TEXT NOT NULL,
                    source              TEXT NOT NULL,
                    source_ref          TEXT NOT NULL,
                    version             TEXT,
                    installed_at        INTEGER NOT NULL
                 );
                 INSERT INTO packages (id, canonical_name) VALUES (1, 'oldpkg');
                 INSERT INTO installed_instances
                    (id, package_id, backend, backend_ref, version, scope,
                     provenance, provenance_source, installed_at, last_seen_at,
                     seen_generation)
                    VALUES (1, 1, 'alpm', 'local/oldpkg', NULL, NULL, 'sync-known', NULL, 42, 1, 7);
                 PRAGMA user_version = 1;",
            )
            .unwrap();
        }
        let storage = Storage::open(&path).unwrap();
        let version: i64 = storage
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let declared: String = storage
            .conn
            .query_row(
                "SELECT type FROM pragma_table_info('installed_instances')
                 WHERE name = 'seen_generation'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(declared, "INTEGER", "v1 DB must be rebuilt with INTEGER");
        let (installed_at, seen): (i64, i64) = storage
            .conn
            .query_row(
                "SELECT installed_at, seen_generation FROM installed_instances
                 WHERE backend_ref = 'local/oldpkg'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (installed_at, seen),
            (42, 7),
            "rows must survive the rebuild"
        );
    }

    #[test]
    fn sweep_removes_stale_instances() {
        let storage = tmp_db();
        let pkg = InstalledPackage {
            source: Source::Alpm,
            backend_ref: "local/oldpkg".into(),
            name: "oldpkg".into(),
            version: None,
            scope: None,
            installed_at: None,
            provenance: crate::Provenance::Foreign,
        };
        storage.upsert_instance_with_generation(&pkg, 1).unwrap();
        let pkg2 = InstalledPackage {
            name: "newpkg".into(),
            backend_ref: "local/newpkg".into(),
            ..pkg.clone()
        };
        storage.upsert_instance_with_generation(&pkg2, 2).unwrap();
        let removed = storage.upsert_and_sweep(&[pkg2], 2, "alpm").unwrap();
        assert_eq!(removed, 1);
    }

    #[test]
    fn sweep_removes_legacy_null_generation() {
        let storage = tmp_db();
        let pkg = InstalledPackage {
            source: Source::Alpm,
            backend_ref: "local/oldpkg".into(),
            name: "oldpkg".into(),
            version: None,
            scope: None,
            installed_at: None,
            provenance: crate::Provenance::SyncKnown,
        };
        storage.upsert_instance(&pkg).unwrap();
        let removed = storage.upsert_and_sweep(&[], 10, "alpm").unwrap();
        assert_eq!(removed, 1);
    }

    #[test]
    fn failed_upsert_sweeps_nothing() {
        let storage = tmp_db();
        let pkg = InstalledPackage {
            source: Source::Alpm,
            backend_ref: "local/keepme".into(),
            name: "keepme".into(),
            version: None,
            scope: None,
            installed_at: None,
            provenance: crate::Provenance::Foreign,
        };
        storage.upsert_instance_with_generation(&pkg, 1).unwrap();
        storage
            .fail_upserts
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let doomed = InstalledPackage {
            name: "boom".into(),
            backend_ref: "local/boom".into(),
            ..pkg.clone()
        };
        let result = storage.upsert_and_sweep(&[doomed], 2, "alpm");
        storage
            .fail_upserts
            .store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(result.is_err(), "upsert must fail");
        let kept: i64 = storage
            .conn
            .query_row(
                "SELECT COUNT(*) FROM installed_instances WHERE backend = 'alpm' AND backend_ref = 'local/keepme'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kept, 1, "old row must survive failed reconcile");
        let inserted: i64 = storage
            .conn
            .query_row(
                "SELECT COUNT(*) FROM installed_instances WHERE backend_ref = 'local/boom'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(inserted, 0, "failed transaction must leave no partial rows");
    }

    #[test]
    fn receipt_roundtrip() {
        let storage = tmp_db();
        let receipt = crate::InstallReceipt {
            package_name: "foo".into(),
            installed_backend: "alpm".into(),
            installed_backend_ref: "local/foo".into(),
            source: "aur".into(),
            source_ref: "aur/foo-bin".into(),
            version: Some("1.0-1".into()),
            installed_at: 42,
        };
        storage.record_receipt(&receipt).unwrap();
        assert_eq!(
            storage
                .known_source_for("foo", "alpm", "local/foo", Some("1.0-1"), Some(42))
                .unwrap(),
            Some("aur".into())
        );
        assert_eq!(
            storage
                .known_source_for("foo", "alpm", "local/foo", Some("1.0-1"), Some(43))
                .unwrap(),
            None,
            "reinstalled incarnation must not inherit the old receipt"
        );
        assert_eq!(
            storage
                .known_source_for("foo", "alpm", "local/foo", Some("2.0-1"), Some(42))
                .unwrap(),
            None,
            "a different version must not match"
        );
        assert_eq!(
            storage
                .known_source_for("foo", "nix", "nix/foo", None, None)
                .unwrap(),
            None
        );
        let nixish = crate::InstallReceipt {
            package_name: "bar".into(),
            installed_backend: "nix".into(),
            installed_backend_ref: "/nix/store/000-bar-2.0".into(),
            source: "nixpkgs".into(),
            source_ref: "nixpkgs#bar".into(),
            version: Some("2.0".into()),
            installed_at: 7,
        };
        storage.record_receipt(&nixish).unwrap();
        assert_eq!(
            storage
                .known_source_for("bar", "nix", "/nix/store/000-bar-2.0", Some("2.0"), None)
                .unwrap(),
            Some("nixpkgs".into()),
            "a backend without an incarnation token must still match"
        );
        assert_eq!(
            storage
                .known_source_for("bar", "nix", "/nix/store/000-bar-3.0", Some("3.0"), None)
                .unwrap(),
            None,
            "version must still be checked when time is unknown"
        );
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
            provenance: crate::Provenance::SyncKnown,
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
