#[macro_use]
extern crate tracing;

use self::{
    cache::Cache,
    ctx::{Context, construct_store},
    mrf_wit::v1::fep::mrf::types::{Direction, Error as MrfError},
};
use color_eyre::{Section, eyre};
use fred::{
    clients::Pool as RedisPool, interfaces::ClientLike, types::config::Config as RedisConfig,
};
use futures_util::{StreamExt, TryStreamExt, stream};
use kitsune_config::mrf::{
    AllocationStrategy, Configuration as MrfConfiguration, FsKvStorage, KvStorage, RedisKvStorage,
};
use kitsune_derive::kitsune_service;
use kitsune_error::Error;
use kitsune_type::ap::Activity;
use mrf_manifest::{Manifest, ManifestV1};
use smol_str::SmolStr;
use std::{
    borrow::Cow,
    fmt::Debug,
    io,
    ops::ControlFlow,
    path::{Path, PathBuf},
    pin::pin,
};
use tokio::fs;
use triomphe::Arc;
use walkdir::WalkDir;
use wasmtime::{
    Config, Engine, InstanceAllocationStrategy, Store,
    component::{Component, HasSelf, Linker},
};

mod cache;
mod ctx;
mod http_client;
mod logging;
mod mrf_wit;

pub mod kv_storage;

#[inline]
async fn find_mrf_modules<P>(dir: P) -> Result<Vec<(PathBuf, Vec<u8>)>, io::Error>
where
    P: AsRef<Path>,
{
    // Read all the `.wasm` files from the disk
    // Recursively traverse the entire directory tree doing so and follow all symlinks
    let entries = WalkDir::new(dir).follow_links(true);

    let mut acc = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                debug!(?error, "failed to get file entry info");
                continue;
            }
        };

        if !entry.path().is_file() || entry.path().extension() != Some("wasm".as_ref()) {
            continue;
        }

        let path = entry.into_path();
        debug!(?path, "discovered WASM module");

        let data = fs::read(path.clone()).await?;
        acc.push((path, data));
    }

    Ok(acc)
}

#[inline]
#[cfg_attr(not(coverage), instrument(skip_all, fields(module_path = %module_path.display())))]
fn load_mrf_module(
    cache: Option<&Cache>,
    engine: &Engine,
    module_path: &Path,
    bytes: &[u8],
) -> eyre::Result<Option<(ManifestV1<'static>, Component)>> {
    let compile_component = || {
        Component::new(engine, bytes)
            .map_err(eyre::Report::msg)
            .with_note(|| format!("path to the module: {}", module_path.display()))
            .suggestion("Did you make the WASM file a component via `wasm-tools`?")
    };

    let component = if let Some(cache) = cache {
        if let Some(component) = cache.load(engine, bytes)? {
            component
        } else {
            let component = compile_component()?;
            cache.store(bytes, &component)?;
            component
        }
    } else {
        compile_component()?
    };

    let Some((manifest, _section_range)) = mrf_manifest::decode(bytes)? else {
        error!("missing manifest. skipping load.");
        return Ok(None);
    };
    let Manifest::V1(ref manifest_v1) = manifest else {
        error!("invalid manifest version. expected v1");
        return Ok(None);
    };

    info!(name = %manifest_v1.name, version = %manifest_v1.version, "loaded MRF module");

    Ok(Some((manifest_v1.to_owned(), component)))
}

#[derive(Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum Outcome<'a> {
    Accept(Cow<'a, str>),
    Reject,
}

pub struct MrfModule {
    pub component: Component,
    pub config: SmolStr,
    pub manifest: ManifestV1<'static>,
}

#[kitsune_service]
pub struct MrfService {
    engine: Engine,
    http_client: kitsune_http_client::Client,
    linker: Arc<Linker<Context>>,
    modules: Arc<[MrfModule]>,
    storage: Arc<kv_storage::BackendDispatch>,
}

impl MrfService {
    #[inline]
    pub fn from_components(
        engine: Engine,
        modules: Vec<MrfModule>,
        http_client: kitsune_http_client::Client,
        storage: kv_storage::BackendDispatch,
    ) -> eyre::Result<Self> {
        let mut linker = Linker::<Context>::new(&engine);

        wasmtime_wasi::p2::add_to_linker_async(&mut linker).map_err(eyre::Report::msg)?;
        self::mrf_wit::v1::Mrf::add_to_linker::<_, HasSelf<_>>(&mut linker, |ctx| ctx)
            .map_err(eyre::Report::msg)?;

        let this = Self::builder()
            .engine(engine)
            .http_client(http_client)
            .linker(Arc::new(linker))
            .modules(modules.into())
            .storage(Arc::new(storage))
            .build();

        Ok(this)
    }

