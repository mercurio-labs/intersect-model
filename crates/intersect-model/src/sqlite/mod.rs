//! Handwritten native persistence adapter over the shared model contract.
//! Format 5 adds explicit resources to metadata, fields and ordered collections. Local
//! handles never cross this boundary: persistence uses external identities.
#![cfg(not(target_arch = "wasm32"))]
use intersect_model::core::storage::InvocationContext;
use intersect_model::core::{
    model::Element,
    storage::{
        self, Checkpoint, CheckpointIssuer, ModelRead, ModelStore, Query, QueryCandidates, Row,
        StorageResult, query_key,
    },
};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use std::path::Path;

/// Native transactional raw adapter. Initial schema adoption resolves provisional
/// roots from unbound producers once. Bound raw producers that insert forward
/// containment edges must finalize explicit root records before validation;
/// checked model edits maintain resource consistency directly.
pub struct SqliteStore {
    connection: Connection,
    frames: Vec<Frame>,
    checkpoints: CheckpointIssuer,
    next_savepoint: u64,
    schema: Option<std::sync::Arc<intersect_model::definitions::schema::Schema>>,
}
struct Frame {
    checkpoint: Checkpoint,
    savepoint: u64,
    schema: Option<std::sync::Arc<intersect_model::definitions::schema::Schema>>,
}
fn error(e: impl std::fmt::Display) -> String {
    format!("SQLite storage: {e}")
}
fn encode(value: &impl serde::Serialize) -> StorageResult<String> {
    serde_json::to_string(value).map_err(error)
}
const TABLES: &str = "
    CREATE TABLE IF NOT EXISTS intersect_metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);
    INSERT OR IGNORE INTO intersect_metadata VALUES ('revision','');
    INSERT OR IGNORE INTO intersect_metadata VALUES ('resource_roots_initialized','false');
    CREATE TABLE IF NOT EXISTS intersect_resources (uri TEXT PRIMARY KEY, payload TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS intersect_elements (seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE, payload TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS intersect_fields (seq INTEGER NOT NULL REFERENCES intersect_elements(seq) ON DELETE CASCADE, name TEXT NOT NULL, kind INTEGER NOT NULL CHECK(kind IN (0,1)), value TEXT, PRIMARY KEY(seq,name));
    CREATE TABLE IF NOT EXISTS intersect_items (seq INTEGER NOT NULL, name TEXT NOT NULL, position INTEGER NOT NULL CHECK(position>=0), value TEXT NOT NULL, PRIMARY KEY(seq,name,position), FOREIGN KEY(seq,name) REFERENCES intersect_fields(seq,name) ON DELETE CASCADE);
    CREATE TABLE IF NOT EXISTS intersect_indexes (kind TEXT NOT NULL, value TEXT NOT NULL, seq INTEGER NOT NULL REFERENCES intersect_elements(seq) ON DELETE CASCADE, PRIMARY KEY(kind,value,seq));
    CREATE INDEX IF NOT EXISTS intersect_indexes_by_seq ON intersect_indexes(seq);";

impl SqliteStore {
    pub fn open(path: impl AsRef<Path>) -> StorageResult<Self> {
        Self::initialize(Connection::open(path).map_err(error)?)
    }
    pub fn in_memory() -> StorageResult<Self> {
        Self::initialize(Connection::open_in_memory().map_err(error)?)
    }
    fn initialize(connection: Connection) -> StorageResult<Self> {
        connection
            .execute_batch("PRAGMA foreign_keys=ON;")
            .map_err(error)?;
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(error)?;
        if version != 0 && version != 5 {
            return Err(format!(
                "Unsupported SQLite model format {version}; expected format 5"
            ));
        }
        if version == 0 {
            let existing: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE name LIKE 'intersect_%'",
                    [],
                    |row| row.get(0),
                )
                .map_err(error)?;
            if existing != 0 {
                return Err("Unversioned Intersect database is not a fresh store".into());
            }
        }
        connection
            .execute_batch("BEGIN IMMEDIATE;")
            .map_err(error)?;
        let result = connection.execute_batch(TABLES).and_then(|()| {
            connection.execute_batch(if version == 0 {
                "PRAGMA user_version=5; COMMIT;"
            } else {
                "COMMIT;"
            })
        });
        if let Err(e) = result {
            let _ = connection.execute_batch("ROLLBACK;");
            return Err(error(e));
        }
        Ok(Self {
            connection,
            frames: vec![],
            checkpoints: CheckpointIssuer::default(),
            next_savepoint: 0,
            schema: None,
        })
    }
    fn active(&self) -> StorageResult<()> {
        if self.frames.is_empty() {
            Err("Storage write requires a transaction".into())
        } else {
            Ok(())
        }
    }
    fn check(&self, token: &Checkpoint) -> StorageResult<u64> {
        self.frames
            .last()
            .filter(|frame| &frame.checkpoint == token)
            .map(|frame| frame.savepoint)
            .ok_or_else(|| "Invalid or non-LIFO storage checkpoint".into())
    }
    fn sequence(&self, id: &str) -> StorageResult<i64> {
        self.connection
            .query_row(
                "SELECT seq FROM intersect_elements WHERE id=?1",
                [id],
                |r| r.get(0),
            )
            .optional()
            .map_err(error)?
            .ok_or_else(|| format!("Missing element {id}"))
    }
    fn metadata(&self, id: &str) -> StorageResult<Option<Element>> {
        let payload: Option<String> = self
            .connection
            .query_row(
                "SELECT payload FROM intersect_elements WHERE id=?1",
                [id],
                |r| r.get(0),
            )
            .optional()
            .map_err(error)?;
        payload
            .map(|payload| {
                let element: Element = serde_json::from_str(&payload).map_err(error)?;
                Ok(element)
            })
            .transpose()
    }
    fn is_containment(&self, element: &Element, field: &str) -> bool {
        self.schema
            .as_ref()
            .and_then(|schema| schema.effective_feature(&element.class, field).ok())
            .is_some_and(|feature| feature.feature.containment)
    }
    fn index_atom(&self, seq: i64, field: &str, value: &Value) -> StorageResult<()> {
        if !value.is_null() {
            self.connection
                .execute(
                    "INSERT OR IGNORE INTO intersect_indexes(kind,value,seq) VALUES (?1,?2,?3)",
                    params![format!("value:{field}"), encode(value)?, seq],
                )
                .map_err(error)?;
        }
        Ok(())
    }
    fn write_field(&self, seq: i64, field: &str, value: Option<Value>) -> StorageResult<()> {
        self.connection
            .execute(
                "DELETE FROM intersect_indexes WHERE seq=?1 AND kind=?2",
                params![seq, format!("value:{field}")],
            )
            .map_err(error)?;
        self.connection
            .execute(
                "DELETE FROM intersect_fields WHERE seq=?1 AND name=?2",
                params![seq, field],
            )
            .map_err(error)?;
        match value {
            None => {}
            Some(Value::Array(items)) => {
                self.connection
                    .execute(
                        "INSERT INTO intersect_fields VALUES (?1,?2,1,NULL)",
                        params![seq, field],
                    )
                    .map_err(error)?;
                let mut insert = self
                    .connection
                    .prepare_cached("INSERT INTO intersect_items VALUES (?1,?2,?3,?4)")
                    .map_err(error)?;
                for (i, item) in items.iter().enumerate() {
                    let position = i64::try_from(i).map_err(error)?;
                    insert
                        .execute(params![seq, field, position, encode(item)?])
                        .map_err(error)?;
                    self.index_atom(seq, field, item)?;
                }
            }
            Some(value) => {
                self.connection
                    .execute(
                        "INSERT INTO intersect_fields VALUES (?1,?2,0,?3)",
                        params![seq, field, encode(&value)?],
                    )
                    .map_err(error)?;
                self.index_atom(seq, field, &value)?;
            }
        }
        Ok(())
    }
    /// Bounded collection reads do not materialize the containing object.
    pub fn collection_page(
        &self,
        id: &str,
        field: &str,
        offset: usize,
        limit: usize,
    ) -> StorageResult<Vec<Value>> {
        self.collection_page_observed(id, field, offset, limit, &InvocationContext::default())
    }
    pub fn collection_page_observed(
        &self,
        id: &str,
        field: &str,
        offset: usize,
        limit: usize,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<Vec<Value>> {
        self.read_collection_page(id, field, offset, limit, observation, None)
    }
    fn read_collection_page(
        &self,
        id: &str,
        field: &str,
        offset: usize,
        limit: usize,
        observation: &InvocationContext<'_>,
        mut budget: Option<&mut storage::JsonDecodeBudget>,
    ) -> StorageResult<Vec<Value>> {
        observation.checkpoint()?;
        let kind: Option<i64> = self.connection.query_row(
            "SELECT f.kind FROM intersect_fields f JOIN intersect_elements e ON e.seq=f.seq WHERE e.id=?1 AND f.name=?2",
            params![id, field], |r| r.get(0),
        ).optional().map_err(error)?;
        if kind != Some(1) {
            return Err("Multiple feature requires a list".into());
        }
        let Ok(offset) = i64::try_from(offset) else {
            return Ok(vec![]);
        };
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut stmt = self.connection.prepare_cached("SELECT i.value FROM intersect_items i JOIN intersect_elements e ON e.seq=i.seq WHERE e.id=?1 AND i.name=?2 AND i.position>=?3 ORDER BY i.position LIMIT ?4").map_err(error)?;
        let mut rows = stmt
            .query(params![id, field, offset, limit])
            .map_err(error)?;
        let mut page = Vec::new();
        loop {
            observation.checkpoint()?;
            let Some(row) = rows.next().map_err(error)? else {
                break;
            };
            observation.work(1)?;
            if observation.has_observer() {
                observation.retain_storage(32)?;
            }
            page.try_reserve(1)
                .map_err(|_| "SQLite value page allocation failed")?;
            let encoded = row.get_ref(0).map_err(error)?;
            let text = encoded.as_str().map_err(error)?;
            let value = match &mut budget {
                Some(budget) => budget.decode(text, observation),
                None => storage::decode_json_observed(text, observation),
            }?;
            page.push(value);
        }
        Ok(page)
    }
}
impl ModelRead for SqliteStore {
    fn resource_records(
        &self,
    ) -> StorageResult<
        std::collections::BTreeMap<String, intersect_model::core::resources::ResourceRecord>,
    > {
        let mut statement = self
            .connection
            .prepare_cached("SELECT uri,payload FROM intersect_resources ORDER BY uri")
            .map_err(error)?;
        let mut rows = statement.query([]).map_err(error)?;
        let mut records = std::collections::BTreeMap::new();
        while let Some(row) = rows.next().map_err(error)? {
            let uri: String = row.get(0).map_err(error)?;
            let payload: String = row.get(1).map_err(error)?;
            let record: intersect_model::core::resources::ResourceRecord =
                serde_json::from_str(&payload).map_err(error)?;
            if uri != record.uri {
                return Err("SQLite resource identity differs from its persisted key".into());
            }
            records.insert(uri, record);
        }
        Ok(records)
    }
    fn resource_record(
        &self,
        uri: &str,
    ) -> StorageResult<Option<intersect_model::core::resources::ResourceRecord>> {
        let payload: Option<String> = self
            .connection
            .query_row(
                "SELECT payload FROM intersect_resources WHERE uri=?1",
                [uri],
                |row| row.get(0),
            )
            .optional()
            .map_err(error)?;
        payload
            .map(|payload| {
                let record: intersect_model::core::resources::ResourceRecord =
                    serde_json::from_str(&payload).map_err(error)?;
                if uri != record.uri {
                    return Err("SQLite resource identity differs from its persisted key".into());
                }
                Ok(record)
            })
            .transpose()
    }
    fn collection_page(
        &self,
        id: &str,
        field: &str,
        offset: usize,
        limit: usize,
    ) -> StorageResult<Vec<Value>> {
        SqliteStore::collection_page(self, id, field, offset, limit)
    }
    fn collection_page_observed(
        &self,
        id: &str,
        field: &str,
        offset: usize,
        limit: usize,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<Vec<Value>> {
        SqliteStore::collection_page_observed(self, id, field, offset, limit, observation)
    }
    fn matches(&self, id: &str, query: Query<'_>) -> StorageResult<bool> {
        self.matches_observed(id, query, &InvocationContext::default())
    }
    fn matches_observed(
        &self,
        id: &str,
        query: Query<'_>,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<bool> {
        observation.checkpoint()?;
        let (kind, value) = storage::query_key_observed(query, observation)?;
        self.connection.query_row("SELECT EXISTS(SELECT 1 FROM intersect_indexes i JOIN intersect_elements e ON e.seq=i.seq WHERE e.id=?1 AND i.kind=?2 AND i.value=?3)", params![id,kind,value], |r| r.get(0)).map_err(error)
    }
    fn class(&self, id: &str) -> StorageResult<Option<String>> {
        self.connection.query_row("SELECT i.value FROM intersect_elements e JOIN intersect_indexes i ON i.seq=e.seq WHERE e.id=?1 AND i.kind='class'", [id], |r| r.get(0)).optional().map_err(error)
    }
    fn inspect_class(
        &self,
        id: &str,
        f: &mut dyn FnMut(&str) -> StorageResult<()>,
    ) -> StorageResult<bool> {
        let mut statement = self.connection.prepare_cached("SELECT i.value FROM intersect_elements e JOIN intersect_indexes i ON i.seq=e.seq WHERE e.id=?1 AND i.kind='class'").map_err(error)?;
        let mut rows = statement.query([id]).map_err(error)?;
        if let Some(row) = rows.next().map_err(error)? {
            let value = row.get_ref(0).map_err(error)?;
            f(value.as_str().map_err(error)?)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
    fn revision(&self) -> StorageResult<String> {
        self.connection
            .query_row(
                "SELECT value FROM intersect_metadata WHERE key='revision'",
                [],
                |r| r.get(0),
            )
            .map_err(error)
    }
    fn inspect_revision(&self, f: &mut dyn FnMut(&str) -> StorageResult<()>) -> StorageResult<()> {
        let mut statement = self
            .connection
            .prepare_cached("SELECT value FROM intersect_metadata WHERE key='revision'")
            .map_err(error)?;
        let mut rows = statement.query([]).map_err(error)?;
        let row = rows
            .next()
            .map_err(error)?
            .ok_or_else(|| error(rusqlite::Error::QueryReturnedNoRows))?;
        let value = row.get_ref(0).map_err(error)?;
        f(value.as_str().map_err(error)?)
    }
    fn get(&self, id: &str) -> StorageResult<Option<Element>> {
        let Some(mut element) = self.metadata(id)? else {
            return Ok(None);
        };
        let mut stmt = self.connection.prepare_cached("SELECT f.name FROM intersect_fields f JOIN intersect_elements e ON e.seq=f.seq WHERE e.id=?1 ORDER BY f.name").map_err(error)?;
        for field in stmt
            .query_map([id], |r| r.get::<_, String>(0))
            .map_err(error)?
        {
            let field = field.map_err(error)?;
            if let Some(value) = self.value(id, &field)? {
                element.values.insert(field, value);
            }
        }
        Ok(Some(element))
    }
    fn page(&self, after: Option<u64>, limit: usize) -> StorageResult<Vec<Row>> {
        self.id_page(after, limit)?
            .into_iter()
            .map(|(seq, id)| Ok((seq, self.get(&id)?.ok_or("Missing paged element")?)))
            .collect()
    }
    fn id_page(&self, after: Option<u64>, limit: usize) -> StorageResult<Vec<(u64, String)>> {
        self.id_page_observed(after, limit, &InvocationContext::default())
    }
    fn id_page_observed(
        &self,
        after: Option<u64>,
        limit: usize,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<Vec<(u64, String)>> {
        observation.checkpoint()?;
        let Ok(cursor) = i64::try_from(after.unwrap_or(0)) else {
            return Ok(vec![]);
        };
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut stmt = self
            .connection
            .prepare_cached(
                "SELECT seq,id FROM intersect_elements WHERE seq>?1 ORDER BY seq LIMIT ?2",
            )
            .map_err(error)?;
        let mut cursor_rows = stmt.query(params![cursor, limit]).map_err(error)?;
        let mut output = Vec::new();
        loop {
            observation.checkpoint()?;
            let Some(row) = cursor_rows.next().map_err(error)? else {
                break;
            };
            let identity = row.get_ref(1).map_err(error)?;
            storage::push_identity_observed(
                &mut output,
                row.get(0).map_err(error)?,
                identity.as_str().map_err(error)?,
                observation,
            )?;
        }
        Ok(output)
    }
    fn query_candidates(
        &self,
        query: Query<'_>,
        after: Option<u64>,
        limit: usize,
    ) -> StorageResult<QueryCandidates> {
        self.query_candidates_observed(query, after, limit, &InvocationContext::default())
    }
    fn query_candidates_observed(
        &self,
        query: Query<'_>,
        after: Option<u64>,
        limit: usize,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<QueryCandidates> {
        observation.checkpoint()?;
        if limit == 0 {
            return Err("Query candidate limit must be positive".into());
        }
        let bound = i64::try_from(limit)
            .ok()
            .and_then(|n| n.checked_add(1))
            .ok_or("Query candidate limit exceeds SQLite range")?;
        let Ok(cursor) = i64::try_from(after.unwrap_or(0)) else {
            return Ok(QueryCandidates {
                rows: vec![],
                after,
                exhausted: true,
                exact: true,
            });
        };
        let (kind, value) = storage::query_key_observed(query, observation)?;
        let mut statement = self.connection.prepare_cached(
            "SELECT e.seq,e.id FROM intersect_indexes i JOIN intersect_elements e ON e.seq=i.seq WHERE i.kind=?1 AND i.value=?2 AND i.seq>?3 ORDER BY i.seq LIMIT ?4"
        ).map_err(error)?;
        let mut cursor_rows = statement
            .query(params![kind, value, cursor, bound])
            .map_err(error)?;
        let mut rows = Vec::new();
        let exhausted = loop {
            observation.checkpoint()?;
            let Some(row) = cursor_rows.next().map_err(error)? else {
                break true;
            };
            if rows.len() == limit {
                break false;
            }
            let identity = row.get_ref(1).map_err(error)?;
            storage::push_identity_observed(
                &mut rows,
                row.get(0).map_err(error)?,
                identity.as_str().map_err(error)?,
                observation,
            )?;
        };
        Ok(QueryCandidates {
            after: rows.last().map(|(row, _)| *row).or(after),
            rows,
            exhausted,
            exact: true,
        })
    }
    fn query(&self, query: Query<'_>) -> StorageResult<Vec<String>> {
        let (kind, value) = query_key(query);
        let mut stmt = self.connection.prepare_cached("SELECT e.id FROM intersect_indexes i JOIN intersect_elements e ON e.seq=i.seq WHERE i.kind=?1 AND i.value=?2 ORDER BY i.seq").map_err(error)?;
        stmt.query_map(params![kind, value], |r| r.get(0))
            .map_err(error)?
            .map(|r| r.map_err(error))
            .collect()
    }
    fn value(&self, id: &str, field: &str) -> StorageResult<Option<Value>> {
        self.value_observed(id, field, &InvocationContext::default())
    }
    fn value_observed(
        &self,
        id: &str,
        field: &str,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<Option<Value>> {
        observation.checkpoint()?;
        let mut statement = self.connection.prepare_cached("SELECT f.kind,f.value FROM intersect_fields f JOIN intersect_elements e ON e.seq=f.seq WHERE e.id=?1 AND f.name=?2").map_err(error)?;
        let mut rows = statement.query(params![id, field]).map_err(error)?;
        let Some(row) = rows.next().map_err(error)? else {
            return Ok(None);
        };
        match row.get::<_, i64>(0).map_err(error)? {
            0 => {
                let encoded = row.get_ref(1).map_err(error)?;
                let Ok(text) = encoded.as_str() else {
                    return Err("Invalid persisted field representation".into());
                };
                storage::decode_json_observed(text, observation).map(Some)
            }
            1 => {
                drop(rows);
                drop(statement);
                let mut budget = storage::JsonDecodeBudget::array(observation)?;
                Ok(Some(Value::Array(self.read_collection_page(
                    id,
                    field,
                    0,
                    usize::MAX,
                    observation,
                    Some(&mut budget),
                )?)))
            }
            _ => Err("Invalid persisted field representation".into()),
        }
    }
    fn value_len(&self, id: &str, field: &str) -> StorageResult<Option<usize>> {
        let kind: Option<i64> = self.connection.query_row("SELECT f.kind FROM intersect_fields f JOIN intersect_elements e ON e.seq=f.seq WHERE e.id=?1 AND f.name=?2", params![id,field], |r| r.get(0)).optional().map_err(error)?;
        if kind != Some(1) {
            return Ok(None);
        }
        let n: i64 = self.connection.query_row("SELECT COUNT(*) FROM intersect_items i JOIN intersect_elements e ON e.seq=i.seq WHERE e.id=?1 AND i.name=?2", params![id,field], |r| r.get(0)).map_err(error)?;
        Ok(Some(usize::try_from(n).map_err(error)?))
    }
}
impl ModelStore for SqliteStore {
    fn set_resource_record(
        &mut self,
        record: intersect_model::core::resources::ResourceRecord,
    ) -> StorageResult<()> {
        self.active()?;
        self.connection.execute("INSERT INTO intersect_resources(uri,payload) VALUES (?1,?2) ON CONFLICT(uri) DO UPDATE SET payload=excluded.payload", params![record.uri, encode(&record)?]).map_err(error)?;
        Ok(())
    }
    fn remove_resource_record(&mut self, uri: &str) -> StorageResult<()> {
        self.active()?;
        self.connection
            .execute("DELETE FROM intersect_resources WHERE uri=?1", [uri])
            .map_err(error)?;
        Ok(())
    }
    fn is_transaction_active(&self) -> bool {
        !self.frames.is_empty()
    }
    fn begin(&mut self) -> StorageResult<Checkpoint> {
        let savepoint = self
            .next_savepoint
            .checked_add(1)
            .ok_or("Savepoint counter exhausted")?;
        let token = self.checkpoints.issue()?;
        self.connection
            .execute_batch(&format!("SAVEPOINT intersect_{savepoint}"))
            .map_err(error)?;
        self.next_savepoint = savepoint;
        self.frames.push(Frame {
            checkpoint: token.clone(),
            savepoint,
            schema: self.schema.clone(),
        });
        Ok(token)
    }
    fn commit(&mut self, token: &Checkpoint) -> StorageResult<()> {
        let savepoint = self.check(token)?;
        self.connection
            .execute_batch(&format!("RELEASE intersect_{savepoint}"))
            .map_err(error)?;
        self.frames.pop();
        Ok(())
    }
    fn rollback(&mut self, token: &Checkpoint) -> StorageResult<()> {
        let savepoint = self.check(token)?;
        self.connection
            .execute_batch(&format!(
                "ROLLBACK TO intersect_{savepoint}; RELEASE intersect_{savepoint}"
            ))
            .map_err(error)?;
        let frame = self.frames.pop().ok_or("Missing checkpoint frame")?;
        self.schema = frame.schema;
        Ok(())
    }
    fn configure_schema(
        &mut self,
        schema: &intersect_model::definitions::schema::Schema,
    ) -> StorageResult<()> {
        self.active()?;
        let revision = self.revision()?;
        if !revision.is_empty() && revision != schema.revision {
            return Err("Model belongs to a different schema revision".into());
        }
        storage::atomic(self, |store| {
            let initialized: String = store
                .connection
                .query_row(
                    "SELECT value FROM intersect_metadata WHERE key='resource_roots_initialized'",
                    [],
                    |row| row.get(0),
                )
                .map_err(error)?;
            if initialized != "true" {
                // An unbound raw producer cannot classify containment. Resolve
                // its provisional root lists exactly once when adopting schema.
                let mut elements = Vec::new();
                storage::visit(store, |element| {
                    elements.push(element);
                    Ok(())
                })?;
                let expected =
                    intersect_model::core::resources::records_for_elements(schema, &elements)?;
                for (uri, expected) in expected {
                    let mut record = store
                        .resource_record(&uri)?
                        .unwrap_or_else(|| expected.clone());
                    let expected_ids: std::collections::BTreeSet<_> =
                        expected.roots.iter().collect();
                    let mut roots: Vec<String> = record
                        .roots
                        .iter()
                        .filter(|id| expected_ids.contains(id))
                        .cloned()
                        .collect();
                    let present: std::collections::BTreeSet<_> = roots.iter().cloned().collect();
                    for id in expected.roots.iter() {
                        if !present.contains(id) {
                            roots.push(id.clone());
                        }
                    }
                    record.roots = std::sync::Arc::new(roots);
                    store.set_resource_record(record)?;
                }
                store.connection.execute("UPDATE intersect_metadata SET value='true' WHERE key='resource_roots_initialized'", []).map_err(error)?;
            }
            store.set_revision(&schema.revision)?;
            if store
                .schema
                .as_ref()
                .is_none_or(|current| current.revision != schema.revision)
            {
                store.schema = Some(std::sync::Arc::new(schema.clone()));
            }
            Ok(())
        })
    }
    fn set_revision(&mut self, revision: &str) -> StorageResult<()> {
        self.active()?;
        self.connection
            .execute(
                "UPDATE intersect_metadata SET value=?1 WHERE key='revision'",
                [revision],
            )
            .map_err(error)?;
        if self
            .schema
            .as_ref()
            .is_some_and(|schema| schema.revision != revision)
        {
            self.schema = None;
        }
        Ok(())
    }
    fn put(&mut self, mut element: Element) -> StorageResult<()> {
        self.active()?;
        if element.id.is_empty() {
            return Err("Empty model identity".into());
        }
        storage::atomic(self, |s| {
            let before = s.get(&element.id)?;
            if before.as_ref() == Some(&element) {
                return Ok(());
            }
            let schema = s.schema.clone();
            intersect_model::core::resources::element_delta(
                s,
                schema.as_deref(),
                before.as_ref(),
                Some(&element),
            )?;
            let fields = std::mem::take(&mut element.values);
            s.connection.execute("INSERT INTO intersect_elements(id,payload) VALUES (?1,?2) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload", params![element.id,encode(&element)?]).map_err(error)?;
            let seq = s.sequence(&element.id)?;
            s.connection
                .execute("DELETE FROM intersect_indexes WHERE seq=?1", [seq])
                .map_err(error)?;
            s.connection
                .execute("DELETE FROM intersect_fields WHERE seq=?1", [seq])
                .map_err(error)?;
            for (kind, value) in [("class", &element.class), ("resource", &element.resource)] {
                s.connection
                    .execute(
                        "INSERT INTO intersect_indexes VALUES (?1,?2,?3)",
                        params![kind, value, seq],
                    )
                    .map_err(error)?;
            }
            for (field, value) in fields {
                s.write_field(seq, &field, Some(value))?;
            }
            Ok(())
        })
    }
    fn remove(&mut self, id: &str) -> StorageResult<()> {
        self.active()?;
        storage::atomic(self, |s| {
            let before = s.get(id)?;
            let schema = s.schema.clone();
            intersect_model::core::resources::element_delta(
                s,
                schema.as_deref(),
                before.as_ref(),
                None,
            )?;
            s.connection
                .execute("DELETE FROM intersect_elements WHERE id=?1", [id])
                .map_err(error)?;
            Ok(())
        })
    }
    fn set_value(&mut self, id: &str, field: &str, value: Option<Value>) -> StorageResult<()> {
        self.active()?;
        storage::atomic(self, |s| {
            let metadata = s.metadata(id)?.ok_or("Missing element")?;
            let before = s.value(id, field)?;
            if before != value {
                let containment = s.is_containment(&metadata, field);
                intersect_model::core::resources::field_delta(
                    s,
                    &metadata.resource,
                    containment,
                    before.as_ref(),
                    value.as_ref(),
                )?;
            }
            s.write_field(s.sequence(id)?, field, value)
        })
    }
    fn append_value(&mut self, id: &str, field: &str, value: Value) -> StorageResult<()> {
        self.active()?;
        storage::atomic(self, |s| {
            let metadata = s.metadata(id)?.ok_or("Missing element")?;
            intersect_model::core::resources::field_delta(
                s,
                &metadata.resource,
                s.is_containment(&metadata, field),
                None,
                Some(&value),
            )?;
            let seq = s.sequence(id)?;
            let kind: Option<i64> = s
                .connection
                .query_row(
                    "SELECT kind FROM intersect_fields WHERE seq=?1 AND name=?2",
                    params![seq, field],
                    |r| r.get(0),
                )
                .optional()
                .map_err(error)?;
            match kind {
                None => {
                    s.connection
                        .execute(
                            "INSERT INTO intersect_fields VALUES (?1,?2,1,NULL)",
                            params![seq, field],
                        )
                        .map_err(error)?;
                }
                Some(1) => {}
                _ => return Err("Expected list".into()),
            }
            let last: Option<i64> = s
                .connection
                .query_row(
                    "SELECT MAX(position) FROM intersect_items WHERE seq=?1 AND name=?2",
                    params![seq, field],
                    |r| r.get(0),
                )
                .map_err(error)?;
            let next = last.map_or(Ok(0), |v| {
                v.checked_add(1).ok_or("Collection position exhausted")
            })?;
            s.connection
                .execute(
                    "INSERT INTO intersect_items VALUES (?1,?2,?3,?4)",
                    params![seq, field, next, encode(&value)?],
                )
                .map_err(error)?;
            s.index_atom(seq, field, &value)
        })
    }
    fn truncate_values(&mut self, id: &str, field: &str, length: usize) -> StorageResult<()> {
        self.active()?;
        let current = self.value_len(id, field)?.ok_or("Expected list")?;
        if length > current {
            return Err("Cannot extend a collection by truncating it".into());
        }
        let length = i64::try_from(length).map_err(error)?;
        storage::atomic(self, |s| {
            if current != usize::try_from(length).map_err(error)? {
                let metadata = s.metadata(id)?.ok_or("Missing element")?;
                let containment = s.is_containment(&metadata, field);
                let before = if containment {
                    Some(Value::Array(s.collection_page(
                        id,
                        field,
                        usize::try_from(length).map_err(error)?,
                        usize::MAX,
                    )?))
                } else {
                    None
                };
                intersect_model::core::resources::field_delta(
                    s,
                    &metadata.resource,
                    containment,
                    before.as_ref(),
                    None,
                )?;
            }
            let seq = s.sequence(id)?;
            let kind: Option<i64> = s
                .connection
                .query_row(
                    "SELECT kind FROM intersect_fields WHERE seq=?1 AND name=?2",
                    params![seq, field],
                    |r| r.get(0),
                )
                .optional()
                .map_err(error)?;
            if kind != Some(1) {
                return Err("Expected list".into());
            }
            s.connection
                .execute(
                    "DELETE FROM intersect_items WHERE seq=?1 AND name=?2 AND position>=?3",
                    params![seq, field, length],
                )
                .map_err(error)?;
            // Retain postings while at least one duplicate still exists.
            s.connection.execute("DELETE FROM intersect_indexes WHERE seq=?1 AND kind=?2 AND NOT EXISTS (SELECT 1 FROM intersect_items WHERE seq=?1 AND name=?3 AND value=intersect_indexes.value)",params![seq,format!("value:{field}"),field]).map_err(error)?;
            Ok(())
        })
    }
    fn set_span(&mut self, id: &str, start: usize, end: usize) -> StorageResult<()> {
        self.active()?;
        storage::atomic(self, |s| {
            let mut element = s.metadata(id)?.ok_or("Missing element")?;
            if element.start != start || element.end != end {
                intersect_model::core::resources::touch_resource(s, &element.resource)?;
                element.start = start;
                element.end = end;
                s.connection
                    .execute(
                        "UPDATE intersect_elements SET payload=?1 WHERE id=?2",
                        params![encode(&element)?, id],
                    )
                    .map_err(error)?;
            }
            Ok(())
        })
    }
    fn resolve_reference(
        &mut self,
        id: &str,
        field: &str,
        index: Option<usize>,
        target: &str,
    ) -> StorageResult<()> {
        self.active()?;
        let Some(index) = index else {
            return self.set_value(id, field, Some(Value::String(target.into())));
        };
        let index = i64::try_from(index).map_err(error)?;
        storage::atomic(self, |s| {
            let seq = s.sequence(id)?;
            let old: String = s
                .connection
                .query_row(
                    "SELECT value FROM intersect_items WHERE seq=?1 AND name=?2 AND position=?3",
                    params![seq, field, index],
                    |r| r.get(0),
                )
                .map_err(error)?;
            let value = Value::String(target.into());
            let previous: Value = serde_json::from_str(&old).map_err(error)?;
            if previous != value {
                let metadata = s.metadata(id)?.ok_or("Missing element")?;
                intersect_model::core::resources::field_delta(
                    s,
                    &metadata.resource,
                    s.is_containment(&metadata, field),
                    Some(&previous),
                    Some(&value),
                )?;
            }
            s.connection
                .execute(
                    "UPDATE intersect_items SET value=?1 WHERE seq=?2 AND name=?3 AND position=?4",
                    params![encode(&value)?, seq, field, index],
                )
                .map_err(error)?;
            s.connection.execute("DELETE FROM intersect_indexes WHERE seq=?1 AND kind=?2 AND value=?3 AND NOT EXISTS (SELECT 1 FROM intersect_items WHERE seq=?1 AND name=?4 AND value=?3)",params![seq,format!("value:{field}"),old,field]).map_err(error)?;
            s.index_atom(seq, field, &value)
        })
    }
}

#[cfg(test)]
mod resource_tests {
    use super::*;
    use intersect_model::core::resources::{
        ResourceLoadState, ResourceRecord, SourceCorrespondence,
    };
    use std::sync::Arc;

    #[test]
    fn raw_sqlite_adoption_initializes_containment_roots_once_and_rolls_back() -> StorageResult<()>
    {
        use intersect_model::definitions::dynamic::SchemaBuilder;
        let mut builder = SchemaBuilder::new("nodes", "urn:nodes");
        let node = builder.class("Node")?;
        node.reference("children", &node).many().containment()?;
        let schema = builder.build()?.as_schema().clone();
        let mut parent = Element::new(&schema, "parent", "Node", "catalog", 0, 0)?;
        parent
            .values
            .insert("children".into(), serde_json::json!(["child"]));
        let child = Element::new(&schema, "child", "Node", "catalog", 0, 0)?;
        let sibling = Element::new(&schema, "sibling", "Node", "catalog", 0, 0)?;
        let mut store = SqliteStore::in_memory()?;
        storage::atomic(&mut store, |store| {
            store.put(parent)?;
            store.put(child)?;
            store.put(sibling)
        })?;
        let provisional = storage::export(&store)?;
        let outer = store.begin()?;
        store.configure_schema(&schema)?;
        assert_eq!(
            store
                .resource_record("catalog")?
                .ok_or("catalog")?
                .roots
                .as_slice(),
            ["parent", "sibling"]
        );
        store.rollback(&outer)?;
        assert_eq!(storage::export(&store)?, provisional);
        storage::atomic(&mut store, |store| store.configure_schema(&schema))?;
        intersect_model::core::resources::reorder_roots(
            &mut store,
            "catalog",
            &["sibling".into(), "parent".into()],
        )?;
        let initialized = storage::export(&store)?;
        storage::atomic(&mut store, |store| store.configure_schema(&schema))?;
        assert_eq!(storage::export(&store)?, initialized);
        storage::validate(&store, &schema)?;
        Ok(())
    }

    #[test]
    fn bound_sqlite_mutations_keep_roots_revisions_and_failed_writes_atomic() -> StorageResult<()> {
        use intersect_model::definitions::dynamic::{ScalarType, SchemaBuilder};
        let mut builder = SchemaBuilder::new("nodes", "urn:nodes");
        let node = builder.class("Node")?;
        node.attribute("name", ScalarType::String).default("")?;
        node.reference("children", &node).many().containment()?;
        let schema = builder.build()?.as_schema().clone();
        let mut store = SqliteStore::in_memory()?;
        storage::atomic(&mut store, |store| {
            store.create_element(&schema, "parent", "Node", "catalog", 0, 0)?;
            store.create_element(&schema, "child", "Node", "catalog", 0, 0)?;
            store.append_value("parent", "children", Value::String("child".into()))
        })?;
        assert_eq!(
            store
                .resource_record("catalog")?
                .ok_or("catalog")?
                .roots
                .as_slice(),
            ["parent"]
        );
        storage::validate(&store, &schema)?;
        let original = storage::export(&store)?;
        let outer = store.begin()?;
        let inner = store.begin()?;
        store.truncate_values("parent", "children", 0)?;
        assert_eq!(
            store
                .resource_record("catalog")?
                .ok_or("catalog")?
                .roots
                .as_slice(),
            ["parent", "child"]
        );
        store.commit(&inner)?;
        store.rollback(&outer)?;
        assert_eq!(storage::export(&store)?, original);
        let outer = store.begin()?;
        assert!(
            store
                .append_value("parent", "name", Value::String("invalid".into()))
                .is_err()
        );
        assert_eq!(storage::export(&store)?, original);
        store.rollback(&outer)?;
        storage::atomic(&mut store, |store| {
            store.set_value("parent", "name", Some(Value::String("edited".into())))
        })?;
        assert!(
            store
                .resource_record("catalog")?
                .ok_or("catalog")?
                .content_revision
                > original.resources["catalog"].content_revision
        );
        Ok(())
    }

    #[test]
    fn empty_resources_and_metadata_survive_reopen_and_nested_rollback() -> StorageResult<()> {
        let path = std::env::temp_dir().join(format!(
            "intersect-resource-lifecycle-{}.sqlite",
            std::process::id()
        ));
        if path.exists() {
            return Err("Resource test database already exists".into());
        }
        let mut record = ResourceRecord::new("catalog")?;
        record.load_state = ResourceLoadState::Failed;
        record.diagnostics.push("source unavailable".into());
        record.source = Some(SourceCorrespondence {
            uri: "catalog.nodes".into(),
            revision: "source-1".into(),
        });
        {
            let mut store = SqliteStore::open(&path)?;
            assert!(store.set_resource_record(record.clone()).is_err());
            storage::atomic(&mut store, |store| {
                store.set_resource_record(record.clone())
            })?;
            let outer = store.begin()?;
            let mut changed = record.clone();
            changed.roots = Arc::new(vec!["b".into(), "a".into()]);
            changed.touch()?;
            store.set_resource_record(changed.clone())?;
            assert_eq!(store.resource_record("catalog")?, Some(changed));
            let inner = store.begin()?;
            store.remove_resource_record("catalog")?;
            assert!(store.resource_records()?.is_empty());
            store.commit(&inner)?;
            store.rollback(&outer)?;
            assert_eq!(store.resource_record("catalog")?, Some(record.clone()));
        }
        let store = SqliteStore::open(&path)?;
        assert_eq!(store.resource_record("catalog")?, Some(record));
        assert_eq!(storage::export(&store)?.resources.len(), 1);
        drop(store);
        std::fs::remove_file(path).map_err(error)?;
        Ok(())
    }
}
