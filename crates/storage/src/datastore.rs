//! Generic Schemaless Document Store and Asset Ingestion Pipeline.
//!
//! Provides in-memory, wait-free document indexing (JSON and MessagePack)
//! with recursive directory loading, collection isolation, and dynamic
//! entity property population without engine recompilation.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwap;
use mmo_core::{DynamicProperties, Entity, PropertyValue};
use serde::{Deserialize, Serialize};

/// An ingested schemaless static asset document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataDocument {
    pub id: String,
    pub collection: String,
    pub properties: DynamicProperties,
}

impl DataDocument {
    /// Creates a new `DataDocument`.
    pub fn new(
        collection: impl Into<String>,
        id: impl Into<String>,
        properties: DynamicProperties,
    ) -> Self {
        Self {
            id: id.into(),
            collection: collection.into(),
            properties,
        }
    }

    /// Retrieves an immutable reference to the property value associated with the specified key.
    pub fn get(&self, key: &str) -> Option<&PropertyValue> {
        self.properties.get(key)
    }

    /// Navigates a dotted path (e.g. `"combat.element.fire"`) through nested objects.
    pub fn get_path(&self, dot_path: &str) -> Option<&PropertyValue> {
        self.properties.get_path(dot_path)
    }

    /// Helper to get a string property slice if present and of string type.
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.properties.get_str(key)
    }

    /// Helper to get an integer property value if present.
    pub fn get_i64(&self, key: &str) -> Option<i64> {
        self.properties.get_i64(key)
    }

    /// Helper to get a float property value (with integer auto-coercion) if present.
    pub fn get_f64(&self, key: &str) -> Option<f64> {
        self.properties.get_f64(key)
    }

    /// Helper to get a boolean property value if present.
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.properties.get_bool(key)
    }
}

/// Errors occurring during document store ingestion or query.
#[derive(Debug, thiserror::Error)]
pub enum DataStoreError {
    #[error("I/O error at '{path}': {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("JSON deserialization error in '{path}': {source}")]
    JsonParse {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("MessagePack deserialization error in '{path}': {source}")]
    MsgPackParse {
        path: String,
        #[source]
        source: rmp_serde::decode::Error,
    },
    #[error("Unsupported file extension for '{path}'")]
    UnsupportedExtension { path: String },
    #[error("Document missing required 'id' attribute in '{path}'")]
    MissingId { path: String },
    #[error("Document not found: collection '{collection}', id '{id}'")]
    DocumentNotFound { collection: String, id: String },
    #[error("Invalid document structure in '{path}': {message}")]
    InvalidStructure { path: String, message: String },
}

/// Ingestion report tracking successfully loaded document count and non-fatal errors.
#[derive(Debug, Default)]
pub struct LoadReport {
    pub loaded: usize,
    pub errors: Vec<DataStoreError>,
}

impl LoadReport {
    /// Returns `true` if ingestion completed with zero errors.
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// In-memory snapshot of all collections and documents.
#[derive(Debug, Default, Clone)]
struct DataStoreSnapshot {
    // collection -> (id -> Arc<DataDocument>)
    collections: HashMap<String, HashMap<String, Arc<DataDocument>>>,
}

/// Thread-safe, wait-free in-memory schemaless document repository.
#[derive(Debug, Clone)]
pub struct DataStore {
    snapshot: Arc<ArcSwap<DataStoreSnapshot>>,
    write_lock: Arc<Mutex<()>>,
}

impl Default for DataStore {
    fn default() -> Self {
        Self::new()
    }
}

impl DataStore {
    /// Constructs a new empty `DataStore`.
    pub fn new() -> Self {
        Self {
            snapshot: Arc::new(ArcSwap::from_pointee(DataStoreSnapshot::default())),
            write_lock: Arc::new(Mutex::new(())),
        }
    }

    /// Retrieves an `Arc` reference to a document by collection and ID in wait-free O(1) time.
    pub fn get(&self, collection: &str, id: &str) -> Option<Arc<DataDocument>> {
        let snap = self.snapshot.load();
        snap.collections
            .get(collection)
            .and_then(|col| col.get(id).cloned())
    }

    /// Returns `true` if the specified collection contains a document with the given ID.
    pub fn contains(&self, collection: &str, id: &str) -> bool {
        let snap = self.snapshot.load();
        snap.collections
            .get(collection)
            .map(|col| col.contains_key(id))
            .unwrap_or(false)
    }

