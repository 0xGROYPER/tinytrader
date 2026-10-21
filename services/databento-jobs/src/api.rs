//! The Databento calls this service makes, behind a trait so the purchase
//! paths can be tested against a fake Databento.

use std::future::Future;
use std::pin::Pin;

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
