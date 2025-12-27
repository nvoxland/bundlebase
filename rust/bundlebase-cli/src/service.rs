use crate::state::State;
use arrow::datatypes::SchemaRef;
use arrow::ipc::writer::{DictionaryTracker, IpcDataGenerator, IpcWriteOptions};
use arrow::record_batch::RecordBatch;
use arrow_flight::flight_service_server::FlightService;
use arrow_flight::{
    Action, ActionType, Criteria, Empty, FlightData, FlightDescriptor, FlightInfo,
    HandshakeRequest, HandshakeResponse, PollInfo, PutResult, Result as FlightResult, SchemaResult,
    Ticket,
};
use bytes::Bytes;
use bundlebase::bundle::BundleFacade;
use bundlebase::{Bundle, BundleBuilder};
use futures::stream::Stream;
use std::pin::Pin;
use std::sync::Arc;
use tonic::{Request, Response, Status, Streaming};

pub struct BundlebaseFlightService {
    state: Arc<State>,
}

impl BundlebaseFlightService {
    pub fn new(state: Arc<State>) -> Self {
        Self { state }
    }
}

type BoxedFlightStream = Pin<Box<dyn Stream<Item = Result<FlightData, Status>> + Send>>;
type BoxedPutResultStream = Pin<Box<dyn Stream<Item = Result<PutResult, Status>> + Send>>;
type BoxedResultStream = Pin<Box<dyn Stream<Item = Result<FlightResult, Status>> + Send>>;
type BoxedHandshakeStream = Pin<Box<dyn Stream<Item = Result<HandshakeResponse, Status>> + Send>>;

#[tonic::async_trait]
impl FlightService for BundlebaseFlightService {
    type DoGetStream = BoxedFlightStream;
    type DoPutStream = BoxedPutResultStream;
    type DoExchangeStream = BoxedFlightStream;
    type DoActionStream = BoxedResultStream;
    type ListFlightsStream = Pin<Box<dyn Stream<Item = Result<FlightInfo, Status>> + Send>>;
    type ListActionsStream = Pin<Box<dyn Stream<Item = Result<ActionType, Status>> + Send>>;
    type HandshakeStream = BoxedHandshakeStream;

    async fn do_get(
        &self,
        request: Request<Ticket>,
    ) -> Result<Response<Self::DoGetStream>, Status> {
        let ticket = request.into_inner();

        // Extract SQL query from ticket
        let sql = String::from_utf8(ticket.ticket.to_vec())
            .map_err(|e| Status::invalid_argument(format!("Invalid SQL: {}", e)))?;

        tracing::info!("Executing query: {}", sql);

        // Clone Arc for async execution
        let state = self.state.clone();

        // Execute the query upfront
        let flight_data = execute_query_impl(&state, sql).await?;

        // Convert to a stream
        let stream = futures::stream::iter(flight_data.into_iter().map(Ok));

        // Box the stream into a pinned trait object
        let boxed_stream: Self::DoGetStream = Box::pin(stream);
        Ok(Response::new(boxed_stream))
    }

    async fn do_put(
        &self,
        _request: Request<Streaming<FlightData>>,
    ) -> Result<Response<Self::DoPutStream>, Status> {
        let stream = async_stream::stream! {
            yield Err(Status::unimplemented(
                "do_put is not supported in read-only mode",
            ));
        };
        Ok(Response::new(Box::pin(stream)))
    }

    async fn do_exchange(
        &self,
        _request: Request<Streaming<FlightData>>,
    ) -> Result<Response<Self::DoExchangeStream>, Status> {
        let stream = async_stream::stream! {
            yield Err(Status::unimplemented(
                "do_exchange is not supported in read-only mode",
            ));
        };
        Ok(Response::new(Box::pin(stream)))
    }

    async fn get_flight_info(
        &self,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        Err(Status::unimplemented("get_flight_info is not supported"))
    }

