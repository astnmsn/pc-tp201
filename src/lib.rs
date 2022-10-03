use serde::{Deserialize, Serialize};

pub trait KvsEngine<K, V, Clone = Self> {
    fn set(&self, key: K, value: V) -> Result<()>;
    fn get(&self, key: K) -> Result<Option<V>>;
    fn remove(&self, key: K) -> Result<()>;
}
pub type Result<T> = std::result::Result<T, KvsError>;

#[derive(Debug, Serialize, Deserialize)]
pub enum KvsError {
    FileListEmpty,
    WrongEngine,
    SerializationError(String),
    IOError(String),
    NonExistantKey,
    Other,
}

impl From<serde_json::Error> for KvsError {
    fn from(serde_err: serde_json::Error) -> Self {
        KvsError::SerializationError(serde_err.to_string())
    }
}

impl From<std::io::Error> for KvsError {
    fn from(io_err: std::io::Error) -> Self {
        KvsError::IOError(io_err.to_string())
    }
}

pub mod thread_pool {
    use crate::Result;
    pub trait ThreadPool {
        fn new(threads: u32) -> Result<Self>
        where
            Self: Sized;
        fn spawn<F>(&self, job: F)
        where
            F: FnOnce() + Send + 'static;
    }

    pub struct NaiveThreadPool {}
    impl ThreadPool for NaiveThreadPool {
        fn new(threads: u32) -> Result<Self>
        where
            Self: Sized,
        {
            todo!()
        }

        fn spawn<F>(&self, job: F)
        where
            F: FnOnce() + Send + 'static,
        {
            todo!()
        }
    }

    pub struct SharedQueueThreadPool {}
    impl ThreadPool for SharedQueueThreadPool {
        fn new(threads: u32) -> Result<Self>
        where
            Self: Sized,
        {
            todo!()
        }

        fn spawn<F>(&self, job: F)
        where
            F: FnOnce() + Send + 'static,
        {
            todo!()
        }
    }

    pub struct RayonThreadPool {}
    impl ThreadPool for RayonThreadPool {
        fn new(threads: u32) -> Result<Self>
        where
            Self: Sized,
        {
            todo!()
        }

        fn spawn<F>(&self, job: F)
        where
            F: FnOnce() + Send + 'static,
        {
            todo!()
        }
    }
}

pub mod protocol {
    use crate::Result;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, Debug)]
    pub enum KvRequest<K, V> {
        Set((K, V)),
        Rm(K),
        Get(K),
    }

    #[derive(Serialize, Deserialize, Debug)]
    pub struct KvResponse<V> {
        pub value: Result<Option<V>>,
    }
}

pub mod store {
    use std::fmt::Debug;
    use std::fmt::Display;
    use std::fs;
    use std::fs::OpenOptions;
    use std::io::Cursor;
    use std::io::Read;
    use std::io::{Seek, SeekFrom, Write};
    use std::marker::PhantomData;
    use std::path::Path;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::PoisonError;
    use std::sync::RwLock;
    use std::time::SystemTime;
    use std::time::UNIX_EPOCH;
    use std::{collections::HashMap, hash::Hash};

    use serde::{Deserialize, Serialize};
    use stderrlog::new;

    use crate::{KvsEngine, KvsError, Result};

    pub trait Key:
        Debug + Display + Clone + Eq + Hash + Serialize + for<'de> Deserialize<'de>
    {
    }
    pub trait Value: Debug + Display + Clone + Serialize + for<'de> Deserialize<'de> {}

    impl Key for String {}
    impl Value for String {}

    #[derive(Serialize, Deserialize, Debug)]
    enum KvRecord<K, V> {
        Set((K, V)),
        Rm(K),
    }

    struct ValueData {
        size: usize,
        offset: u64,
        file_path: PathBuf,
    }

    struct InnerStructs<K> {
        dir_path: PathBuf,
        bytes_in_last_file: u64,
        active_file: PathBuf,
        inactive_files: Vec<PathBuf>,
        key_map: HashMap<K, ValueData>,
    }

    pub struct KvStore<K, V>
    where
        K: Key,
        V: Value,
    {
        inner: Arc<RwLock<InnerStructs<K>>>,
        phantom: PhantomData<V>,
    }

