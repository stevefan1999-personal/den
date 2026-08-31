use std::{
    cell::RefCell,
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, RwLock, Weak},
};

use rquickjs::{
    AsyncContext, Ctx, Exception, JsLifetime, Result, TypedArray, Value, class::Trace,
    runtime::UserDataError,
};
use surrealkv::{Durability, Error as StorageError, Mode, Transaction, Tree, TreeBuilder};
use tokio::{
    runtime::Handle,
    sync::{Mutex as AsyncMutex, watch},
    task::{JoinError, spawn_blocking},
};

pub use crate::js_kv_module as js_kv;

type KvResult<T> = std::result::Result<T, KvError>;

#[derive(Debug, thiserror::Error)]
enum KvError {
    #[error("{0} is detached")]
    Detached(&'static str),
    #[error("{0} must not be empty")]
    Empty(&'static str),
    #[error("{name} exceeds {max} bytes")]
    ByteLimit { name: &'static str, max: usize },
    #[error("{0} is closed")]
    Closed(&'static str),
    #[error("KV registry is missing")]
    RegistryMissing,
    #[error("{0} lock is poisoned")]
    Poisoned(&'static str),
    #[error("KV blocking task failed")]
    Join(#[from] JoinError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(
        "transaction stages {bytes} bytes across {entries} entries; limits are {max_bytes} bytes \
         and {max_entries} entries"
    )]
    TransactionTooLarge {
        bytes:       usize,
        entries:     usize,
        max_bytes:   usize,
        max_entries: usize,
    },
}

impl KvError {
    fn throw(&self, ctx: &Ctx<'_>) -> rquickjs::Error {
        match self {
            Self::Detached(_) | Self::Empty(_) | Self::Closed(_) => {
                Exception::throw_type(ctx, &self.to_string())
            }
            Self::Poisoned(_) | Self::Join(_) | Self::RegistryMissing => {
                Exception::throw_internal(ctx, &self.to_string())
            }
            Self::ByteLimit { .. } | Self::TransactionTooLarge { .. } => {
                Exception::throw_range(ctx, &self.to_string())
            }
            Self::Storage(error) => den_util::stack::throw_error(ctx, &error.to_string()),
        }
    }
}

struct TransactionState {
    transaction:  Option<Transaction>,
    staged:       HashMap<Vec<u8>, usize>,
    staged_bytes: usize,
}

struct TransactionSlot {
    state: Mutex<TransactionState>,
}

type CloseOutcome = std::result::Result<(), Arc<KvError>>;

enum CloseState {
    Open,
    Closing(watch::Receiver<Option<CloseOutcome>>),
    Closed(CloseOutcome),
}

impl TransactionState {
    fn new(transaction: Transaction) -> Self {
        Self {
            transaction:  Some(transaction),
            staged:       HashMap::new(),
            staged_bytes: 0,
        }
    }

    fn transaction(&self) -> KvResult<&Transaction> {
        self.transaction
            .as_ref()
            .ok_or(KvError::Closed("KV transaction"))
    }

    fn transaction_mut(&mut self) -> KvResult<&mut Transaction> {
        self.transaction
            .as_mut()
            .ok_or(KvError::Closed("KV transaction"))
    }

    fn staged_size(&self, key: &[u8], value_len: usize) -> (usize, usize, usize) {
        let record_bytes = KvStore::RECORD_OVERHEAD_BYTES
            .saturating_add(key.len())
            .saturating_add(value_len);
        let previous = self.staged.get(key).copied().unwrap_or_default();
        let bytes = self
            .staged_bytes
            .saturating_sub(previous)
            .saturating_add(record_bytes);
        let entries = self.staged.len() + usize::from(previous == 0);
        (record_bytes, bytes, entries)
    }

    const fn check_size(bytes: usize, entries: usize) -> KvResult<()> {
        if bytes > KvStore::MAX_TRANSACTION_BYTES || entries > KvStore::MAX_TRANSACTION_ENTRIES {
            return Err(KvError::TransactionTooLarge {
                bytes,
                entries,
                max_bytes: KvStore::MAX_TRANSACTION_BYTES,
                max_entries: KvStore::MAX_TRANSACTION_ENTRIES,
            });
        }
        Ok(())
    }

    fn set(&mut self, key: Vec<u8>, value: Vec<u8>) -> KvResult<()> {
        let (record_bytes, bytes, entries) = self.staged_size(&key, value.len());
        Self::check_size(bytes, entries)?;
        self.transaction_mut()?.set(key.as_slice(), value)?;
        self.staged.insert(key, record_bytes);
        self.staged_bytes = bytes;
        Ok(())
    }

    fn delete(&mut self, key: Vec<u8>) -> KvResult<()> {
        let (record_bytes, bytes, entries) = self.staged_size(&key, 0);
        Self::check_size(bytes, entries)?;
        self.transaction_mut()?.delete(key.as_slice())?;
        self.staged.insert(key, record_bytes);
        self.staged_bytes = bytes;
        Ok(())
    }

    fn rollback(&mut self) {
        if let Some(mut transaction) = self.transaction.take() {
            transaction.rollback();
        }
        self.staged.clear();
        self.staged_bytes = 0;
    }
}

struct KvStore {
    tree:         RwLock<Option<Tree>>,
    transactions: Mutex<Vec<Weak<TransactionSlot>>>,
    close:        AsyncMutex<CloseState>,
    /// SurrealKV's `Tree::drop` starts another asynchronous close. Retaining
    /// the already-closed value keeps that second pass after the close every
    /// caller awaited.
    retired:      Mutex<Option<Tree>>,
}

impl KvStore {
    const IMPLICIT_ATTEMPTS: usize = 16;
    const MAX_KEY_BYTES: usize = 2_048;
    /// Deliberately below SurrealKV's 100 MiB default memtable. The estimate
    /// includes a conservative per-record allowance for its skip-list node.
    const MAX_TRANSACTION_BYTES: usize = 16 * 1024 * 1024;
    const MAX_TRANSACTION_ENTRIES: usize = 1_000;
    const MAX_VALUE_BYTES: usize = 64 * 1024;
    const RECORD_OVERHEAD_BYTES: usize = 256;
    const USER_PREFIX: u8 = 1;

    async fn open(path: PathBuf) -> KvResult<Arc<Self>> {
        let runtime = Handle::current();
        let tree = Self::run_blocking(move || {
            let _entered = runtime.enter();
            TreeBuilder::new()
                .with_path(path)
                .build()
                .map_err(Into::into)
        })
        .await?;
        Ok(Arc::new(Self {
            tree:         RwLock::new(Some(tree)),
            transactions: Mutex::new(Vec::new()),
            close:        AsyncMutex::new(CloseState::Open),
            retired:      Mutex::new(None),
        }))
    }

    async fn run_blocking<T, F>(operation: F) -> KvResult<T>
    where
        T: Send + 'static,
        F: FnOnce() -> KvResult<T> + Send + 'static,
    {
        spawn_blocking(operation).await.map_err(KvError::from)?
    }

    async fn with_tree<T, F>(self: &Arc<Self>, operation: F) -> KvResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&Tree) -> KvResult<T> + Send + 'static,
    {
        let store = Arc::clone(self);
        Self::run_blocking(move || {
            let tree = store
                .tree
                .read()
                .map_err(|_error| KvError::Poisoned("KV"))?;
            operation(tree.as_ref().ok_or(KvError::Closed("KV"))?)
        })
        .await
    }

    async fn update<F>(self: &Arc<Self>, operation: F) -> KvResult<()>
    where
        F: Fn(&mut Transaction) -> surrealkv::Result<()> + Send + 'static,
    {
        let runtime = Handle::current();
        self.with_tree(move |tree| {
            for attempt in 0..Self::IMPLICIT_ATTEMPTS {
                let mut transaction = tree.begin()?;
                transaction.set_durability(Durability::Immediate);
                operation(&mut transaction)?;
                match runtime.block_on(transaction.commit()) {
                    Ok(()) => return Ok(()),
                    Err(
                        StorageError::TransactionWriteConflict | StorageError::TransactionRetry,
                    ) if attempt + 1 < Self::IMPLICIT_ATTEMPTS => std::thread::yield_now(),
                    Err(error) => return Err(error.into()),
                }
            }
            unreachable!("the final implicit transaction attempt returns")
        })
        .await
    }

    async fn get(self: &Arc<Self>, key: Vec<u8>) -> KvResult<Option<Vec<u8>>> {
        self.with_tree(move |tree| {
            tree.begin_with_mode(Mode::ReadOnly)?
                .get(key)
                .map_err(Into::into)
        })
        .await
    }

    async fn set(self: &Arc<Self>, key: Vec<u8>, value: Vec<u8>) -> KvResult<()> {
        self.update(move |transaction| transaction.set(key.as_slice(), value.as_slice()))
            .await
    }

    async fn delete(self: &Arc<Self>, key: Vec<u8>) -> KvResult<()> {
        self.update(move |transaction| transaction.delete(key.as_slice()))
            .await
    }

    async fn begin(self: &Arc<Self>) -> KvResult<Arc<TransactionSlot>> {
        let store = Arc::clone(self);
        Self::run_blocking(move || {
            let tree = store
                .tree
                .read()
                .map_err(|_error| KvError::Poisoned("KV"))?;
            let mut transaction = tree.as_ref().ok_or(KvError::Closed("KV"))?.begin()?;
            transaction.set_durability(Durability::Immediate);
            let slot = Arc::new(TransactionSlot {
                state: Mutex::new(TransactionState::new(transaction)),
            });
            let mut transactions = store
                .transactions
                .lock()
                .map_err(|_error| KvError::Poisoned("KV transaction registry"))?;
            transactions.retain(|transaction| transaction.strong_count() != 0);
            transactions.push(Arc::downgrade(&slot));
            drop(transactions);
            drop(tree);
            Ok(slot)
        })
        .await
    }

    async fn with_transaction<T, F>(
        self: &Arc<Self>, slot: Arc<TransactionSlot>, operation: F,
    ) -> KvResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut TransactionState) -> KvResult<T> + Send + 'static,
    {
        self.with_tree(move |_tree| {
            let mut state = slot
                .state
                .lock()
                .map_err(|_error| KvError::Poisoned("KV transaction"))?;
            operation(&mut state)
        })
        .await
    }

    async fn commit_transaction(self: &Arc<Self>, slot: Arc<TransactionSlot>) -> KvResult<bool> {
        let runtime = Handle::current();
        self.with_tree(move |_tree| {
            let mut state = slot
                .state
                .lock()
                .map_err(|_error| KvError::Poisoned("KV transaction"))?;
            if let Err(error) = TransactionState::check_size(state.staged_bytes, state.staged.len())
            {
                state.rollback();
                return Err(error);
            }
            let result = runtime.block_on(state.transaction_mut()?.commit());
            state.rollback();
            drop(state);
            match result {
                Ok(()) => Ok(true),
                Err(StorageError::TransactionWriteConflict | StorageError::TransactionRetry) => {
                    Ok(false)
                }
                Err(error) => Err(error.into()),
            }
        })
        .await
    }

    async fn rollback_transaction(slot: Arc<TransactionSlot>) -> KvResult<()> {
        Self::run_blocking(move || {
            slot.state
                .lock()
                .map_err(|_error| KvError::Poisoned("KV transaction"))?
                .rollback();
            Ok(())
        })
        .await
    }

    async fn finish_close(self: &Arc<Self>) -> KvResult<()> {
        let store = Arc::clone(self);
        let tree = Self::run_blocking(move || {
            let tree = {
                let mut active = store
                    .tree
                    .write()
                    .map_err(|_error| KvError::Poisoned("KV"))?;
                active.take()
            };
            let Some(tree) = tree else {
                return Ok(None);
            };
            let mut transactions = store
                .transactions
                .lock()
                .map_err(|_error| KvError::Poisoned("KV transaction registry"))?;
            for slot in transactions.drain(..).filter_map(|slot| slot.upgrade()) {
                let mut state = slot
                    .state
                    .lock()
                    .map_err(|_error| KvError::Poisoned("KV transaction"))?;
                state.rollback();
            }
            drop(transactions);
            Ok(Some(tree))
        })
        .await?;
        if let Some(tree) = tree {
            let result = tree.close().await;
            self.retired
                .lock()
                .map_err(|_error| KvError::Poisoned("retired KV"))?
                .replace(tree);
            result?;
        }
        Ok(())
    }

    async fn close(self: &Arc<Self>) -> CloseOutcome {
        let mut receiver = {
            let mut state = self.close.lock().await;
            match &*state {
                CloseState::Closed(outcome) => return outcome.clone(),
                CloseState::Closing(receiver) => receiver.clone(),
                CloseState::Open => {
                    let (sender, receiver) = watch::channel(None);
                    *state = CloseState::Closing(receiver.clone());
                    drop(state);
                    let store = Arc::clone(self);
                    tokio::spawn(async move {
                        let outcome = store.finish_close().await.map_err(Arc::new);
                        *store.close.lock().await = CloseState::Closed(outcome.clone());
                        let _ = sender.send(Some(outcome));
                    });
                    receiver
                }
            }
        };
        let observed = receiver
            .wait_for(Option::is_some)
            .await
            .map_err(|_error| Arc::new(KvError::Poisoned("KV close barrier")))?;
        observed
            .as_ref()
            .cloned()
            .ok_or_else(|| Arc::new(KvError::Poisoned("KV close barrier")))?
    }

    fn key(ctx: &Ctx<'_>, key: &TypedArray<'_, u8>) -> Result<Vec<u8>> {
        let key = key
            .as_bytes()
            .ok_or_else(|| KvError::Detached("key").throw(ctx))?;
        if key.is_empty() {
            return Err(KvError::Empty("key").throw(ctx));
        }
        if key.len() > Self::MAX_KEY_BYTES {
            return Err(KvError::ByteLimit {
                name: "key",
                max:  Self::MAX_KEY_BYTES,
            }
            .throw(ctx));
        }
        let mut encoded = Vec::with_capacity(key.len() + 1);
        encoded.push(Self::USER_PREFIX);
        encoded.extend_from_slice(key);
        Ok(encoded)
    }

    fn value(ctx: &Ctx<'_>, value: &TypedArray<'_, u8>) -> Result<Vec<u8>> {
        let value = value
            .as_bytes()
            .ok_or_else(|| KvError::Detached("value").throw(ctx))?;
        if value.len() > Self::MAX_VALUE_BYTES {
            return Err(KvError::ByteLimit {
                name: "value",
                max:  Self::MAX_VALUE_BYTES,
            }
            .throw(ctx));
        }
        Ok(value.to_vec())
    }

    fn js_value(ctx: Ctx<'_>, value: Option<Vec<u8>>) -> Result<Value<'_>> {
        match value {
            Some(value) => TypedArray::new_copy(ctx, value).map(TypedArray::into_value),
            None => Ok(Value::new_null(ctx)),
        }
    }
}

impl Drop for KvStore {
    fn drop(&mut self) {
        if let Ok(tree) = self.tree.get_mut()
            && let Some(tree) = tree.as_ref()
            && let Err(error) = tree.flush_wal(true)
        {
            log::error!("could not flush den:kv during drop: {error}");
        }
    }
}

/// A persistent, transaction-capable byte key/value store.
#[derive(Clone, Trace)]
#[rquickjs::class(rename = "Kv")]
pub struct Kv {
    #[qjs(skip_trace)]
    store: Arc<KvStore>,
}

// SAFETY: `Kv` contains no JS values or references.
unsafe impl JsLifetime<'_> for Kv {
    type Changed<'to> = Kv;
}

#[rquickjs::methods(rename_all = "camelCase")]
impl Kv {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> {
        Err(Exception::throw_type(&ctx, "Illegal constructor"))
    }

    #[qjs(static)]
    pub async fn open(path: String, ctx: Ctx<'_>) -> Result<Self> {
        if path.is_empty() {
            return Err(KvError::Empty("path").throw(&ctx));
        }
        let store = KvStore::open(path.into())
            .await
            .map_err(|error| error.throw(&ctx))?;
        KvRegistry::register(&ctx, Arc::clone(&store))?;
        Ok(Self { store })
    }

    pub async fn get<'js>(self, key: TypedArray<'js, u8>, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let key = KvStore::key(&ctx, &key)?;
        let value = self
            .store
            .get(key)
            .await
            .map_err(|error| error.throw(&ctx))?;
        KvStore::js_value(ctx, value)
    }

    pub async fn set(
        self, key: TypedArray<'_, u8>, value: TypedArray<'_, u8>, ctx: Ctx<'_>,
    ) -> Result<()> {
        let key = KvStore::key(&ctx, &key)?;
        let value = KvStore::value(&ctx, &value)?;
        self.store
            .set(key, value)
            .await
            .map_err(|error| error.throw(&ctx))
    }

    pub async fn delete(self, key: TypedArray<'_, u8>, ctx: Ctx<'_>) -> Result<()> {
        let key = KvStore::key(&ctx, &key)?;
        self.store
            .delete(key)
            .await
            .map_err(|error| error.throw(&ctx))
    }

    pub async fn transaction(self, ctx: Ctx<'_>) -> Result<KvTransaction> {
        let slot = self
            .store
            .begin()
            .await
            .map_err(|error| error.throw(&ctx))?;
        Ok(KvTransaction {
            slot,
            store: self.store,
        })
    }

    pub async fn close(self, ctx: Ctx<'_>) -> Result<()> {
        self.store
            .close()
            .await
            .map_err(|error| error.throw(&ctx))?;
        KvRegistry::unregister(&ctx, &self.store)
    }
}

/// An explicit SurrealKV snapshot transaction. A commit attempt is terminal.
#[derive(Clone, Trace)]
#[rquickjs::class(rename = "KvTransaction")]
pub struct KvTransaction {
    #[qjs(skip_trace)]
    slot:  Arc<TransactionSlot>,
    #[qjs(skip_trace)]
    store: Arc<KvStore>,
}

// SAFETY: `KvTransaction` contains no JS values or references.
unsafe impl JsLifetime<'_> for KvTransaction {
    type Changed<'to> = KvTransaction;
}

