//! Query execution and streaming for Flight SQL.

use crate::sql_executor::{self, SqlResult};
use crate::state::BundleState;
use arrow_flight::encode::FlightDataEncoderBuilder;
use arrow_flight::error::FlightError;
use arrow_flight::FlightData;
use futures::{Stream, StreamExt, TryStreamExt};
use std::pin::Pin;
use std::sync::Arc;
use tonic::{Response, Status};

/// Stream type for FlightData responses.
pub type DoGetStream = Pin<Box<dyn Stream<Item = Result<FlightData, Status>> + Send>>;

/// Execute a query and return a streaming FlightData response.
pub async fn execute_query_streaming(
    state: &Arc<BundleState>,
    sql: String,
) -> Result<Response<DoGetStream>, Status> {
    match sql_executor::execute_sql(state, &sql).await {
        Ok(SqlResult::Stream(record_stream)) => {
            // Get schema from the stream
            let schema = record_stream.schema();

            // Convert the SendableRecordBatchStream to a stream of Results
            let batch_stream = record_stream
                .map(|result| result.map_err(|e| FlightError::ExternalError(Box::new(e))));

            // Use FlightDataEncoder to properly encode the stream
            let flight_stream = FlightDataEncoderBuilder::new()
                .with_schema(schema)
                .build(batch_stream)
                .map_err(|e| Status::from_error(Box::new(e)));

            Ok(Response::new(Box::pin(flight_stream)))
        }
        Ok(SqlResult::Output(output)) => {
            // BundleCommand - convert to record batch and stream
            let batch = output
                .to_record_batch()
                .map_err(|e| Status::internal(format!("Failed to convert output: {}", e)))?;

            let schema = batch.schema();

            let flight_stream = FlightDataEncoderBuilder::new()
                .with_schema(schema)
                .build(futures::stream::once(async { Ok(batch) }))
                .map_err(|e| Status::from_error(Box::new(e)));

            Ok(Response::new(Box::pin(flight_stream)))
        }
        Err(e) => Err(Status::internal(format!("Failed to execute query: {}", e))),
    }
}