    async fn get_schema(
        &self,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<SchemaResult>, Status> {
        // Get schema by reading from the locked bundle
        // Clone builder to drop lock guard before await
        let builder = {
            let guard = self.state.bundle.read();
            guard.clone()
        };

        let schema = builder
            .schema()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let options = IpcWriteOptions::default();
        let gen = IpcDataGenerator::default();
        let mut dict_tracker = DictionaryTracker::new(false);

        // Encode schema to bytes with dictionary tracker
        let encoded_data = gen.schema_to_bytes_with_dictionary_tracker(
            schema.as_ref(),
            &mut dict_tracker,
            &options,
        );

        let result = SchemaResult {
            schema: encoded_data.ipc_message.into(),
        };
        Ok(Response::new(result))
    }

    async fn do_action(
        &self,
        _request: Request<Action>,
    ) -> Result<Response<Self::DoActionStream>, Status> {
        let stream = async_stream::stream! {
            yield Err(Status::unimplemented("do_action is not supported"));
        };
        Ok(Response::new(Box::pin(stream)))
    }

    async fn list_actions(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<Self::ListActionsStream>, Status> {
        let stream = async_stream::stream! {
            yield Err(Status::unimplemented("list_actions is not supported"));
        };
        Ok(Response::new(Box::pin(stream)))
    }

    async fn list_flights(
        &self,
        _request: Request<Criteria>,
    ) -> Result<Response<Self::ListFlightsStream>, Status> {
        let stream = async_stream::stream! {
            yield Err(Status::unimplemented("list_flights is not supported"));
        };
        Ok(Response::new(Box::pin(stream)))
    }

    async fn handshake(
        &self,
        _request: Request<Streaming<HandshakeRequest>>,
    ) -> Result<Response<Self::HandshakeStream>, Status> {
        let stream = async_stream::stream! {
            yield Err(Status::unimplemented("handshake is not supported"));
        };
        Ok(Response::new(Box::pin(stream)))
    }

    async fn poll_flight_info(
        &self,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<PollInfo>, Status> {
        Err(Status::unimplemented("poll_flight_info is not supported"))
    }
}

/// Execute a query and return FlightData messages
async fn execute_query_impl(state: &Arc<State>, sql: String) -> Result<Vec<FlightData>, Status> {
    // Clone the builder to execute the query (drop lock guard before await)
    let builder = {
        let guard = state.bundle.read();
        guard.clone()
    };

    // Execute the query
    let bundle = builder
        .select(&sql, vec![])
        .await
        .map_err(|e| Status::internal(format!("Failed to execute query: {}", e)))?;

    // Get the dataframe and collect results
    let df = bundle
        .dataframe()
        .await
        .map_err(|e| Status::internal(format!("Failed to get dataframe: {}", e)))?;

    let batches = df
        .as_ref()
        .clone()
        .collect()
        .await
        .map_err(|e| Status::internal(format!("Failed to collect batches: {}", e)))?;

    let mut messages = vec![];

    if let Some(first_batch) = batches.first() {
        let schema = first_batch.schema();

        // Send schema as first message
        let schema_message = create_schema_message(&schema)?;
        messages.push(schema_message);

        // Send each record batch
        for batch in batches {
            let msg = create_batch_message(&batch)?;
            messages.push(msg);
        }
    }

    Ok(messages)
}

/// Create a FlightData message containing the schema
fn create_schema_message(schema: &SchemaRef) -> Result<FlightData, Status> {
    let options = IpcWriteOptions::default();
    let gen = IpcDataGenerator::default();
    let mut dict_tracker = DictionaryTracker::new(false);

    // Encode schema to IPC format with dictionary tracker
    let encoded_data =
        gen.schema_to_bytes_with_dictionary_tracker(schema.as_ref(), &mut dict_tracker, &options);

    Ok(FlightData {
        flight_descriptor: None,
        data_header: Bytes::copy_from_slice(&encoded_data.ipc_message),
        app_metadata: Bytes::new(),
        data_body: Bytes::copy_from_slice(&encoded_data.arrow_data),
    })
}

/// Create a FlightData message containing a record batch
fn create_batch_message(batch: &RecordBatch) -> Result<FlightData, Status> {
    let options = IpcWriteOptions::default();
    let gen = IpcDataGenerator::default();
    let mut dict_tracker = DictionaryTracker::new(false);

    // encoded_batch returns (Vec<EncodedData>, EncodedData)
    let (_dict_batches, encoded_batch) = gen
        .encoded_batch(batch, &mut dict_tracker, &options)
        .map_err(|e| Status::internal(format!("Failed to encode batch: {}", e)))?;

    Ok(FlightData {
        flight_descriptor: None,
        data_header: Bytes::copy_from_slice(&encoded_batch.ipc_message),
        app_metadata: Bytes::new(),
        data_body: Bytes::copy_from_slice(&encoded_batch.arrow_data),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_flight_service_with_memory_bundle() {
        // Create a bundle and wrap it in a Flight service
        let builder = BundleBuilder::create("memory:///flight_test")
            .await
            .expect("Failed to create bundle");

        let service = BundlebaseFlightService::new(Arc::new(State::new(builder)));

        // Verify service can be instantiated and has an empty schema
        let result = service
            .get_schema(tonic::Request::new(
                arrow_flight::FlightDescriptor::default(),
            ))
            .await;

        assert!(result.is_ok(), "Failed to get schema from flight service");
    }

    #[tokio::test]
    async fn test_server_startup_scenario() {
        // Simulates the server startup scenario with --create flag
        // when a new bundle needs to be created and wrapped in Flight service
        let url = format!(
            "memory:///server_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );

        // Create a new bundle (what happens with --create flag)
        let builder = BundleBuilder::create(&url)
            .await
            .expect("Failed to create bundle");

        // Verify the bundle is valid and can be used in Flight service immediately
        let service = BundlebaseFlightService::new(Arc::new(State::new(builder)));

        // Create Flight service successfully
        let result = service
            .get_schema(tonic::Request::new(
                arrow_flight::FlightDescriptor::default(),
            ))
            .await;
        assert!(
            result.is_ok(),
            "Flight service should work with memory:// URLs"
        );
    }
}
