use std::cell::Cell;

use leveldb::database::Database as LevelDataBase;
use std::sync::Arc as core_Arc;

type LevelDBShared<K> = core_Arc<LevelDataBase<K>>;
pub fn open_db<K: db_key::Key>(
    path: &std::path::Path,
    db_options: options::Options,
) -> Result<LevelDataBase<K>, leveldb::database::error::Error> {
    Ok(leveldb::database::Database::<K>::open(path, db_options)?)
}

use db_key;
#[derive(Debug, PartialEq)]
pub struct StringKey(pub String);
impl db_key::Key for StringKey {
    fn from_u8(key: &[u8]) -> Self {
        Self(String::from_utf8(key.to_vec()).unwrap())
    }

    fn as_slice<T, F: Fn(&[u8]) -> T>(&self, f: F) -> T {
        let dst = self.0.as_bytes();
        f(&dst)
    }
}

#[derive(Clone, Copy)]
struct ReadOptions {
    fill_cache: bool,
    verify_checksums: bool,
}

impl<'a, K> From<ReadOptions> for options::ReadOptions<'a, K>
where
    K: db_key::Key,
{
    fn from(item: ReadOptions) -> Self {
        let mut options = options::ReadOptions::new();
        options.fill_cache = item.fill_cache;
        options.verify_checksums = item.verify_checksums;
        options
    }
}

impl<'a, K> From<options::ReadOptions<'a, K>> for ReadOptions
where
    K: db_key::Key,
{
    fn from(item: options::ReadOptions<'a, K>) -> Self {
        ReadOptions {
            fill_cache: item.fill_cache,
            verify_checksums: item.verify_checksums,
        }
    }
}

pub struct SyncCell<T>(Cell<T>);
unsafe impl<T> Sync for SyncCell<T> {}

use serde::{de::DeserializeOwned, Serialize};
use std::marker::PhantomData;
pub struct WrapperLevelDB<K: db_key::Key, V: Serialize + DeserializeOwned> {
    db: LevelDBShared<K>,
    selected_table: String,
    read_options: SyncCell<Option<ReadOptions>>,
    write_options: SyncCell<Option<options::WriteOptions>>,
    separator: char,
    phantom: PhantomData<V>,
}

impl<K, V> WrapperLevelDB<K, V>
where
    K: db_key::Key,
    V: Serialize + DeserializeOwned,
{
    fn deserialize(bytes: Vec<u8>) -> Result<V, error::WrapperLevelDBErrors> {
        let result = bincode::deserialize::<V>(bytes.as_slice());
        if let Ok(value) = result {
            return Ok(value);
        } else {
            return Err(error::WrapperLevelDBErrors::DeserializeError);
        }
    }

    fn serialize(value: V) -> Result<Vec<u8>, error::WrapperLevelDBErrors> {
        if let Ok(bytes) = bincode::serialize(&value) {
            return Ok(bytes);
        } else {
            return Err(error::WrapperLevelDBErrors::SerializeError);
        };
    }
}

#[derive(PartialEq)]
pub enum CursorIndex {
    FromBeginning,
    FromEnding,
    FromKey(String),
}

