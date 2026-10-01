use arrow_schema::ArrowError;
use datafusion::{arrow::array::RecordBatch, prelude::DataFrame};
use datafusion_common::DataFusionError;
use futures::StreamExt;

/// Extension trait that normalizes streaming Arrow output across DataFusion types.
#[async_trait::async_trait]
pub trait ArrowSendable {
    async fn arrow_sendable(
        self,
    ) -> Result<
        std::pin::Pin<
            Box<dyn futures::Stream<Item = Result<RecordBatch, ArrowError>> + Send + 'static>,
        >,
        DataFusionError,
    >;
}

#[async_trait::async_trait]
impl ArrowSendable for DataFrame {
    async fn arrow_sendable(
        self,
    ) -> Result<
        std::pin::Pin<
            Box<dyn futures::Stream<Item = Result<RecordBatch, ArrowError>> + Send + 'static>,
        >,
        DataFusionError,
    > {
        Ok(self
            .execute_stream()
            .await?
            .map(|res| res.map_err(|e| ArrowError::ExternalError(Box::new(e))))
            .boxed())
    }
}