    impl<K, V> KvsEngine<K, V> for KvStore<K, V>
    where
        K: Key,
        V: Value,
    {
        fn set(&self, key: K, val: V) -> Result<()> {
            self.write_command(&KvRecord::Set((key.clone(), val.clone())), key)?;
            Ok(())
        }
        fn get(&self, key: K) -> Result<Option<V>> {
            if let Some(value_data) = self.inner.read()?.key_map.get(&key) {
                let mut file = OpenOptions::new()
                    .read(true)
                    .open(value_data.file_path.clone())?;
                file.seek(SeekFrom::Start(value_data.offset as u64))?;
                let mut buf = vec![0u8; value_data.size];
                file.read_exact(&mut buf)?;
                match rmp_serde::from_slice(&buf)? {
                    KvRecord::Set(kv) => {
                        let _key: K = kv.0;
                        Ok(Some(kv.1))
                    }
                    _ => Ok(None),
                }
            } else {
                Ok(None)
            }
        }
        fn remove(&self, key: K) -> Result<()> {
            if let Ok(Some(_val)) = self.get(key.clone()) {
                self.write_command(&KvRecord::Rm::<K, V>(key.clone()), key)?;
                Ok(())
            } else {
                Err(KvsError::NonExistantKey)
            }
        }
    }

    impl From<rmp_serde::decode::Error> for KvsError {
        fn from(serde_err: rmp_serde::decode::Error) -> Self {
            KvsError::SerializationError(serde_err.to_string())
        }
    }

    impl From<rmp_serde::encode::Error> for KvsError {
        fn from(serde_err: rmp_serde::encode::Error) -> Self {
            KvsError::SerializationError(serde_err.to_string())
        }
    }

    impl<T> From<PoisonError<T>> for KvsError {
        fn from(poison_error: PoisonError<T>) -> Self {
            KvsError::SerializationError(poison_error.to_string())
        }
    }

    impl<K, V> KvStore<K, V>
    where
        K: Key,
        V: Value,
    {
        fn alloc_new_file(dir_path: &Path) -> Result<PathBuf> {
            let path = dir_path.join(format!(
                "{}.kvs",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("time went backwards")
                    .as_nanos()
                    .to_string()
            ));
            fs::File::create(&path)?;
            Ok(path)
        }

        fn alloc_new_file_if_needed(&mut self) -> Result<()> {
            let mut inner = self.inner.write()?;
            if inner.bytes_in_last_file > 1000000 {
                if inner.inactive_files.len() >= 10 {
                    self.compact_files()?;
                    return Ok(());
                }
                let new_path = KvStore::<K, V>::alloc_new_file(&inner.dir_path)?;
                inner.inactive_files.push(inner.active_file);
                inner.active_file = new_path;
                inner.bytes_in_last_file = 0;
            }
            Ok(())
        }

        fn new(db_path: &Path) -> Result<KvStore<K, V>> {
            if !db_path.exists() {
                fs::create_dir_all(&db_path)?;
            }
            let mut files = vec![];
            let mut bytes_in_last_file: u64 = 0;
            for file in fs::read_dir(&db_path)? {
                let file = file.unwrap();
                if file
                    .path()
                    .extension()
                    .map(|osstr| (*osstr).to_str().map(|str| str == "kvs").unwrap_or(false))
                    .unwrap_or(false)
                {
                    files.push(file.path());
                    bytes_in_last_file = file.metadata()?.len();
                }
            }
            if files.is_empty() {
                files.push(KvStore::<K, V>::alloc_new_file(&db_path)?);
            }
            Ok(KvStore {
                inner: Arc::new(RwLock::new(InnerStructs {
                    dir_path: db_path.to_owned(),
                    key_map: HashMap::new(),
                    active_file: files.pop().unwrap(),
                    inactive_files: files,
                    bytes_in_last_file,
                })),
                phantom: PhantomData,
            })
        }

        fn write_command(&self, command: &KvRecord<K, V>, key: K) -> Result<()> {
            let serialized = rmp_serde::to_vec(command)?;
            let mut inner_structs = self.inner.write()?;
            inner_structs.key_map.insert(
                key,
                ValueData {
                    offset: inner_structs.bytes_in_last_file,
                    size: serialized.len(),
                    file_path: inner_structs.active_file.clone(),
                },
            );
            let mut file = OpenOptions::new()
                .write(true)
                .append(true)
                .open(&inner_structs.active_file)
                .unwrap();
            file.write_all(&serialized)?;
            inner_structs.bytes_in_last_file += serialized.len() as u64;
            Ok(())
        }

        fn deserialize_files(
            files: &[PathBuf],
            mut f: impl FnMut(KvRecord<K, V>, ValueData) -> (),
        ) -> Result<()> {
            for file_path in files {
                let file = fs::read(&file_path)?;
                let mut deserializer = rmp_serde::Deserializer::new(Cursor::new(&file));
                let mut position: u64 = 0;
                while position < file.len() as u64 {
                    let deserialized: KvRecord<K, V> =
                        serde::Deserialize::deserialize(&mut deserializer)?;
                    // println!("Deserialized: {:?}", deserialized);
                    let new_position = rmp_serde::decode::Deserializer::position(&deserializer);
                    let value_data = ValueData {
                        offset: position,
                        size: (new_position - position) as usize,
                        file_path: file_path.clone(),
                    };
                    f(deserialized, value_data);
                    position = new_position;
                }
            }
            Ok(())
        }

        pub fn open(db_path: &Path) -> Result<KvStore<K, V>> {
            let store = KvStore::new(db_path)?;
            let mut key_map = HashMap::new();
            let inner = store.inner.read()?;
            KvStore::deserialize_files(
                &[inner.inactive_files.as_slice(), vec![inner.active_file.clone()].as_slice()].concat(),
                |deserialized: KvRecord<K, V>, value_data| match deserialized {
                    KvRecord::Set(kv) => {
                        key_map.insert(kv.0, value_data);
                    }
                    KvRecord::Rm(key) => {
                        key_map.insert(key, value_data);
                    }
                },
            )?;
            let mut inner = store.inner.write()?;
            inner.key_map = key_map;
            Ok(store)
        }

        fn compact_files(& self) -> Result<()> {
            let inner = self.inner.read()?;
            let mut set_map = HashMap::new();
            KvStore::deserialize_files(
                &[inner.inactive_files.as_slice(), vec![inner.active_file.clone()].as_slice()].concat(),
                |deserialized: KvRecord<K, V>, _| {
                match deserialized {
                    KvRecord::Set(kv) => {
                        set_map.insert(kv.0, Some(kv.1));
                    }
                    KvRecord::Rm(k) => {
                        set_map.insert(k, None);
                    }
                }
            })?;
            let mut inner = self.inner.write()?;
            let compacted_path = KvStore::<K, V>::alloc_new_file(&inner.dir_path)?;
            let mut compacted_file = fs::File::create(&compacted_path)?;
            let mut next_offset = 0;
            for entry in &set_map {
                match entry.1 {
                    Some(v) => {
                        let serialized = rmp_serde::to_vec(&KvRecord::Set((entry.0, v)))?;
                        let value_data = ValueData {
                            offset: next_offset,
                            size: serialized.len(),
                            file_path: compacted_path.clone(),
                        };
                        inner.key_map.insert(entry.0.clone(), value_data);
                        compacted_file.write_all(&serialized)?;
                        next_offset += serialized.len() as u64;
                    }
                    None => {
                        inner.key_map.remove(entry.0);
                    }
                }
            }
            for file in &inner.inactive_files {
                fs::remove_file(file)?;
            }
            fs::remove_file(&inner.active_file)?;
            inner.active_file = compacted_path;
            inner.inactive_files = vec![];
            inner.bytes_in_last_file = next_offset;
            Ok(())
        }
    }