    /// Returns all documents belonging to a specified collection.
    pub fn get_collection(&self, collection: &str) -> Vec<Arc<DataDocument>> {
        let snap = self.snapshot.load();
        snap.collections
            .get(collection)
            .map(|col| col.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Returns the names of all indexed collections.
    pub fn list_collections(&self) -> Vec<String> {
        let snap = self.snapshot.load();
        snap.collections.keys().cloned().collect()
    }

    /// Returns the total count of documents stored across all collections.
    pub fn count(&self) -> usize {
        let snap = self.snapshot.load();
        snap.collections.values().map(|col| col.len()).sum()
    }

    /// Returns the count of documents in a specific collection.
    pub fn collection_count(&self, collection: &str) -> usize {
        let snap = self.snapshot.load();
        snap.collections
            .get(collection)
            .map(|col| col.len())
            .unwrap_or(0)
    }

    /// Directly inserts or updates a `DataDocument`.
    pub fn insert_document(&self, doc: DataDocument) {
        let _guard = self.write_lock.lock().unwrap();
        let mut new_snap = (**self.snapshot.load()).clone();
        let col = new_snap
            .collections
            .entry(doc.collection.clone())
            .or_default();
        col.insert(doc.id.clone(), Arc::new(doc));
        self.snapshot.store(Arc::new(new_snap));
    }

    /// Atomically removes a document from the store, returning the removed instance if present.
    pub fn remove_document(&self, collection: &str, id: &str) -> Option<Arc<DataDocument>> {
        let _guard = self.write_lock.lock().unwrap();
        let mut new_snap = (**self.snapshot.load()).clone();
        let removed = if let Some(col) = new_snap.collections.get_mut(collection) {
            let rem = col.remove(id);
            if col.is_empty() {
                new_snap.collections.remove(collection);
            }
            rem
        } else {
            None
        };

        if removed.is_some() {
            self.snapshot.store(Arc::new(new_snap));
        }
        removed
    }

    /// Clears all documents and collections from the store.
    pub fn clear(&self) {
        let _guard = self.write_lock.lock().unwrap();
        self.snapshot.store(Arc::new(DataStoreSnapshot::default()));
    }

    /// Attaches the dynamic properties from the specified document to an `Entity`.
    pub fn apply_to_entity(
        &self,
        collection: &str,
        id: &str,
        entity: &mut Entity,
    ) -> Result<(), DataStoreError> {
        let doc = self
            .get(collection, id)
            .ok_or_else(|| DataStoreError::DocumentNotFound {
                collection: collection.to_string(),
                id: id.to_string(),
            })?;

        entity.properties.merge(&doc.properties);
        Ok(())
    }

    /// Ingests a single document directly from a JSON string.
    pub fn load_from_json_str(
        &self,
        collection: &str,
        id: &str,
        json_str: &str,
    ) -> Result<(), DataStoreError> {
        let val: serde_json::Value =
            serde_json::from_str(json_str).map_err(|source| DataStoreError::JsonParse {
                path: format!("{}/{}", collection, id),
                source,
            })?;

        let properties = DynamicProperties::from(val);
        self.insert_document(DataDocument::new(collection, id, properties));
        Ok(())
    }

    /// Ingests a single document directly from MessagePack binary bytes.
    pub fn load_from_msgpack_bytes(
        &self,
        collection: &str,
        id: &str,
        bytes: &[u8],
    ) -> Result<(), DataStoreError> {
        let val: serde_json::Value =
            rmp_serde::from_slice(bytes).map_err(|source| DataStoreError::MsgPackParse {
                path: format!("{}/{}", collection, id),
                source,
            })?;

        let properties = DynamicProperties::from(val);
        self.insert_document(DataDocument::new(collection, id, properties));
        Ok(())
    }

    /// Ingests asset documents from a single file on disk (.json or .mpk/.msgpack).
    ///
    /// Supports:
    /// - A single JSON/MessagePack object (uses `"id"` or the file stem).
    /// - An array of objects (each must contain an `"id"`).
    /// - An object map whose values are sub-objects (keys are IDs).
    pub fn load_file(&self, path: impl AsRef<Path>) -> Result<usize, DataStoreError> {
        let path = path.as_ref();
        let path_str = path.to_string_lossy().to_string();

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        let file_stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("document")
            .to_string();

        let default_collection = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("default")
            .to_string();

        let raw_bytes = fs::read(path).map_err(|source| DataStoreError::Io {
            path: path_str.clone(),
            source,
        })?;

        let parsed_value: serde_json::Value = match ext.as_str() {
            "json" => {
                serde_json::from_slice(&raw_bytes).map_err(|source| DataStoreError::JsonParse {
                    path: path_str.clone(),
                    source,
                })?
            }
            "mpk" | "msgpack" => rmp_serde::from_slice(&raw_bytes).map_err(|source| {
                DataStoreError::MsgPackParse {
                    path: path_str.clone(),
                    source,
                }
            })?,
            _ => {
                return Err(DataStoreError::UnsupportedExtension { path: path_str });
            }
        };

        let docs =
            Self::extract_documents(parsed_value, &default_collection, &file_stem, &path_str)?;
        let count = docs.len();

        let _guard = self.write_lock.lock().unwrap();
        let mut new_snap = (**self.snapshot.load()).clone();
        for doc in docs {
            let col = new_snap
                .collections
                .entry(doc.collection.clone())
                .or_default();
            col.insert(doc.id.clone(), Arc::new(doc));
        }
        self.snapshot.store(Arc::new(new_snap));

        Ok(count)
    }

    /// Recursively scans a directory tree, parsing all valid `.json`, `.mpk`, and `.msgpack` files.
    ///
    /// Non-fatal file parse errors are accumulated in `LoadReport` without halting ingestion
    /// of valid documents.
    pub fn load_directory(&self, dir_path: impl AsRef<Path>) -> LoadReport {
        let mut report = LoadReport::default();
        let mut file_paths = Vec::new();
        Self::collect_files_recursive(dir_path.as_ref(), &mut file_paths);

        for path in file_paths {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();

            if ext == "json" || ext == "mpk" || ext == "msgpack" {
                match self.load_file(&path) {
                    Ok(n) => report.loaded += n,
                    Err(err) => report.errors.push(err),
                }
            }
        }

        report
    }

    fn collect_files_recursive(dir: &Path, files: &mut Vec<std::path::PathBuf>) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    Self::collect_files_recursive(&p, files);
                } else if p.is_file() {
                    files.push(p);
                }
            }
        }
    }

    fn extract_documents(
        val: serde_json::Value,
        default_collection: &str,
        file_stem: &str,
        path_str: &str,
    ) -> Result<Vec<DataDocument>, DataStoreError> {
        let mut docs = Vec::new();

        match val {
            serde_json::Value::Array(arr) => {
                for (idx, item) in arr.into_iter().enumerate() {
                    if let serde_json::Value::Object(map) = item {
                        let id = map
                            .get("id")
                            .or_else(|| map.get("_id"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| format!("{}_{}", file_stem, idx));

                        let collection = map
                            .get("collection")
                            .and_then(|v| v.as_str())
                            .unwrap_or(default_collection)
                            .to_string();

                        let props = DynamicProperties::from(serde_json::Value::Object(map));
                        docs.push(DataDocument::new(collection, id, props));
                    } else {
                        return Err(DataStoreError::InvalidStructure {
                            path: path_str.to_string(),
                            message: format!("Array entry at index {} is not a JSON object", idx),
                        });
                    }
                }
            }
            serde_json::Value::Object(map) => {
                let doc_id = map
                    .get("id")
                    .or_else(|| map.get("_id"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| file_stem.to_string());

                let collection = map
                    .get("collection")
                    .and_then(|v| v.as_str())
                    .unwrap_or(default_collection)
                    .to_string();

                let props = DynamicProperties::from(serde_json::Value::Object(map));
                docs.push(DataDocument::new(collection, doc_id, props));
            }
            _ => {
                return Err(DataStoreError::InvalidStructure {
                    path: path_str.to_string(),
                    message: "Top-level asset content must be a JSON object or array".to_string(),
                });
            }
        }

        Ok(docs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_core::{EntityType, Guid};

    #[test]
    fn test_datastore_in_memory_crud_and_query() {
        let store = DataStore::new();
        assert_eq!(store.count(), 0);
        assert!(store.list_collections().is_empty());

        let mut props = DynamicProperties::new();
        props.set("durability", 200);
        props.set("name", "Excalibur");

        let doc = DataDocument::new("weapons", "excalibur", props);
        store.insert_document(doc);

        assert_eq!(store.count(), 1);
        assert_eq!(store.collection_count("weapons"), 1);
        assert_eq!(store.collection_count("armors"), 0);
        assert!(store.contains("weapons", "excalibur"));
        assert!(!store.contains("weapons", "missing"));

        let retrieved = store.get("weapons", "excalibur").expect("doc exists");
        assert_eq!(retrieved.get_str("name"), Some("Excalibur"));
        assert_eq!(retrieved.get_i64("durability"), Some(200));

        let removed = store.remove_document("weapons", "excalibur");
        assert!(removed.is_some());
        assert_eq!(store.count(), 0);
        assert!(!store.contains("weapons", "excalibur"));
    }

    #[test]
    fn test_collection_isolation_prevents_id_collisions() {
        let store = DataStore::new();

        let mut item_props = DynamicProperties::new();
        item_props.set("kind", "item");
        store.insert_document(DataDocument::new("items", "101", item_props));

        let mut monster_props = DynamicProperties::new();
        monster_props.set("kind", "monster");
        store.insert_document(DataDocument::new("monsters", "101", monster_props));

        assert_eq!(store.count(), 2);
        assert_eq!(
            store.get("items", "101").unwrap().get_str("kind"),
            Some("item")
        );
        assert_eq!(
            store.get("monsters", "101").unwrap().get_str("kind"),
            Some("monster")
        );
    }

    #[test]
    fn test_data_document_get_bool() {
        let mut props = DynamicProperties::new();
        props.set("flag_true", true);
        props.set("flag_false", false);
        props.set("not_a_bool", "hello");

        let doc = DataDocument::new("settings", "config_01", props);
        assert_eq!(doc.get_bool("flag_true"), Some(true));
        assert_eq!(doc.get_bool("flag_false"), Some(false));
        assert_eq!(doc.get_bool("not_a_bool"), None);
        assert_eq!(doc.get_bool("nonexistent"), None);
    }

    #[test]
    fn test_datastore_collections_and_clear() {
        let store = DataStore::new();

        // Initially empty
        assert!(store.list_collections().is_empty());
        assert_eq!(store.count(), 0);
        assert!(store.get_collection("items").is_empty());

        let mut p1 = DynamicProperties::new();
        p1.set("val", 1);
        let mut p2 = DynamicProperties::new();
        p2.set("val", 2);
        let mut p3 = DynamicProperties::new();
        p3.set("val", 3);

        store.insert_document(DataDocument::new("items", "item_a", p1));
        store.insert_document(DataDocument::new("items", "item_b", p2));
        store.insert_document(DataDocument::new("monsters", "boss_1", p3));

        assert_eq!(store.count(), 3);
        assert_eq!(store.collection_count("items"), 2);
        assert_eq!(store.collection_count("monsters"), 1);
        assert_eq!(store.collection_count("empty_col"), 0);

        let mut cols = store.list_collections();
        cols.sort();
        assert_eq!(cols, vec!["items".to_string(), "monsters".to_string()]);

        let items = store.get_collection("items");
        assert_eq!(items.len(), 2);
        let item_ids: std::collections::HashSet<String> =
            items.iter().map(|d| d.id.clone()).collect();
        assert!(item_ids.contains("item_a"));
        assert!(item_ids.contains("item_b"));

        let monsters = store.get_collection("monsters");
        assert_eq!(monsters.len(), 1);
        assert_eq!(monsters[0].id, "boss_1");

        // Clear datastore
        store.clear();
        assert_eq!(store.count(), 0);
        assert!(store.list_collections().is_empty());
        assert!(store.get_collection("items").is_empty());
        assert!(store.get_collection("monsters").is_empty());
        assert_eq!(store.collection_count("items"), 0);
    }

    #[test]
    fn test_apply_to_entity() {
        let store = DataStore::new();

        let mut props = DynamicProperties::new();
        props.set("fire_resistance", 0.75);
        props.set("max_shield", 500);
        store.insert_document(DataDocument::new("buffs", "fire_shield", props));

        let mut entity = Entity::new(Guid::new(EntityType::Player, 1, 1), (0.0, 0.0, 0.0));
        assert_eq!(entity.get_property("fire_resistance"), None);

        let res = store.apply_to_entity("buffs", "fire_shield", &mut entity);
        assert!(res.is_ok());

        assert_eq!(entity.get_property_f64("fire_resistance"), Some(0.75));
        assert_eq!(entity.get_property_i64("max_shield"), Some(500));

        // Missing document returns error
        let err = store.apply_to_entity("buffs", "non_existent", &mut entity);
        assert!(matches!(err, Err(DataStoreError::DocumentNotFound { .. })));
    }

    #[test]
    fn test_json_and_msgpack_string_and_bytes_loading() {
        let store = DataStore::new();

        // 1. JSON string loading
        let json_str = r#"{"attack": 45, "tier": "legendary"}"#;
        store
            .load_from_json_str("weapons", "bow_01", json_str)
            .unwrap();

        let bow = store.get("weapons", "bow_01").unwrap();
        assert_eq!(bow.get_i64("attack"), Some(45));
        assert_eq!(bow.get_str("tier"), Some("legendary"));

        // 2. MessagePack bytes loading
        let mut val_map = serde_json::Map::new();
        val_map.insert("speed".to_string(), serde_json::json!(8.5));
        val_map.insert("name".to_string(), serde_json::json!("Swift Wolf"));
        let val = serde_json::Value::Object(val_map);
        let msgpack_bytes = rmp_serde::to_vec(&val).unwrap();

        store
            .load_from_msgpack_bytes("npcs", "wolf_01", &msgpack_bytes)
            .unwrap();
        let wolf = store.get("npcs", "wolf_01").unwrap();
        assert_eq!(wolf.get_f64("speed"), Some(8.5));
        assert_eq!(wolf.get_str("name"), Some("Swift Wolf"));
    }

    #[test]
    fn test_file_and_directory_ingestion_with_error_accumulation() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();

        let items_dir = root.join("items");
        let monsters_dir = root.join("monsters");
        fs::create_dir_all(&items_dir).unwrap();
        fs::create_dir_all(&monsters_dir).unwrap();

        // 1. Single document JSON file (falls back to file stem for ID)
        let sword_json = r#"{"damage": 25, "rarity": "rare"}"#;
        fs::write(items_dir.join("iron_sword.json"), sword_json).unwrap();

        // 2. Array of documents JSON file with explicit IDs
        let potions_json = r#"[
            {"id": "minor_heal", "heal_amount": 50},
            {"id": "major_heal", "heal_amount": 150}
        ]"#;
        fs::write(items_dir.join("potions.JSON"), potions_json).unwrap(); // Test case-insensitive .JSON

        // 3. MessagePack file
        let mut goblin_map = serde_json::Map::new();
        goblin_map.insert("id".to_string(), serde_json::json!("goblin_scout"));
        goblin_map.insert("hp".to_string(), serde_json::json!(60));
        let goblin_bytes = rmp_serde::to_vec(&serde_json::Value::Object(goblin_map)).unwrap();
        fs::write(monsters_dir.join("goblin.msgpack"), goblin_bytes).unwrap();

        // 4. Corrupted file (should be caught and recorded in LoadReport without aborting)
        fs::write(monsters_dir.join("corrupted.json"), "{ invalid json syntax").unwrap();

        // 5. Ingest entire directory tree
        let store = DataStore::new();
        let report = store.load_directory(root);

        assert_eq!(report.loaded, 4); // iron_sword, minor_heal, major_heal, goblin_scout
        assert_eq!(report.errors.len(), 1); // corrupted.json error
        assert!(!report.is_ok());

        // Verify loaded items
        let sword = store.get("items", "iron_sword").expect("iron_sword loaded");
        assert_eq!(sword.get_i64("damage"), Some(25));
        assert_eq!(sword.get_str("rarity"), Some("rare"));

        let minor_heal = store.get("items", "minor_heal").expect("minor_heal loaded");
        assert_eq!(minor_heal.get_i64("heal_amount"), Some(50));

        let major_heal = store.get("items", "major_heal").expect("major_heal loaded");
        assert_eq!(major_heal.get_i64("heal_amount"), Some(150));

        let goblin = store
            .get("monsters", "goblin_scout")
            .expect("goblin_scout loaded");
        assert_eq!(goblin.get_i64("hp"), Some(60));
    }
}
