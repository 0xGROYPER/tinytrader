//! The Databento calls this service makes, behind a trait so the purchase
//! paths can be tested against a fake Databento.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use databento::HistoricalClient;
use databento::dbn::Schema;
use databento::historical::batch::{
    BatchFileDesc, BatchJob, BatchJobShort, JobState, ListJobsParams, SubmitJobParams,
};
use databento::historical::metadata::{
    DatasetConditionDetail, DatasetRange, GetCostParams, GetDatasetConditionParams,
};

pub type Fut<'a, T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send + 'a>>;

pub trait Databento: Send + Sync {
    fn list_jobs<'a>(&'a self, p: &'a ListJobsParams) -> Fut<'a, Vec<BatchJobShort>>;
    fn job<'a>(&'a self, id: &'a str) -> Fut<'a, BatchJob>;
    fn files<'a>(&'a self, id: &'a str) -> Fut<'a, Vec<BatchFileDesc>>;
    fn submit<'a>(&'a self, p: &'a SubmitJobParams) -> Fut<'a, BatchJob>;
    /// The price in US dollars and the billable size in bytes.
    fn cost<'a>(&'a self, p: &'a GetCostParams) -> Fut<'a, (f64, u64)>;
    fn datasets(&self) -> Fut<'_, Vec<String>>;
    fn schemas<'a>(&'a self, dataset: &'a str) -> Fut<'a, Vec<Schema>>;
    fn range<'a>(&'a self, dataset: &'a str) -> Fut<'a, DatasetRange>;
    fn conditions<'a>(
        &'a self,
        p: &'a GetDatasetConditionParams,
    ) -> Fut<'a, Vec<DatasetConditionDetail>>;
}

/// Every job in every state over the account's whole history: what was
/// bought can't be judged from part of the list.
#[must_use]
pub fn every_job() -> ListJobsParams {
    ListJobsParams {
        states: Some(vec![
            JobState::Queued,
            JobState::Processing,
            JobState::Done,
            JobState::Expired,
        ]),
        since: Some(time::macros::datetime!(2000-01-01 0:00 UTC)),
    }
}

/// Databento itself.
pub struct Live(pub tokio::sync::Mutex<HistoricalClient>);

macro_rules! call {
    ($self:ident, |$c:ident| $body:expr) => {
        Box::pin(async move {
            let mut $c = $self.0.lock().await;
            $body.await.map_err(|e| e.to_string())
        })
    };
}

impl Databento for Live {
    fn list_jobs<'a>(&'a self, p: &'a ListJobsParams) -> Fut<'a, Vec<BatchJobShort>> {
        call!(self, |c| c.batch().list_jobs(p))
    }
    fn job<'a>(&'a self, id: &'a str) -> Fut<'a, BatchJob> {
        call!(self, |c| c.batch().get_job_details(id))
    }
    fn files<'a>(&'a self, id: &'a str) -> Fut<'a, Vec<BatchFileDesc>> {
        call!(self, |c| c.batch().list_files(id))
    }
    fn submit<'a>(&'a self, p: &'a SubmitJobParams) -> Fut<'a, BatchJob> {
        call!(self, |c| c.batch().submit_job(p))
    }
    fn cost<'a>(&'a self, p: &'a GetCostParams) -> Fut<'a, (f64, u64)> {
        Box::pin(async move {
            let mut c = self.0.lock().await;
            let usd = c.metadata().get_cost(p).await.map_err(|e| e.to_string())?;
            let bytes = c
                .metadata()
                .get_billable_size(p)
                .await
                .map_err(|e| e.to_string())?;
            Ok((usd, bytes))
        })
    }
    fn datasets(&self) -> Fut<'_, Vec<String>> {
        call!(self, |c| c.metadata().list_datasets(None))
    }
    fn schemas<'a>(&'a self, dataset: &'a str) -> Fut<'a, Vec<Schema>> {
        call!(self, |c| c.metadata().list_schemas(dataset))
    }
    fn range<'a>(&'a self, dataset: &'a str) -> Fut<'a, DatasetRange> {
        call!(self, |c| c.metadata().get_dataset_range(dataset))
    }
    fn conditions<'a>(
        &'a self,
        p: &'a GetDatasetConditionParams,
    ) -> Fut<'a, Vec<DatasetConditionDetail>> {
        call!(self, |c| c.metadata().get_dataset_condition(p))
    }
}

/// Databento with every call bounded in time, so a hung call never holds
/// the purchase lock: a read gives up after `read`, a submission after
/// `submit` (its outcome then unknown, so written `uncertain`).
pub struct Timed {
    pub inner: Arc<dyn Databento>,
    pub read: Duration,
    pub submit: Duration,
}

impl Timed {
    #[must_use]
    pub fn new(inner: Arc<dyn Databento>) -> Self {
        Self {
            inner,
            read: Duration::from_secs(60),
            submit: Duration::from_secs(120),
        }
    }
}

fn within<'a, T: 'a>(d: Duration, f: Fut<'a, T>) -> Fut<'a, T> {
    Box::pin(async move {
        tokio::time::timeout(d, f)
            .await
            .map_err(|_| format!("no answer from Databento in {} s", d.as_secs_f64()))?
    })
}

impl Databento for Timed {
    fn list_jobs<'a>(&'a self, p: &'a ListJobsParams) -> Fut<'a, Vec<BatchJobShort>> {
        within(self.read, self.inner.list_jobs(p))
    }
    fn job<'a>(&'a self, id: &'a str) -> Fut<'a, BatchJob> {
        within(self.read, self.inner.job(id))
    }
    fn files<'a>(&'a self, id: &'a str) -> Fut<'a, Vec<BatchFileDesc>> {
        within(self.read, self.inner.files(id))
    }
    fn submit<'a>(&'a self, p: &'a SubmitJobParams) -> Fut<'a, BatchJob> {
        within(self.submit, self.inner.submit(p))
    }
    fn cost<'a>(&'a self, p: &'a GetCostParams) -> Fut<'a, (f64, u64)> {
        within(self.read, self.inner.cost(p))
    }
    fn datasets(&self) -> Fut<'_, Vec<String>> {
        within(self.read, self.inner.datasets())
    }
    fn schemas<'a>(&'a self, dataset: &'a str) -> Fut<'a, Vec<Schema>> {
        within(self.read, self.inner.schemas(dataset))
    }
    fn range<'a>(&'a self, dataset: &'a str) -> Fut<'a, DatasetRange> {
        within(self.read, self.inner.range(dataset))
    }
    fn conditions<'a>(
        &'a self,
        p: &'a GetDatasetConditionParams,
    ) -> Fut<'a, Vec<DatasetConditionDetail>> {
        within(self.read, self.inner.conditions(p))
    }
}