use super::error;
use leveldb::iterator::{Iterable, LevelDBIterator};
use leveldb::{database::options, kv::KV};
impl<V> WrapperLevelDB<StringKey, V>
where
    V: Serialize + DeserializeOwned,
{
    pub fn new(db: LevelDBShared<StringKey>, table_name: &str) -> WrapperLevelDB<StringKey, V> {
        WrapperLevelDB {
            db: db.clone(),
            selected_table: String::from(table_name),
            read_options: SyncCell(Cell::new(None)),
            write_options: SyncCell(Cell::new(None)),
            separator: char::MAX,
            phantom: PhantomData::default(),
        }
    }

    pub fn partition(&self, subtable_name: &str) -> Self {
        // Create the concatenation
        let table_name = self.build_key(subtable_name);
        WrapperLevelDB {
            db: self.db.clone(),
            selected_table: table_name.0,
            read_options: SyncCell(self.read_options.0.clone()),
            write_options: SyncCell(self.write_options.0.clone()),
            separator: self.separator,
            phantom: PhantomData::default(),
        }
    }

    fn create_last_key(&self) -> String {
        let mut last_key = self.selected_table.clone();
        last_key.push(self.separator);
        last_key.push(self.separator);
        last_key
    }

    fn get_read_options(&self) -> options::ReadOptions<StringKey> {
        if let Some(options) = self.read_options.0.get() {
            return options::ReadOptions::from(options);
        } else {
            return options::ReadOptions::new();
        }
    }

    pub fn set_read_options(&mut self, options: options::ReadOptions<StringKey>) {
        self.read_options
            .0
            .replace(Some(ReadOptions::from(options)));
    }

    fn get_write_options(&self) -> options::WriteOptions {
        if let Some(options) = self.write_options.0.get() {
            return options;
        } else {
            let mut write_options = options::WriteOptions::new();
            write_options.sync = true;
            return write_options;
        }
    }

    pub fn set_write_options(&self, options: options::WriteOptions) {
        self.write_options.0.replace(Some(options));
    }

    fn build_key(&self, key: &str) -> StringKey {
        let table_name = self.selected_table.clone();
        let mut key_builder = String::with_capacity(table_name.len() + key.len() + 1);
        key_builder.push_str(&table_name);
        let last_char: String = self.separator.into();
        key_builder.push_str(&last_char);
        key_builder.push_str(&key);

        StringKey(key_builder)
    }

    pub fn get_table_name(&self) -> String {
        let table_name = self.selected_table.clone();
        let mut key_builder = String::with_capacity(table_name.len() + 1);
        key_builder.push_str(&table_name);
        let last_char: String = self.separator.into();
        key_builder.push_str(&last_char);
        key_builder
    }

    pub fn put(&self, key: &str, value: V) -> Result<(), error::WrapperLevelDBErrors> {
        let key = self.build_key(key);
        let value = WrapperLevelDB::<StringKey, V>::serialize(value)?;

        Ok({
            self.db
                .put(self.get_write_options(), key, value.as_slice())?
        })
    }

    pub fn get_bytes(
        &self,
        key: &str,
    ) -> Result<leveldb::database::bytes::Bytes, error::WrapperLevelDBErrors> {
        let key = self.build_key(key);
        let result = { self.db.get_bytes(self.get_read_options(), key)? };
        if let Some(bytes) = result {
            return Ok(bytes);
        } else {
            return Err(error::WrapperLevelDBErrors::EntryNotFoundError);
        }
    }

    pub fn get(&self, key: &str) -> Result<V, error::WrapperLevelDBErrors> {
        let key = self.build_key(key);
        let result = { self.db.get(self.get_read_options(), key)? };
        if let Some(bytes) = result {
            return Ok(WrapperLevelDB::<StringKey, V>::deserialize(bytes)?);
        } else {
            return Err(error::WrapperLevelDBErrors::EntryNotFoundError);
        }
    }

    pub fn update(&self, key: &str, value: V) -> Result<V, error::WrapperLevelDBErrors> {
        // Check that something exists
        let old_value = self.get(key)?;
        // If it exists, we modify it
        let key = self.build_key(key);
        let value = if let Ok(bytes) = bincode::serialize(&value) {
            bytes
        } else {
            return Err(error::WrapperLevelDBErrors::SerializeError);
        };
        // Update
        self.db
            .put(self.get_write_options(), key, value.as_slice())?;
        Ok(old_value)
    }

    pub fn del(&self, key: &str) -> Result<Option<V>, error::WrapperLevelDBErrors> {
        let old_value = if let Ok(value) = self.get(key) {
            Some(value)
        } else {
            None
        };
        let key = self.build_key(key);
        let write_opts = self.get_write_options();
        self.db.delete(write_opts, key)?;
        Ok(old_value)
    }

    pub fn get_all(&self) -> Vec<(StringKey, V)> {
        let iter = self.db.iter(self.get_read_options());
        let table_name = self.get_table_name();

        iter.seek(&StringKey(self.selected_table.clone()));
        iter.map_while(|(key, bytes)| {
            // Stop when it returns None
            if key.0.starts_with(&table_name) {
                let key = {
                    let StringKey(value) = key;
                    // Remove the table name from the key
                    StringKey(value.replace(&table_name, ""))
                };
                // Perform deserialization to obtain the stored structure from bytes
                let value = WrapperLevelDB::<StringKey, V>::deserialize(bytes).unwrap();
                Some((key, value))
            } else {
                None
            }
        })
        .collect()
    }

    pub fn get_range(&self, cursor: &CursorIndex, quantity: isize) -> Vec<(StringKey, V)> {
        let iter = self.db.iter(self.get_read_options());
        let table_name = self.get_table_name();
        let mut count = 0usize;
        let closure = |value: (StringKey, Vec<u8>)| {
            // Stop when it returns None
            let (key, bytes) = value;
            let quantity = quantity.abs() as usize;
            if key.0.starts_with(&table_name) && count < quantity {
                let key = {
                    let StringKey(value) = key;
                    // Remove the table name from the key
                    StringKey(value.replace(&table_name, ""))
                };
                // Perform deserialization to obtain the stored structure from bytes
                let value = WrapperLevelDB::<StringKey, V>::deserialize(bytes).unwrap();
                count += 1;
                return Some((key, value));
            } else {
                None
            }
        };

        let mut key = match cursor {
            CursorIndex::FromBeginning => StringKey(table_name.clone()),
            CursorIndex::FromEnding => StringKey(self.create_last_key()),
            CursorIndex::FromKey(key) => self.build_key(&key),
        };
        if quantity < 0 {
            let mut iter = iter.reverse();
            iter.seek(&key);
            if cursor == &CursorIndex::FromEnding {
                iter.advance();
            }
            iter.map_while(closure).collect()
        } else {
            if cursor == &CursorIndex::FromEnding {
                let temp_iter = self.db.iter(self.get_read_options()).reverse();
                temp_iter.seek(&key);
                key = temp_iter.skip(1).next().unwrap().0; // Modify the marker for the real one.
            }
            iter.seek(&key);
            iter.map_while(closure).collect()
        }
    }

    pub fn get_count(&self) -> usize {
        let mut iter = self.db.keys_iter(self.get_read_options());
        let first_key = StringKey(self.get_table_name());
        let mut count = 0;
        iter.seek(&first_key);
        // Take the index of the first key of our 'table'....
        iter.any(|key| {
            if key.0.starts_with(&first_key.0) {
                count += 1;
                false
            } else {
                true
            }
        });
        count
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::bd::level_db::wrapper_leveldb::{open_db, CursorIndex};
    use leveldb::options::Options;
    use serde::{Deserialize, Serialize};
    use tempdir::TempDir;

    use super::{StringKey, WrapperLevelDB};

    const TABLE_NAME1: &str = "TESTS";
    const TABLE_NAME2: &str = "PRUEBA";

    #[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
    struct Test {
        id: usize,
        value: String,
    }
    #[test]
    fn test_insert_update_remove() {
        let rt = tokio::runtime::Runtime::new().unwrap();

        rt.block_on(async {
            let temp_dir = TempDir::new("test_insert_update_remove").unwrap();
            let path = temp_dir.path();
            println!("DB_PATH = {:#?}", path.as_os_str());

            let mut db_options = Options::new();
            db_options.create_if_missing = true;
            let db = if let Ok(db) = open_db(path, db_options) {
                db
            } else {
                panic!("Error trying to open database");
            };
            let wrapper1: WrapperLevelDB<StringKey, Test> =
                WrapperLevelDB::<StringKey, Test>::new(Arc::new(db), TABLE_NAME1);

            // Insert
            let mut test_value = Test {
                id: 0,
                value: String::from("hola"),
            };
            if let Err(_) = wrapper1.put("key", test_value.clone()) {
                assert!(false);
            }

            if let Ok(value) = wrapper1.get("key") {
                assert_eq!(test_value, value);
            } else {
                assert!(false);
            }

            // Update
            test_value.id = 1;
            if let Err(_) = wrapper1.update("key", test_value.clone()) {
                assert!(false);
            }

            if let Ok(value) = wrapper1.get("key") {
                assert_eq!(test_value, value);
            } else {
                assert!(false);
            }

            // Delete
            if let Err(_) = wrapper1.del("key") {
                assert!(false);
            }
            if let Ok(_) = wrapper1.get("key") {
                assert!(false);
            } else {
                assert!(true);
            }
        });
    }

    #[test]
    fn test_two_tables() {
        let rt = tokio::runtime::Runtime::new().unwrap();

        rt.block_on(async {
            let temp_dir = TempDir::new("test_two_tables").unwrap();
            let path = temp_dir.path();
            println!("DB_PATH = {:#?}", path.as_os_str());

            let mut db_options = Options::new();
            db_options.create_if_missing = true;
            let db = if let Ok(db) = open_db(path, db_options) {
                Arc::new(db)
            } else {
                panic!("Error trying to open database");
            };
            let wrapper1 = WrapperLevelDB::<StringKey, String>::new(db.clone(), TABLE_NAME1);
            let wrapper2 = WrapperLevelDB::<StringKey, u32>::new(db, TABLE_NAME2);

            if let Err(_) = wrapper1.put("Clave", String::from("Valor en tabla test")) {
                assert!(false);
            }

            if let Err(_) = wrapper2.put("Clave", 5) {
                assert!(false);
            }

            let value1 = if let Ok(value) = wrapper1.get("Clave") {
                value
            } else {
                panic!("Error taking back value")
            };

            assert_eq!(value1, String::from("Valor en tabla test"));

            let value2 = if let Ok(value) = wrapper2.get("Clave") {
                value
            } else {
                panic!("Error taking back value")
            };
            assert_eq!(value2, 5);
        });
    }

    use leveldb::options::Options as LevelDBOptions;
    const EJEMPLO_TABLE: &str = "EJEMPLO0";
    const PRUEBA_TABLE: &str = "PRUEBA1";
    const TEST_TABLE: &str = "TEST2";

    fn set_up_entries(
        wrapper0: WrapperLevelDB<StringKey, u64>,
        wrapper1: WrapperLevelDB<StringKey, u64>,
        wrapper2: WrapperLevelDB<StringKey, u64>,
    ) {
        wrapper0.put("b", 01).unwrap();
        wrapper0.put("a", 02).unwrap();
        wrapper0.put("0", 03).unwrap();

        wrapper1.put("b", 10).unwrap();
        wrapper1.put("a", 11).unwrap();
        wrapper1.put("0", 12).unwrap();
        wrapper1.put("00", 13).unwrap();
        wrapper1.put("0a", 14).unwrap();

        wrapper2.put("b", 20).unwrap();
        wrapper2.put("0", 21).unwrap();
        wrapper2.put("a", 22).unwrap();
    }

    #[test]
    fn test_get_all() {
        let rt = tokio::runtime::Runtime::new().unwrap();

        rt.block_on(async {
            let temp_dir = TempDir::new("test_get_all").unwrap();
            {
                let mut db_options = LevelDBOptions::new();
                db_options.create_if_missing = true;
                let db = Arc::new(
                    crate::bd::level_db::wrapper_leveldb::open_db::<StringKey>(
                        temp_dir.path(),
                        db_options,
                    )
                    .unwrap(),
                );

                let wrapper0 = WrapperLevelDB::<StringKey, u64>::new(db.clone(), EJEMPLO_TABLE);
                let wrapper1 = WrapperLevelDB::<StringKey, u64>::new(db.clone(), PRUEBA_TABLE);
                let wrapper2 = WrapperLevelDB::<StringKey, u64>::new(db.clone(), TEST_TABLE);

                set_up_entries(wrapper0, wrapper1, wrapper2);
            }

            {
                // Reopen the connection to confirm persistence...
                let mut db_options = LevelDBOptions::new();
                db_options.create_if_missing = false;
                let db = Arc::new(
                    crate::bd::level_db::wrapper_leveldb::open_db::<StringKey>(
                        temp_dir.path(),
                        db_options,
                    )
                    .unwrap(),
                );

                let wrapper0 = WrapperLevelDB::<StringKey, u64>::new(db.clone(), EJEMPLO_TABLE);
                let wrapper1 = WrapperLevelDB::<StringKey, u64>::new(db.clone(), PRUEBA_TABLE);
                let wrapper2 = WrapperLevelDB::<StringKey, u64>::new(db.clone(), TEST_TABLE);

                assert_eq!(
                    vec![
                        (StringKey("0".to_string()), 3),
                        (StringKey("a".to_string()), 2),
                        (StringKey("b".to_string()), 1)
                    ],
                    wrapper0.get_all()
                );
                assert_eq!(3, wrapper0.get_count());

                assert_eq!(
                    vec![
                        (StringKey("0".to_string()), 12),
                        (StringKey("00".to_string()), 13),
                        (StringKey("0a".to_string()), 14),
                        (StringKey("a".to_string()), 11),
                        (StringKey("b".to_string()), 10)
                    ],
                    wrapper1.get_all()
                );
                assert_eq!(5, wrapper1.get_count());

                assert_eq!(
                    vec![
                        (StringKey("0".to_string()), 21),
                        (StringKey("a".to_string()), 22),
                        (StringKey("b".to_string()), 20)
                    ],
                    wrapper2.get_all()
                );
                assert_eq!(3, wrapper2.get_count());
            }
        });
    }

    #[test]
    fn test_get_range_positive() {
        let rt = tokio::runtime::Runtime::new().unwrap();

        rt.block_on(async {
            let temp_dir = TempDir::new("test_get_range_positive").unwrap();
            {
                let mut db_options = LevelDBOptions::new();
                db_options.create_if_missing = true;
                let db = Arc::new(
                    crate::bd::level_db::wrapper_leveldb::open_db::<StringKey>(
                        temp_dir.path(),
                        db_options,
                    )
                    .unwrap(),
                );

                let wrapper0 = WrapperLevelDB::<StringKey, u64>::new(db.clone(), EJEMPLO_TABLE);
                let wrapper1 = WrapperLevelDB::<StringKey, u64>::new(db.clone(), PRUEBA_TABLE);
                let wrapper2 = WrapperLevelDB::<StringKey, u64>::new(db.clone(), TEST_TABLE);

                set_up_entries(wrapper0, wrapper1, wrapper2);
            }

            {
                // Reopen the connection to confirm persistence...
                let mut db_options = LevelDBOptions::new();
                db_options.create_if_missing = false;
                let db = Arc::new(
                    crate::bd::level_db::wrapper_leveldb::open_db::<StringKey>(
                        temp_dir.path(),
                        db_options,
                    )
                    .unwrap(),
                );

                let wrapper1 = WrapperLevelDB::<StringKey, u64>::new(db.clone(), PRUEBA_TABLE);

                assert_eq!(
                    vec![
                        (StringKey("0".to_string()), 12),
                        (StringKey("00".to_string()), 13),
                        (StringKey("0a".to_string()), 14),
                        (StringKey("a".to_string()), 11),
                        (StringKey("b".to_string()), 10)
                    ],
                    wrapper1.get_all()
                );
                assert_eq!(
                    vec![
                        (StringKey("0a".to_string()), 14),
                        (StringKey("a".to_string()), 11),
                        (StringKey("b".to_string()), 10)
                    ],
                    wrapper1.get_range(&CursorIndex::FromKey("0a".into()), 6)
                );
                assert_eq!(
                    vec![] as Vec<(StringKey, u64)>,
                    wrapper1.get_range(&CursorIndex::FromKey("a".into()), 0)
                );
                assert_eq!(
                    vec![(StringKey("a".to_string()), 11),],
                    wrapper1.get_range(&CursorIndex::FromKey("a".into()), 1)
                );
            }
        });
    }

    #[test]
    fn test_get_range_negative() {
        let rt = tokio::runtime::Runtime::new().unwrap();

        rt.block_on(async {
            let temp_dir = TempDir::new("test_get_range_negative").unwrap();
            {
                let mut db_options = LevelDBOptions::new();
                db_options.create_if_missing = true;
                let db = Arc::new(
                    crate::bd::level_db::wrapper_leveldb::open_db::<StringKey>(
                        temp_dir.path(),
                        db_options,
                    )
                    .unwrap(),
                );

                let wrapper0 = WrapperLevelDB::<StringKey, u64>::new(db.clone(), EJEMPLO_TABLE);
                let wrapper1 = WrapperLevelDB::<StringKey, u64>::new(db.clone(), PRUEBA_TABLE);
                let wrapper2 = WrapperLevelDB::<StringKey, u64>::new(db.clone(), TEST_TABLE);

                set_up_entries(wrapper0, wrapper1, wrapper2);
            }

            {
                // Reopen the connection to confirm persistence...
                let mut db_options = LevelDBOptions::new();
                db_options.create_if_missing = false;
                let db = Arc::new(
                    crate::bd::level_db::wrapper_leveldb::open_db::<StringKey>(
                        temp_dir.path(),
                        db_options,
                    )
                    .unwrap(),
                );

                let wrapper1 = WrapperLevelDB::<StringKey, u64>::new(db.clone(), PRUEBA_TABLE);

                assert_eq!(
                    vec![
                        (StringKey("0".to_string()), 12),
                        (StringKey("00".to_string()), 13),
                        (StringKey("0a".to_string()), 14),
                        (StringKey("a".to_string()), 11),
                        (StringKey("b".to_string()), 10)
                    ],
                    wrapper1.get_all()
                );
                assert_eq!(
                    vec![
                        (StringKey("a".to_string()), 11),
                        (StringKey("0a".to_string()), 14),
                        (StringKey("00".to_string()), 13),
                        (StringKey("0".to_string()), 12)
                    ],
                    wrapper1.get_range(&CursorIndex::FromKey("a".into()), -6)
                );
                assert_eq!(
                    vec![] as Vec<(StringKey, u64)>,
                    wrapper1.get_range(&CursorIndex::FromKey("a".into()), 0)
                );
                assert_eq!(
                    vec![(StringKey("a".to_string()), 11)],
                    wrapper1.get_range(&CursorIndex::FromKey("a".into()), -1)
                );
            }
        });
    }

    #[test]
    fn test_get_range_from_first() {
        let rt = tokio::runtime::Runtime::new().unwrap();

        rt.block_on(async {
            let temp_dir = TempDir::new("test_get_range_from_first").unwrap();
            {
                let mut db_options = LevelDBOptions::new();
                db_options.create_if_missing = true;
                let db = Arc::new(
                    crate::bd::level_db::wrapper_leveldb::open_db::<StringKey>(
                        temp_dir.path(),
                        db_options,
                    )
                    .unwrap(),
                );

                let wrapper0 = WrapperLevelDB::<StringKey, u64>::new(db.clone(), EJEMPLO_TABLE);
                let wrapper1 = WrapperLevelDB::<StringKey, u64>::new(db.clone(), PRUEBA_TABLE);
                let wrapper2 = WrapperLevelDB::<StringKey, u64>::new(db.clone(), TEST_TABLE);

                set_up_entries(wrapper0, wrapper1, wrapper2);
            }

            {
                // Reopen the connection to confirm persistence...
                let mut db_options = LevelDBOptions::new();
                db_options.create_if_missing = false;
                let db = Arc::new(
                    crate::bd::level_db::wrapper_leveldb::open_db::<StringKey>(
                        temp_dir.path(),
                        db_options,
                    )
                    .unwrap(),
                );

                let wrapper1 = WrapperLevelDB::<StringKey, u64>::new(db.clone(), PRUEBA_TABLE);

                assert_eq!(
                    vec![
                        (StringKey("0".to_string()), 12),
                        (StringKey("00".to_string()), 13),
                        (StringKey("0a".to_string()), 14),
                        (StringKey("a".to_string()), 11),
                        (StringKey("b".to_string()), 10)
                    ],
                    wrapper1.get_all()
                );
                assert_eq!(
                    vec![
                        (StringKey("0".to_string()), 12),
                        (StringKey("00".to_string()), 13),
                        (StringKey("0a".to_string()), 14),
                        (StringKey("a".to_string()), 11),
                    ],
                    wrapper1.get_range(&CursorIndex::FromBeginning, 4)
                );
                assert_eq!(
                    vec![] as Vec<(StringKey, u64)>,
                    wrapper1.get_range(&CursorIndex::FromBeginning, 0)
                );
                assert_eq!(
                    vec![(StringKey("0".to_string()), 12)],
                    wrapper1.get_range(&CursorIndex::FromBeginning, -1)
                );
            }
        });
    }

    #[test]
    fn test_get_range_from_last() {
        let rt = tokio::runtime::Runtime::new().unwrap();

        rt.block_on(async {
            let temp_dir = TempDir::new("test_get_range_from_first").unwrap();
            {
                let mut db_options = LevelDBOptions::new();
                db_options.create_if_missing = true;
                let db = Arc::new(
                    crate::bd::level_db::wrapper_leveldb::open_db::<StringKey>(
                        temp_dir.path(),
                        db_options,
                    )
                    .unwrap(),
                );

                let wrapper0 = WrapperLevelDB::<StringKey, u64>::new(db.clone(), EJEMPLO_TABLE);
                let wrapper1 = WrapperLevelDB::<StringKey, u64>::new(db.clone(), PRUEBA_TABLE);
                let wrapper2 = WrapperLevelDB::<StringKey, u64>::new(db.clone(), TEST_TABLE);

                set_up_entries(wrapper0, wrapper1, wrapper2);
            }

            {
                // Reopen the connection to confirm persistence...
                let mut db_options = LevelDBOptions::new();
                db_options.create_if_missing = false;
                let db = Arc::new(
                    crate::bd::level_db::wrapper_leveldb::open_db::<StringKey>(
                        temp_dir.path(),
                        db_options,
                    )
                    .unwrap(),
                );

                let wrapper1 = WrapperLevelDB::<StringKey, u64>::new(db.clone(), PRUEBA_TABLE);

                assert_eq!(
                    vec![
                        (StringKey("0".to_string()), 12),
                        (StringKey("00".to_string()), 13),
                        (StringKey("0a".to_string()), 14),
                        (StringKey("a".to_string()), 11),
                        (StringKey("b".to_string()), 10)
                    ],
                    wrapper1.get_all()
                );
                assert_eq!(
                    vec![
                        (StringKey("b".to_string()), 10),
                        (StringKey("a".to_string()), 11),
                        (StringKey("0a".to_string()), 14),
                        (StringKey("00".to_string()), 13),
                        (StringKey("0".to_string()), 12)
                    ],
                    wrapper1.get_range(&CursorIndex::FromEnding, -5)
                );
                assert_eq!(
                    vec![(StringKey("b".to_string()), 10),],
                    wrapper1.get_range(&CursorIndex::FromEnding, 1)
                );
                assert_eq!(
                    vec![] as Vec<(StringKey, u64)>,
                    wrapper1.get_range(&CursorIndex::FromEnding, 0)
                );
                assert_eq!(
                    vec![
                        (StringKey("b".to_string()), 10),
                        (StringKey("a".to_string()), 11),
                    ],
                    wrapper1.get_range(&CursorIndex::FromEnding, -2)
                );
            }
        });
    }

    // TODO: Unit test for new_subtable
    #[test]
    fn test_simple_new_subtable() {
        let rt = tokio::runtime::Runtime::new().unwrap();

        rt.block_on(async {
            let temp_dir = TempDir::new("test_simple_new_subtable").unwrap();
            {
                let mut db_options = LevelDBOptions::new();
                db_options.create_if_missing = true;
                let db = Arc::new(
                    crate::bd::level_db::wrapper_leveldb::open_db::<StringKey>(
                        temp_dir.path(),
                        db_options,
                    )
                    .unwrap(),
                );

                let wrapper0 = WrapperLevelDB::<StringKey, u64>::new(db.clone(), EJEMPLO_TABLE);
                let wrapper00 = wrapper0.partition("SUB1");
                let wrapper001 = wrapper00.partition("ASUB1");
                let wrapper01 = wrapper0.partition("SUB2");
                let wrapper1 = WrapperLevelDB::<StringKey, u64>::new(db.clone(), PRUEBA_TABLE);

                set_up_entries(wrapper00, wrapper001, wrapper01);

                wrapper1.put("b", 30).unwrap();
                wrapper1.put("0", 31).unwrap();
                wrapper1.put("a", 32).unwrap();
            }

            {
                // Reopen the connection to confirm persistence...
                let mut db_options = LevelDBOptions::new();
                db_options.create_if_missing = false;
                let db = Arc::new(
                    crate::bd::level_db::wrapper_leveldb::open_db::<StringKey>(
                        temp_dir.path(),
                        db_options,
                    )
                    .unwrap(),
                );

                let wrapper0 = WrapperLevelDB::<StringKey, u64>::new(db.clone(), EJEMPLO_TABLE);
                let wrapper_mal = wrapper0.partition("SUB");
                let wrapper00 = wrapper0.partition("SUB1");
                let wrapper001 = wrapper00.partition("ASUB1");
                let wrapper01 = wrapper0.partition("SUB2");
                let wrapper1 = WrapperLevelDB::<StringKey, u64>::new(db.clone(), PRUEBA_TABLE);

                assert_eq!(
                    // Has all partition values inserted "SUB1", "ASUB1" y "SUB2"
                    vec![
                        (StringKey("SUB1\u{10ffff}0".to_string()), 3),
                        (StringKey("SUB1\u{10ffff}ASUB1\u{10ffff}0".to_string()), 12),
                        (StringKey("SUB1\u{10ffff}ASUB1\u{10ffff}00".to_string()), 13),
                        (StringKey("SUB1\u{10ffff}ASUB1\u{10ffff}0a".to_string()), 14),
                        (StringKey("SUB1\u{10ffff}ASUB1\u{10ffff}a".to_string()), 11),
                        (StringKey("SUB1\u{10ffff}ASUB1\u{10ffff}b".to_string()), 10),
                        (StringKey("SUB1\u{10ffff}a".to_string()), 2),
                        (StringKey("SUB1\u{10ffff}b".to_string()), 1),
                        (StringKey("SUB2\u{10ffff}0".to_string()), 21),
                        (StringKey("SUB2\u{10ffff}a".to_string()), 22),
                        (StringKey("SUB2\u{10ffff}b".to_string()), 20)
                    ],
                    wrapper0.get_all()
                );

                assert_eq!(11, wrapper0.get_count());

                assert_eq!(
                    // The "SUB" partition has nothing inserted
                    vec![] as Vec<(StringKey, u64)>,
                    wrapper_mal.get_all()
                );

                assert_eq!(
                    // Has all the values inserted in itself and in the partition "ASUB1"
                    vec![
                        (StringKey("0".to_string()), 3),
                        (StringKey("ASUB1\u{10ffff}0".to_string()), 12),
                        (StringKey("ASUB1\u{10ffff}00".to_string()), 13),
                        (StringKey("ASUB1\u{10ffff}0a".to_string()), 14),
                        (StringKey("ASUB1\u{10ffff}a".to_string()), 11),
                        (StringKey("ASUB1\u{10ffff}b".to_string()), 10),
                        (StringKey("a".to_string()), 2),
                        (StringKey("b".to_string()), 1)
                    ],
                    wrapper00.get_all()
                );
                assert_eq!(8, wrapper00.get_count());

                assert_eq!(
                    // Has only the values inserted in itself ignoring the rest of the values inserted in its parent
                    vec![
                        (StringKey("0".to_string()), 12),
                        (StringKey("00".to_string()), 13),
                        (StringKey("0a".to_string()), 14),
                        (StringKey("a".to_string()), 11),
                        (StringKey("b".to_string()), 10)
                    ],
                    wrapper001.get_all()
                );

                assert_eq!(
                    // Has only the values inserted in itself ignoring the rest of the values inserted in its siblings
                    vec![
                        (StringKey("0".to_string()), 21),
                        (StringKey("a".to_string()), 22),
                        (StringKey("b".to_string()), 20)
                    ],
                    wrapper01.get_all()
                );

                assert_eq!(
                    vec![
                        (StringKey("0".to_string()), 31),
                        (StringKey("a".to_string()), 32),
                        (StringKey("b".to_string()), 30)
                    ],
                    wrapper1.get_all()
                )
            }
        });
    }

    #[test]
    fn test_complex_new_subtable() {
        let rt = tokio::runtime::Runtime::new().unwrap();

        rt.block_on(async {
            let temp_dir = TempDir::new("test_simple_new_subtable").unwrap();
            {
                let mut db_options = LevelDBOptions::new();
                db_options.create_if_missing = true;
                let db = Arc::new(
                    crate::bd::level_db::wrapper_leveldb::open_db::<StringKey>(
                        temp_dir.path(),
                        db_options,
                    )
                    .unwrap(),
                );

                let wrapper0 = WrapperLevelDB::<StringKey, u64>::new(db.clone(), EJEMPLO_TABLE);
                let wrapper00 = wrapper0.partition("SUB1");
                let wrapper001 = wrapper00.partition("ASUB1");
                let wrapper01 = wrapper0.partition("SUB2");
                let wrapper1 = WrapperLevelDB::<StringKey, u64>::new(db.clone(), PRUEBA_TABLE);

                set_up_entries(wrapper00, wrapper001, wrapper01);

                wrapper1.put("b", 30).unwrap();
                wrapper1.put("0", 31).unwrap();
                wrapper1.put("a", 32).unwrap();
            }

            {
                // Reopen the connection to confirm persistence...
                let mut db_options = LevelDBOptions::new();
                db_options.create_if_missing = false;
                let db = Arc::new(
                    crate::bd::level_db::wrapper_leveldb::open_db::<StringKey>(
                        temp_dir.path(),
                        db_options,
                    )
                    .unwrap(),
                );

                let wrapper0 = WrapperLevelDB::<StringKey, u64>::new(db.clone(), EJEMPLO_TABLE);
                let wrapper_mal = wrapper0.partition("SUB");
                let wrapper00 = wrapper0.partition("SUB1");
                let wrapper001 = wrapper00.partition("ASUB1");
                let wrapper01 = wrapper0.partition("SUB2");
                let wrapper1 = WrapperLevelDB::<StringKey, u64>::new(db.clone(), PRUEBA_TABLE);

                assert_eq!(
                    // Has all partition values inserted "SUB1", "ASUB1" y "SUB2"
                    vec![
                        (StringKey("SUB1\u{10ffff}ASUB1\u{10ffff}0".to_string()), 12),
                        (StringKey("SUB1\u{10ffff}ASUB1\u{10ffff}00".to_string()), 13),
                    ],
                    wrapper0.get_range(
                        &CursorIndex::FromKey("SUB1\u{10ffff}ASUB1\u{10ffff}0".into()),
                        2
                    )
                );

                assert_eq!(
                    // The "SUB" partition has not inserted anything
                    vec![] as Vec<(StringKey, u64)>,
                    wrapper_mal.get_range(&CursorIndex::FromBeginning, 3)
                );

                assert_eq!(
                    // Has all the values inserted in itself and in the partition "ASUB1"
                    vec![
                        (StringKey("b".to_string()), 1),
                        (StringKey("a".to_string()), 2),
                        (StringKey("ASUB1\u{10ffff}b".to_string()), 10),
                        (StringKey("ASUB1\u{10ffff}a".to_string()), 11),
                        (StringKey("ASUB1\u{10ffff}0a".to_string()), 14),
                        (StringKey("ASUB1\u{10ffff}00".to_string()), 13),
                        (StringKey("ASUB1\u{10ffff}0".to_string()), 12),
                        (StringKey("0".to_string()), 3)
                    ],
                    wrapper00.get_range(&CursorIndex::FromEnding, -300)
                );

                assert_eq!(
                    // Has only the values inserted in itself ignoring the rest of the values inserted in its parent
                    vec![
                        (StringKey("a".to_string()), 11),
                        (StringKey("0a".to_string()), 14),
                        (StringKey("00".to_string()), 13),
                        (StringKey("0".to_string()), 12)
                    ],
                    wrapper001.get_range(&CursorIndex::FromKey("a".into()), -200)
                );

                assert_eq!(
                    // Has only the values inserted in itself ignoring the rest of the values inserted in its siblings
                    vec![
                        (StringKey("0".to_string()), 21),
                        (StringKey("a".to_string()), 22),
                        (StringKey("b".to_string()), 20)
                    ],
                    wrapper01.get_range(&CursorIndex::FromBeginning, 3)
                );

                assert_eq!(
                    vec![
                        (StringKey("0".to_string()), 31),
                        (StringKey("a".to_string()), 32),
                        (StringKey("b".to_string()), 30)
                    ],
                    wrapper1.get_all()
                )
            }
        });
    }
}
