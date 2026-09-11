//! SQLCipher-only history with private files, transactional retention, and literal search.
use crate::{
    content::{normalize, Capture, Kind, Representation},
    settings::Settings,
};
use rusqlite::{params, Connection, OptionalExtension};
use std::{
    fs,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroizing;
#[link(name = "sqlcipher")]
extern "C" {
    fn sqlite3_key(db: *mut rusqlite::ffi::sqlite3, key: *const std::ffi::c_void, len: i32) -> i32;
}
pub type Result<T> = std::result::Result<T, &'static str>;
/// Lightweight card data; original payloads are fetched only for restoration.
#[derive(Clone, Debug)]
pub struct Item {
    pub id: i64,
    pub kind: Kind,
    pub preview: String,
    pub source: String,
    pub captured: i64,
    pub pinned: bool,
    pub thumbnail: Option<Vec<u8>>,
}
/// Encrypted database handle, confined to a worker thread.
pub struct Storage {
    conn: Connection,
    path: PathBuf,
    pub cipher_version: String,
}
fn available_space(path: &Path) -> Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| "storage-path")?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::statvfs(c.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return Err("storage-stat");
    }
    let stats = unsafe { stats.assume_init() };
    Ok(stats.f_bavail.saturating_mul(stats.f_frsize))
}
/// Seconds since Unix epoch.
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
impl Storage {
    /// Open verified SQLCipher storage; ordinary SQLite is always rejected.
    pub fn open(path: &Path, key: &[u8]) -> Result<Self> {
        if key.len() != 32 {
            return Err("invalid-key");
        }
        let parent = path.parent().ok_or("storage-path")?;
        fs::create_dir_all(parent).map_err(|_| "storage-permissions")?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
            .map_err(|_| "storage-permissions")?;
        if path
            .symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err("storage-symlink");
        }
        if !path.exists() {
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)
                .map_err(|_| "storage-create")?;
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|_| "storage-permissions")?;
        let conn = Connection::open(path).map_err(|_| "storage-open")?;
        let cipher_version: String = conn
            .query_row("PRAGMA cipher_version", [], |r| r.get(0))
            .map_err(|_| "sqlcipher-required")?;
        if cipher_version.is_empty() {
            return Err("sqlcipher-required");
        }
        let hex = Zeroizing::new(key.iter().map(|b| format!("{b:02x}")).collect::<String>());
        let raw = Zeroizing::new(format!("x'{}'", hex.as_str()));
        let rc = unsafe { sqlite3_key(conn.handle(), raw.as_ptr().cast(), raw.len() as i32) };
        if rc != rusqlite::ffi::SQLITE_OK {
            return Err("storage-key");
        }
        conn.execute_batch("PRAGMA cipher_memory_security=ON; PRAGMA temp_store=MEMORY; PRAGMA secure_delete=ON; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=250;").map_err(|_|"storage-config")?;
        let memory: String = conn
            .query_row("PRAGMA cipher_memory_security", [], |r| r.get(0))
            .map_err(|_| "cipher-memory-security-required")?;
        let temp: i64 = conn
            .query_row("PRAGMA temp_store", [], |r| r.get(0))
            .map_err(|_| "storage-config")?;
        let authenticated: String = conn
            .query_row("PRAGMA cipher_use_hmac", [], |r| r.get(0))
            .map_err(|_| "cipher-authentication-required")?;
        if memory != "1" || temp != 2 || authenticated != "1" {
            return Err("storage-config");
        }
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| {
            r.get::<_, i64>(0)
        })
        .map_err(|_| "key-or-database-invalid")?;
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(|_| "storage-schema")?;
        if version > 1 {
            return Err("newer-schema");
        }
        if version == 0 {
            conn.execute_batch("BEGIN IMMEDIATE;
CREATE TABLE items(id INTEGER PRIMARY KEY,kind TEXT NOT NULL,created INTEGER NOT NULL,captured INTEGER NOT NULL,restored INTEGER,source TEXT NOT NULL,preview TEXT NOT NULL,search TEXT NOT NULL,checksum BLOB NOT NULL,pinned INTEGER NOT NULL DEFAULT 0,size INTEGER NOT NULL,occurrences INTEGER NOT NULL DEFAULT 1,thumbnail BLOB,files TEXT NOT NULL,width INTEGER NOT NULL,height INTEGER NOT NULL);
CREATE TABLE representations(item_id INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,mime TEXT NOT NULL,content BLOB NOT NULL,PRIMARY KEY(item_id,mime));
CREATE VIRTUAL TABLE items_fts USING fts4(search);
CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY,completed INTEGER NOT NULL);
INSERT INTO schema_migrations VALUES(1,strftime('%s','now'));
PRAGMA user_version=1; COMMIT;").map_err(|_|"schema-or-fts-unavailable")?;
        }
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=64;")
            .map_err(|_| "storage-wal")?;
        let store = Self {
            conn,
            path: path.to_owned(),
            cipher_version,
        };
        store.integrity()?;
        Ok(store)
    }
    /// Open an independently keyed, query-only connection for interactive reads.
    pub fn open_reader(path: &Path, key: &[u8]) -> Result<Self> {
        let db = Self::open(path, key)?;
        db.conn
            .execute_batch("PRAGMA query_only=ON")
            .map_err(|_| "storage-config")?;
        Ok(db)
    }
    /// Authenticate all cipher pages and check SQLite structure.
    pub fn integrity(&self) -> Result<()> {
        let mut s = self
            .conn
            .prepare("PRAGMA cipher_integrity_check")
            .map_err(|_| "integrity")?;
        if s.query([])
            .map_err(|_| "integrity")?
            .next()
            .map_err(|_| "integrity")?
            .is_some()
        {
            return Err("integrity");
        }
        let result: String = self
            .conn
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .map_err(|_| "integrity")?;
        if result != "ok" {
            return Err("integrity");
        }
        Ok(())
    }
    /// Store one eligible item and apply retention atomically.
    pub fn insert(&mut self, c: &Capture, s: &Settings) -> Result<i64> {
        self.insert_if(c, s, || true)
    }
    /// Commit only if the privacy epoch is still valid immediately before commit.
    pub fn insert_if(
        &mut self,
        c: &Capture,
        s: &Settings,
        valid: impl Fn() -> bool,
    ) -> Result<i64> {
        if !valid() {
            return Err("capture-cancelled");
        }
        let now = now();
        let size = c.size() as u64;
        let reserved = size.saturating_mul(4).saturating_add(64 * 1024);
        if available_space(&self.path)? < reserved {
            return Err("storage-disk-full");
        }
        let pins: u64 = self
            .conn
            .query_row(
                "SELECT coalesce(sum(size),0) FROM items WHERE pinned=1",
                [],
                |r| r.get(0),
            )
            .map_err(|_| "storage-read")?;
        let tx = self.conn.transaction().map_err(|_| "storage-busy")?;
        let last: Option<(i64, Vec<u8>)> = tx
            .query_row(
                "SELECT id,checksum FROM items ORDER BY captured DESC,id DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|_| "storage-read")?;
        let id = if let Some((id, hash)) = last.filter(|(_, h)| h == &c.checksum) {
            let _ = hash;
            tx.execute(
                "UPDATE items_fts SET search=?1 WHERE rowid=?2",
                params![c.search, id],
            )
            .map_err(|_| "storage-index")?;
            tx.execute(
                "UPDATE items SET captured=?1,source=?2,occurrences=occurrences+1,search=?4 WHERE id=?3",
                params![now, c.source, id,c.search],
            )
            .map_err(|_| "storage-write")?;
            id
        } else {
            if pins.saturating_add(size) > s.max_bytes {
                return Err("storage-capacity");
            }
            tx.execute("INSERT INTO items(kind,created,captured,source,preview,search,checksum,size,thumbnail,files,width,height) VALUES(?1,?2,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![c.kind.label(),now,c.source,c.preview,c.search,c.checksum,size,c.thumbnail,serde_json::to_string(&c.files).map_err(|_|"storage-encode")?,c.width,c.height]).map_err(|_|"storage-write")?;
            let id = tx.last_insert_rowid();
            for r in &c.reps {
                tx.execute(
                    "INSERT INTO representations VALUES(?1,?2,?3)",
                    params![id, r.mime, r.bytes],
                )
                .map_err(|_| "storage-write")?;
            }
            tx.execute(
                "INSERT INTO items_fts(rowid,search) VALUES(?1,?2)",
                params![id, c.search],
            )
            .map_err(|_| "storage-write")?;
            id
        };
        tx.execute("DELETE FROM items WHERE pinned=0 AND (captured < ?1 OR id IN (SELECT id FROM items WHERE pinned=0 ORDER BY captured DESC,id DESC LIMIT -1 OFFSET ?2))",params![now-(s.max_days*86400) as i64,s.max_items as i64]).map_err(|_|"storage-prune")?;
        loop {
            let total: u64 = tx
                .query_row("SELECT coalesce(sum(size),0) FROM items", [], |r| r.get(0))
                .map_err(|_| "storage-read")?;
            if total <= s.max_bytes {
                break;
            }
            if tx.execute("DELETE FROM items WHERE id=(SELECT id FROM items WHERE pinned=0 ORDER BY captured,id LIMIT 1)",[]).map_err(|_|"storage-prune")?==0{return Err("storage-capacity");}
        }
        // Recreate the index in the same transaction when pruning removed rows;
        // FTS tombstones must not retain deleted searchable plaintext.
        let orphaned: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM items_fts WHERE rowid NOT IN (SELECT id FROM items))",
                [],
                |r| r.get(0),
            )
            .map_err(|_| "storage-prune")?;
        if orphaned {
            tx.execute_batch("DROP TABLE items_fts; CREATE VIRTUAL TABLE items_fts USING fts4(search); INSERT INTO items_fts(rowid,search) SELECT id,search FROM items;").map_err(|_|"storage-index")?;
        }
        if !valid() {
            return Err("capture-cancelled");
        }
        tx.commit().map_err(|_| "storage-write")?;
        self.maintain(s)?;
        let retained: bool = self
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM items WHERE id=?1)",
                [id],
                |r| r.get(0),
            )
            .map_err(|_| "storage-read")?;
        if !retained {
            return Err("storage-capacity");
        }
        Ok(id)
    }
    /// Expire old unpinned rows at startup and after retention settings change.
    pub fn expire(&mut self, settings: &Settings) -> Result<()> {
        let changed=self.conn.execute("DELETE FROM items WHERE pinned=0 AND (captured < ?1 OR id IN (SELECT id FROM items WHERE pinned=0 ORDER BY captured DESC,id DESC LIMIT -1 OFFSET ?2))",params![now()-(settings.max_days*86400) as i64,settings.max_items as i64]).map_err(|_|"storage-prune")?;
        if changed > 0 {
            self.invalidate_backup()?;
            self.rebuild_index()?;
        }
        self.maintain(settings)
    }
    /// Keep physical database and WAL growth inside the configured live target.
    pub fn maintain(&mut self, settings: &Settings) -> Result<()> {
        self.conn
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .map_err(|_| "storage-checkpoint")?;
        let mut size = fs::metadata(&self.path).map_err(|_| "storage-stat")?.len();
        if size <= settings.max_bytes {
            return Ok(());
        }
        if available_space(&self.path)? < size.saturating_mul(2) {
            return Err("storage-maintenance-space");
        }
        self.conn
            .execute_batch("VACUUM; PRAGMA wal_checkpoint(TRUNCATE)")
            .map_err(|_| "storage-maintenance")?;
        size = fs::metadata(&self.path).map_err(|_| "storage-stat")?.len();
        while size > settings.max_bytes {
            let deleted=self.conn.execute("DELETE FROM items WHERE id=(SELECT id FROM items WHERE pinned=0 ORDER BY captured,id LIMIT 1)",[]).map_err(|_|"storage-prune")?;
            if deleted == 0 {
                return Err("storage-capacity");
            }
            self.rebuild_index()?;
            self.conn
                .execute_batch("VACUUM; PRAGMA wal_checkpoint(TRUNCATE)")
                .map_err(|_| "storage-maintenance")?;
            size = fs::metadata(&self.path).map_err(|_| "storage-stat")?.len();
        }
        Ok(())
    }
    /// Search literal text with an indexed word-prefix prefilter.
    pub fn search(&self, query: &str, filter: &str) -> Result<Vec<Item>> {
        self.search_page(query, filter, 0)
    }
    /// Fetch a bounded history page without loading original payloads.
    pub fn search_page(&self, query: &str, filter: &str, offset: u32) -> Result<Vec<Item>> {
        let q = normalize(query)
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let terms = normalize(query)
            .split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty())
            .take(32)
            .map(|s| format!("\"{s}*\""))
            .collect::<Vec<_>>()
            .join(" ");
        let prefix = if terms.is_empty() {
            "?4='' AND "
        } else {
            "id IN (SELECT rowid FROM items_fts WHERE items_fts MATCH ?4) AND "
        };
        let sql=format!("SELECT id,kind,preview,source,captured,pinned,thumbnail FROM items WHERE {prefix}(search LIKE ?1 ESCAPE '\\' OR lower(source) LIKE ?1 ESCAPE '\\') AND (?2='Everything' OR (?2='Pinned' AND pinned=1) OR (?2='Text' AND kind IN ('Text','Rich text')) OR (?2='Images' AND kind='Image') OR (?2='Links' AND kind='Link') OR (?2='Files' AND kind='Files')) ORDER BY captured DESC,id DESC LIMIT 60 OFFSET ?3");
        let mut stmt = self.conn.prepare(&sql).map_err(|_| "storage-search")?;
        let rows = stmt
            .query_map(params![format!("%{q}%"), filter, offset, terms], |r| {
                Ok(Item {
                    id: r.get(0)?,
                    kind: Kind::parse(&r.get::<_, String>(1)?),
                    preview: r.get(2)?,
                    source: r.get(3)?,
                    captured: r.get(4)?,
                    pinned: r.get(5)?,
                    thumbnail: r.get(6)?,
                })
            })
            .map_err(|_| "storage-search")?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|_| "storage-search")
    }
    /// Retrieve original formats only for the selected item.
    pub fn restore(&self, id: i64) -> Result<Vec<Representation>> {
        let reps = self.representations(id)?;
        self.touch(id)?;
        Ok(reps)
    }
    /// Record restoration independently from capture expiry.
    pub fn touch(&self, id: i64) -> Result<()> {
        self.conn
            .execute(
                "UPDATE items SET restored=?1 WHERE id=?2",
                params![now(), id],
            )
            .map_err(|_| "storage-write")?;
        Ok(())
    }
    /// Retrieve selected representations using a read-only connection.
    pub fn representations(&self, id: i64) -> Result<Vec<Representation>> {
        let mut stmt = self
            .conn
            .prepare("SELECT mime,content FROM representations WHERE item_id=?1 ORDER BY mime")
            .map_err(|_| "storage-read")?;
        let rows = stmt
            .query_map([id], |r| {
                Ok(Representation::new(&r.get::<_, String>(0)?, r.get(1)?))
            })
            .map_err(|_| "storage-read")?;
        let reps = rows
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|_| "storage-read")?;
        if reps.is_empty() {
            return Err("item-unavailable");
        }
        Ok(crate::content::restoration(reps))
    }
    /// Toggle pin state without changing capture recency.
    pub fn pin(&self, id: i64) -> Result<()> {
        self.conn
            .execute("UPDATE items SET pinned=1-pinned WHERE id=?1", [id])
            .map_err(|_| "storage-write")?;
        Ok(())
    }
    /// Delete one item, enforcing pinned confirmation in the storage layer too.
    pub fn delete(&mut self, id: i64, confirmed: bool) -> Result<()> {
        let pinned: bool = self
            .conn
            .query_row("SELECT pinned FROM items WHERE id=?1", [id], |r| r.get(0))
            .optional()
            .map_err(|_| "storage-read")?
            .unwrap_or(false);
        if pinned && !confirmed {
            return Err("confirmation-required");
        }
        self.invalidate_backup()?;
        self.conn
            .execute("DELETE FROM items WHERE id=?1", [id])
            .map_err(|_| "storage-delete")?;
        self.rebuild_index()
    }
    /// Clear history, retaining pins unless explicitly requested otherwise.
    pub fn clear(&mut self, include_pins: bool) -> Result<()> {
        self.invalidate_backup()?;
        self.conn
            .execute("DELETE FROM items WHERE pinned=0 OR ?1", [include_pins])
            .map_err(|_| "storage-delete")?;
        self.rebuild_index()?;
        self.conn
            .execute_batch(
                "PRAGMA wal_checkpoint(TRUNCATE); VACUUM; PRAGMA wal_checkpoint(TRUNCATE);",
            )
            .map_err(|_| "storage-maintenance")
    }
    fn rebuild_index(&self) -> Result<()> {
        self.conn.execute_batch("BEGIN; DROP TABLE items_fts; CREATE VIRTUAL TABLE items_fts USING fts4(search); INSERT INTO items_fts(rowid,search) SELECT id,search FROM items; COMMIT; PRAGMA wal_checkpoint(TRUNCATE);").map_err(|_|"storage-index")
    }
    fn invalidate_backup(&self) -> Result<()> {
        let p = self.path.with_extension("recovery.db");
        if p.exists() {
            fs::remove_file(p).map_err(|_| "backup-delete")?;
        }
        Ok(())
    }
    /// Number of retained items, without reading their content.
    pub fn count(&self) -> Result<u64> {
        self.conn
            .query_row("SELECT count(*) FROM items", [], |r| r.get(0))
            .map_err(|_| "storage-read")
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{prepare, Representation};
    #[test]
    fn encrypted_roundtrip_and_privacy() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("history.db");
        let key = [42; 32];
        let mut db = Storage::open(&p, &key).unwrap();
        let c = prepare(
            vec![Representation::new(
                "text/plain",
                b"fixture-sensitive-unique-749".to_vec(),
            )],
            "editor".into(),
            &Settings::default(),
        )
        .unwrap();
        let id = db.insert(&c, &Settings::default()).unwrap();
        assert_eq!(db.search("unique", "Everything").unwrap().len(), 1);
        assert_eq!(db.restore(id).unwrap()[0].bytes, c.reps[0].bytes);
        db.pin(id).unwrap();
        db.clear(false).unwrap();
        assert_eq!(db.count().unwrap(), 1);
        assert!(db.delete(id, false).is_err());
        db.delete(id, true).unwrap();
        assert_eq!(db.count().unwrap(), 0);
        drop(db);
        assert!(Storage::open(&p, &[43; 32]).is_err());
        for f in fs::read_dir(dir.path()).unwrap() {
            let b = fs::read(f.unwrap().path()).unwrap();
            assert!(!b.windows(16).any(|w| w == b"SQLite format 3\0"));
            assert!(!b
                .windows(b"fixture-sensitive-unique-749".len())
                .any(|w| w == b"fixture-sensitive-unique-749"));
        }
    }
    #[test]
    fn pin_pressure_and_dedup() {
        let d = tempfile::tempdir().unwrap();
        let mut db = Storage::open(&d.path().join("h.db"), &[7; 32]).unwrap();
        let mut s = Settings::default();
        s.max_items = 1;
        let make = |v: &str| {
            prepare(
                vec![Representation::new("text/plain", v.as_bytes().to_vec())],
                "".into(),
                &Settings::default(),
            )
            .unwrap()
        };
        let a = db.insert(&make("first"), &s).unwrap();
        db.pin(a).unwrap();
        db.insert(&make("second"), &s).unwrap();
        db.insert(&make("second"), &s).unwrap();
        assert_eq!(db.count().unwrap(), 2);
        db.insert(&make("third"), &s).unwrap();
        assert_eq!(db.count().unwrap(), 2);
        assert_eq!(db.search("first", "Pinned").unwrap().len(), 1);
    }
    #[test]
    fn cancelled_capture_never_commits() {
        let d = tempfile::tempdir().unwrap();
        let mut db = Storage::open(&d.path().join("h.db"), &[9; 32]).unwrap();
        let c = prepare(
            vec![Representation::new(
                "text/plain",
                b"never persisted".to_vec(),
            )],
            "editor".into(),
            &Settings::default(),
        )
        .unwrap();
        let checks = std::cell::Cell::new(0);
        assert_eq!(
            db.insert_if(&c, &Settings::default(), || {
                let n = checks.get();
                checks.set(n + 1);
                n == 0
            }),
            Err("capture-cancelled")
        );
        assert_eq!(db.count().unwrap(), 0);
        assert!(db.search("persisted", "Everything").unwrap().is_empty());
    }
    #[test]
    fn search_literals_paging_and_expiry() {
        let d = tempfile::tempdir().unwrap();
        let mut db = Storage::open(&d.path().join("h.db"), &[10; 32]).unwrap();
        let mut settings = Settings::default();
        settings.max_items = 100;
        for n in 0..65 {
            let c = prepare(
                vec![Representation::new(
                    "text/plain",
                    format!("literal 100%_{n} phrase").into_bytes(),
                )],
                "editor".into(),
                &settings,
            )
            .unwrap();
            db.insert(&c, &settings).unwrap();
        }
        assert_eq!(db.search("100%_", "Everything").unwrap().len(), 60);
        assert_eq!(db.search_page("100%_", "Everything", 60).unwrap().len(), 5);
        assert!(db.search("\" OR *", "Everything").unwrap().is_empty());
        db.conn.execute("UPDATE items SET captured=0", []).unwrap();
        db.pin(1).unwrap();
        db.expire(&settings).unwrap();
        assert_eq!(db.count().unwrap(), 1);
        assert_eq!(db.search("literal", "Pinned").unwrap().len(), 1);
        let fts: i64 = db
            .conn
            .query_row("SELECT count(*) FROM items_fts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fts, 1);
    }
    #[test]
    fn newer_schema_is_preserved() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("h.db");
        let db = Storage::open(&path, &[11; 32]).unwrap();
        db.conn.execute_batch("PRAGMA user_version=99").unwrap();
        drop(db);
        assert!(matches!(
            Storage::open(&path, &[11; 32]),
            Err("newer-schema")
        ));
        assert!(path.exists());
    }
    #[test]
    fn search_ten_thousand_entries() {
        let d = tempfile::tempdir().unwrap();
        let mut db = Storage::open(&d.path().join("h.db"), &[12; 32]).unwrap();
        let tx = db.conn.transaction().unwrap();
        for n in 0..10_000 {
            tx.execute("INSERT INTO items(kind,created,captured,source,preview,search,checksum,size,files,width,height) VALUES('Text',0,?1,'editor',?2,?2,x'00',12,'[]',0,0)",params![n,format!("synthetic research entry {n}")]).unwrap();
        }
        tx.execute(
            "INSERT INTO items_fts(rowid,search) SELECT id,search FROM items",
            [],
        )
        .unwrap();
        tx.commit().unwrap();
        let mut times = Vec::new();
        for _ in 0..20 {
            let start = std::time::Instant::now();
            assert_eq!(db.search("entry 9999", "Everything").unwrap().len(), 1);
            times.push(start.elapsed());
        }
        times.sort();
        eprintln!("10,000-entry literal-search p95: {:?}", times[18]);
        assert!(times[18] < std::time::Duration::from_millis(100));
    }
}