#[rquickjs::methods(rename_all = "camelCase")]
impl KvTransaction {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> {
        Err(Exception::throw_type(&ctx, "Illegal constructor"))
    }

    pub async fn get<'js>(self, key: TypedArray<'js, u8>, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let key = KvStore::key(&ctx, &key)?;
        let value = self
            .store
            .with_transaction(Arc::clone(&self.slot), move |state| {
                state.transaction()?.get(key).map_err(Into::into)
            })
            .await
            .map_err(|error| error.throw(&ctx))?;
        KvStore::js_value(ctx, value)
    }

    pub async fn get_for_update<'js>(
        self, key: TypedArray<'js, u8>, ctx: Ctx<'js>,
    ) -> Result<Value<'js>> {
        let key = KvStore::key(&ctx, &key)?;
        let value = self
            .store
            .with_transaction(Arc::clone(&self.slot), move |state| {
                state
                    .transaction_mut()?
                    .get_for_update(key)
                    .map_err(Into::into)
            })
            .await
            .map_err(|error| error.throw(&ctx))?;
        KvStore::js_value(ctx, value)
    }

    pub async fn set(
        self, key: TypedArray<'_, u8>, value: TypedArray<'_, u8>, ctx: Ctx<'_>,
    ) -> Result<()> {
        let key = KvStore::key(&ctx, &key)?;
        let value = KvStore::value(&ctx, &value)?;
        self.store
            .with_transaction(Arc::clone(&self.slot), move |state| state.set(key, value))
            .await
            .map_err(|error| error.throw(&ctx))
    }

    pub async fn delete(self, key: TypedArray<'_, u8>, ctx: Ctx<'_>) -> Result<()> {
        let key = KvStore::key(&ctx, &key)?;
        self.store
            .with_transaction(Arc::clone(&self.slot), move |state| state.delete(key))
            .await
            .map_err(|error| error.throw(&ctx))
    }

    pub async fn commit(self, ctx: Ctx<'_>) -> Result<bool> {
        self.store
            .commit_transaction(Arc::clone(&self.slot))
            .await
            .map_err(|error| error.throw(&ctx))
    }

    pub async fn rollback(self, ctx: Ctx<'_>) -> Result<()> {
        KvStore::rollback_transaction(Arc::clone(&self.slot))
            .await
            .map_err(|error| error.throw(&ctx))
    }
}

