//! Schema command - displays the bundle's table schema.

use super::{ReplCommandResult, ReplCommand, ReplCommandDef};
use bundlebase::bundle::CommandResponse;
use bundlebase::BundleFacade;
use futures::future::BoxFuture;
use std::sync::Arc;

/// Command metadata
pub const DEF: ReplCommandDef = ReplCommandDef {
    name: "schema",
    aliases: &[],
    description: "Show table schema",
    usage: "/schema",
    create,
    execute,
};

fn create(_args: &str) -> Result<ReplCommand, String> {
    Ok(ReplCommand::Schema)
}

fn execute(_cmd: &ReplCommand, bundle: &Arc<dyn BundleFacade>) -> BoxFuture<'static, ReplCommandResult> {
    let bundle = bundle.clone();
    Box::pin(async move {
        let schema = bundle.schema().await?;
        let response: Box<dyn CommandResponse> = if schema.fields().is_empty() {
            Box::new("No columns in schema".to_string())
        } else {
            Box::new(schema)
        };
        let (stream, shape) = super::response_to_stream(response)?;
        Ok(Some((stream, shape)))
    })
}

#[cfg(test)]
mod tests {
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow_schema::SchemaRef;
    use bundlebase::bundle::CommandResponse;
    use futures::StreamExt;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_schema_result() {
        let schema: SchemaRef = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, true),
        ]));
        let response: Box<dyn CommandResponse> = Box::new(schema);
        let mut stream = response.into_stream().unwrap();
        let batch = stream.next().await.unwrap().unwrap();
        assert_eq!(batch.num_rows(), 2);
        assert_eq!(batch.num_columns(), 3);
    }
}