    #[cfg_attr(not(coverage), instrument(skip_all, fields(module_dir = %config.module_dir)))]
    pub async fn from_config(
        config: &MrfConfiguration,
        http_client: kitsune_http_client::Client,
    ) -> eyre::Result<Self> {
        let cache = config
            .artifact_cache
            .as_ref()
            .map(|cache_config| Cache::open(cache_config.path.as_str()))
            .transpose()?;

        let storage = match config.storage {
            KvStorage::Fs(FsKvStorage { ref path }) => {
                kv_storage::FsBackend::from_path(path.as_str())?.into()
            }
            KvStorage::Redis(RedisKvStorage { ref url, pool_size }) => {
                let pool = RedisPool::new(
                    RedisConfig::from_url(url.as_str())?,
                    None,
                    None,
                    None,
                    pool_size.get(),
                )?;

                pool.init().await?;

                kv_storage::RedisBackend::builder()
                    .pool(pool)
                    .build()
                    .into()
            }
        };

        let allocation_strategy = match config.allocation_strategy {
            AllocationStrategy::OnDemand => InstanceAllocationStrategy::OnDemand,
            AllocationStrategy::Pooling => InstanceAllocationStrategy::pooling(),
        };

        let mut engine_config = Config::new();
        engine_config
            .allocation_strategy(allocation_strategy)
            .async_support(true)
            .wasm_component_model(true);

        let engine = Engine::new(&engine_config).map_err(eyre::Report::msg)?;

        let wasm_modules = find_mrf_modules(config.module_dir.as_str()).await?;
        let wasm_data_stream = stream::iter(wasm_modules).map(|(module_path, wasm_data)| {
            let cache = cache.as_ref();
            let engine = &engine;

            load_mrf_module(cache, engine, &module_path, &wasm_data)
        });
        let mut wasm_data_stream = pin!(wasm_data_stream);

        let mut modules = Vec::new();
        while let Some(maybe_module) = wasm_data_stream.try_next().await? {
            let Some((manifest, component)) = maybe_module else {
                continue;
            };

            // TODO: permission grants, etc.

            let span = info_span!(
                "load_mrf_module_config",
                name = %manifest.name,
                version = %manifest.version,
            );

            let config = span.in_scope(|| {
                config
                    .module_config
                    .get(&*manifest.name)
                    .cloned()
                    .inspect(|_| debug!("found configuration"))
                    .unwrap_or_else(|| {
                        debug!("didn't find configuration. defaulting to empty string");
                        SmolStr::default()
                    })
            });

            let module = MrfModule {
                component,
                config,
                manifest,
            };

            modules.push(module);
        }

        Self::from_components(engine, modules, http_client, storage)
    }

    #[must_use]
    pub fn module_count(&self) -> usize {
        self.modules.len()
    }

    #[instrument(
        skip_all,
        fields(module_name = %module.manifest.name),
    )]
    async fn invoke_module<'a>(
        &self,
        mut store: &mut Store<Context>,
        module: &MrfModule,
        direction: Direction,
        activity: Cow<'a, str>,
    ) -> Result<ControlFlow<(), Cow<'a, str>>, Error> {
        let mrf = mrf_wit::v1::Mrf::instantiate_async(&mut store, &module.component, &self.linker)
            .await
            .map_err(Error::msg)?;

        store.data_mut().kv_ctx.module_name = Some(module.manifest.name.to_string());

        let result = mrf
            .call_transform(store, &module.config, direction, &activity)
            .await
            .map_err(Error::msg)?;

        match result {
            Ok(transformed) => {
                return Ok(ControlFlow::Continue(Cow::Owned(transformed)));
            }
            Err(MrfError::ErrorContinue(msg)) => {
                error!(%msg, "MRF errored out. Continuing.");
            }
            Err(MrfError::ErrorReject(msg)) => {
                error!(%msg, "MRF errored out. Aborting.");
                return Ok(ControlFlow::Break(()));
            }
            Err(MrfError::Reject) => {
                error!("MRF rejected activity. Aborting.");
                return Ok(ControlFlow::Break(()));
            }
        }

        Ok(ControlFlow::Continue(activity))
    }

    async fn handle<'a>(
        &self,
        direction: Direction,
        activity_type: &str,
        activity: &'a str,
    ) -> Result<Outcome<'a>, Error> {
        let mut store =
            construct_store(&self.engine, self.http_client.clone(), self.storage.clone());
        let mut activity = Cow::Borrowed(activity);

        for module in self.modules.iter() {
            let activity_types = &module.manifest.activity_types;
            if !activity_types.all_activities() && !activity_types.contains(activity_type) {
                continue;
            }

            let outcome = self
                .invoke_module(&mut store, module, direction, activity)
                .await?;

            match outcome {
                ControlFlow::Break(()) => return Ok(Outcome::Reject),
                ControlFlow::Continue(transformed_activity) => activity = transformed_activity,
            }
        }

        Ok(Outcome::Accept(activity))
    }

    #[inline]
    pub async fn handle_incoming<'a>(
        &self,
        activity_type: &str,
        activity: &'a str,
    ) -> Result<Outcome<'a>, Error> {
        self.handle(Direction::Incoming, activity_type, activity)
            .await
    }

    #[inline]
    pub async fn handle_outgoing(&self, activity: &Activity) -> Result<Outcome<'static>, Error> {
        let serialised = sonic_rs::to_string(activity)?;
        let outcome = self
            .handle(Direction::Outgoing, activity.r#type.as_ref(), &serialised)
            .await?;

        let outcome: Outcome<'static> = match outcome {
            Outcome::Accept(Cow::Borrowed(..)) => {
                // As per the logic in the previous function, we can assume that if the Cow is owned, it has been modified
                // If it hasn't been modified it is in its borrowed state
                //
                // Therefore we don't need to allocate again here, simply reconstruct a new `Outcome` with an owned Cow.
                Outcome::Accept(Cow::Owned(serialised))
            }
            Outcome::Accept(Cow::Owned(owned)) => Outcome::Accept(Cow::Owned(owned)),
            Outcome::Reject => Outcome::Reject,
        };

        Ok(outcome)
    }
}