/// Stores opened by one realm, retained so engine shutdown can close them.
#[derive(Default)]
pub struct KvRegistry {
    stores: RefCell<Vec<Arc<KvStore>>>,
}

// SAFETY: the registry contains no JS values or references.
unsafe impl JsLifetime<'_> for KvRegistry {
    type Changed<'to> = KvRegistry;
}

impl KvRegistry {
    fn install(ctx: &Ctx<'_>) -> Result<()> {
        if ctx.userdata::<Self>().is_some() {
            return Ok(());
        }
        ctx.store_userdata(Self::default())
            .map(|_| ())
            .map_err(|_error| rquickjs::Error::UserData(UserDataError(())))
    }

    fn register(ctx: &Ctx<'_>, store: Arc<KvStore>) -> Result<()> {
        Self::install(ctx)?;
        ctx.userdata::<Self>()
            .ok_or_else(|| KvError::RegistryMissing.throw(ctx))?
            .stores
            .try_borrow_mut()
            .map_err(|_error| KvError::Poisoned("KV registry").throw(ctx))?
            .push(store);
        Ok(())
    }

    fn unregister(ctx: &Ctx<'_>, store: &Arc<KvStore>) -> Result<()> {
        let Some(registry) = ctx.userdata::<Self>() else {
            return Ok(());
        };
        registry
            .stores
            .try_borrow_mut()
            .map_err(|_error| KvError::Poisoned("KV registry").throw(ctx))?
            .retain(|registered| !Arc::ptr_eq(registered, store));
        Ok(())
    }

    fn take(ctx: &Ctx<'_>) -> Result<Vec<Arc<KvStore>>> {
        let Some(registry) = ctx.userdata::<Self>() else {
            return Ok(Vec::new());
        };
        let mut stores = registry
            .stores
            .try_borrow_mut()
            .map_err(|_error| KvError::Poisoned("KV registry").throw(ctx))?;
        Ok(std::mem::take(&mut *stores))
    }

    pub async fn shutdown(context: &AsyncContext) {
        let stores = match context.with(|ctx| Self::take(&ctx)).await {
            Ok(stores) => stores,
            Err(error) => {
                log::error!("could not access den:kv during engine shutdown: {error}");
                return;
            }
        };
        for store in stores {
            if let Err(error) = store.close().await {
                log::error!("could not close den:kv during engine shutdown: {error}");
            }
        }
    }
}

#[rquickjs::module(rename_vars = "camelCase", rename_types = "PascalCase")]
pub mod kv_module {
    use rquickjs::{Ctx, Result, module::Exports};

    pub use super::{Kv, KvTransaction};

    #[qjs(evaluate)]
    pub fn evaluate(ctx: &Ctx<'_>, _exports: &Exports<'_>) -> Result<()> {
        super::KvRegistry::install(ctx)
    }
}