    impl<K, V> Drop for KvStore<K, V>
    where
        K: Key,
        V: Value,
    {
        fn drop(&mut self) {
            self.compact_files()
                .expect("Could not compact files on drop");
        }
    }
}

pub mod sled {
    use std::path::Path;

    use crate::{KvsEngine, KvsError, Result};
    use ::sled::Db;

    pub struct SledKvsEngine {
        db: Db,
    }

    impl SledKvsEngine {
        pub fn new(db_dir: &Path) -> Result<SledKvsEngine> {
            Ok(SledKvsEngine {
                db: sled::open(db_dir)?,
            })
        }
    }

    impl From<sled::Error> for KvsError {
        fn from(sled_err: sled::Error) -> Self {
            KvsError::IOError(sled_err.to_string())
        }
    }

    impl KvsEngine<String, String> for SledKvsEngine {
        fn set(&self, key: String, value: String) -> Result<()> {
            self.db.insert(key.as_bytes(), value.as_bytes())?;
            self.db.flush()?;
            Ok(())
        }
        fn get(&self, key: String) -> Result<Option<String>> {
            Ok(self
                .db
                .get(key.as_bytes())
                .map(|op| op.map(|v| String::from_utf8(v.to_vec()).unwrap()))?)
        }
        fn remove(&self, key: String) -> Result<()> {
            match self.db.remove(key.as_bytes())? {
                Some(_v) => {
                    self.db.flush()?;
                    Ok(())
                }
                None => Err(KvsError::NonExistantKey),
            }
        }
    }
    impl Drop for SledKvsEngine {
        fn drop(&mut self) {
            self.db
                .flush()
                .expect("Error flushing database when dropped");
        }
    }
}
