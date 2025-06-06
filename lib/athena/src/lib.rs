#[macro_use]
extern crate tracing;

use self::error::{BoxError, Result};
use async_trait::async_trait;
use futures_util::Stream;
use iso8601_timestamp::Timestamp;
use serde::{Deserialize, Serialize};
use speedy_uuid::Uuid;
use std::{
    any::Any,
    fmt::{self, Debug},
};
use triomphe::Arc;
use typed_builder::TypedBuilder;
use unsize::{CoerceUnsize, Coercion};

pub use self::error::Error;
pub use tokio_util::task::TaskTracker;

pub use self::common::spawn_jobs;
#[cfg(feature = "redis")]
pub use self::redis::JobQueue as RedisJobQueue;

mod common;
mod error;
#[cfg(feature = "redis")]
mod redis;

pub mod consts;

#[derive(TypedBuilder)]
#[non_exhaustive]
pub struct JobDetails<C> {
    #[builder(setter(into))]
    pub context: C,
    #[builder(default)]
    pub fail_count: u32,
    #[builder(default = Uuid::now_v7(), setter(into))]
    pub job_id: Uuid,
    #[builder(default, setter(into))]
    pub run_at: Option<Timestamp>,
}

#[typetag::serde]
pub trait Keepable: Any + Send + Sync + 'static {}

#[typetag::serde]
impl Keepable for String {}

#[derive(Deserialize, Serialize)]
#[serde(transparent)]
pub struct KeeperOfTheSecrets {
    inner: Option<Box<dyn Keepable>>,
}

impl KeeperOfTheSecrets {
    #[inline]
    #[must_use]
    pub fn empty() -> Self {
        Self { inner: None }
    }

    #[inline]
    pub fn new<T>(inner: T) -> Self
    where
        T: Keepable,
    {
        Self {
            inner: Some(Box::new(inner)),
        }
    }

    #[inline]
    #[must_use]
    pub fn get<T>(&self) -> Option<&T>
    where
        T: Keepable + 'static,
    {
        self.inner
            .as_deref()
            .and_then(|item| (item as &dyn Any).downcast_ref())
    }
}

impl Debug for KeeperOfTheSecrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .finish_non_exhaustive()
    }
}

pub enum Outcome {
    Success,
    Fail { fail_count: u32 },
}

pub struct JobResult<'a> {
    pub outcome: Outcome,
    pub job_id: Uuid,
    pub ctx: &'a KeeperOfTheSecrets,
}

#[derive(Deserialize, Serialize)]
pub struct JobData {
    pub job_id: Uuid,
    pub fail_count: u32,
    pub ctx: KeeperOfTheSecrets,
}

#[async_trait]
pub trait JobQueue: Send + Sync + 'static {
    type ContextRepository: JobContextRepository + 'static;

    fn context_repository(&self) -> &Self::ContextRepository;

    async fn enqueue(
        &self,
        job_details: JobDetails<<Self::ContextRepository as JobContextRepository>::JobContext>,
    ) -> Result<()>;

    async fn fetch_job_data(&self, max_jobs: usize) -> Result<Vec<JobData>>;

    async fn reclaim_job(&self, job_data: &JobData) -> Result<()>;

    async fn complete_job(&self, state: &JobResult<'_>) -> Result<()>;
}

pub trait Coerce<CR> {
    fn coerce<'a>(self) -> Arc<dyn JobQueue<ContextRepository = CR> + 'a>;
}

impl<T> Coerce<T::ContextRepository> for Arc<T>
where
    T: JobQueue,
{
    fn coerce<'a>(self) -> Arc<dyn JobQueue<ContextRepository = T::ContextRepository> + 'a> {
        #[allow(unsafe_code)]
        let coercion: Coercion<T, _, _> = unsafe {
            Coercion::new({
                fn coerce<'lt, T: JobQueue + 'lt>(
                    p: *const T,
                ) -> *const (dyn JobQueue<ContextRepository = T::ContextRepository> + 'lt)
                {
                    p
                }
                coerce
            })
        };

        self.unsize(coercion)
    }
}

#[async_trait]
impl<CR> JobQueue for Arc<dyn JobQueue<ContextRepository = CR> + '_>
where
    CR: JobContextRepository + 'static,
{
    type ContextRepository = CR;

    fn context_repository(&self) -> &Self::ContextRepository {
        (**self).context_repository()
    }

    async fn enqueue(
        &self,
        job_details: JobDetails<<Self::ContextRepository as JobContextRepository>::JobContext>,
    ) -> Result<()> {
        (**self).enqueue(job_details).await
    }

    async fn fetch_job_data(&self, max_jobs: usize) -> Result<Vec<JobData>> {
        (**self).fetch_job_data(max_jobs).await
    }

    async fn reclaim_job(&self, job_data: &JobData) -> Result<()> {
        (**self).reclaim_job(job_data).await
    }

    async fn complete_job(&self, state: &JobResult<'_>) -> Result<()> {
        (**self).complete_job(state).await
    }
}

pub trait Runnable {
    /// User-defined context that is getting passed to the job when run
    ///
    /// This way you can reference services, configurations, etc.
    type Context: Send + Sync + 'static;

    type Error: Into<BoxError> + Send;

    /// Run the job
    fn run(&self, ctx: &Self::Context) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

pub trait JobContextRepository {
    /// Some job context
    ///
    /// To support multiple job types per repository, consider using the enum dispatch technique
    type JobContext: Runnable + Send + Sync + 'static;
    type Error: Into<BoxError>;
    type Stream: Stream<Item = Result<(Uuid, Self::JobContext), Self::Error>> + Send;

    /// Batch fetch job contexts
    ///
    /// The stream has to return `([Job ID], [Job context])`, this gives you the advantage that the order isn't enforced.
    /// You can return them as you find them
    fn fetch_context<I>(
        &self,
        job_ids: I,
    ) -> impl Future<Output = Result<Self::Stream, Self::Error>> + Send
    where
        I: Iterator<Item = Uuid> + Send + 'static;

    /// Remove job context from the database
    fn remove_context(&self, job_id: Uuid) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Store job context into the database
    ///
    /// Make sure the job can be efficiently found via the job ID (such as using the job ID as the primary key of a database table)
    fn store_context(
        &self,
        job_id: Uuid,
        context: Self::JobContext,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}
