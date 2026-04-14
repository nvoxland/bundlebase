use bundlebase;
use bundlebase::bundle::BundleFacade;
use bundlebase::AnyOperation;
use bundlebase::test_utils::{assert_vec_regexp, random_memory_url, test_datafile};
use bundlebase::op_field;
use bundlebase_common::BundlebaseError;
use bundlebase::Operation;
use bundlebase_command::BundleBuilderExt;

mod common;

fn init() {
    common::init_catalog();
}


#[tokio::test]
async fn test_adding_blocks() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_url();
    let bundle = bundlebase::BundleBuilder::create(data_dir.as_str(), None).await?;

    bundle.attach(test_datafile("customers-0-100.csv"), None).await?;

    assert_vec_regexp(
        vec![
            "ATTACH: memory:///test_data/customers-0-100.csv",
        ],
        bundle
            .status()
            .operations()
            .iter()
            .map(|x| x.describe())
            .collect::<Vec<_>>(),
    );

    assert_eq!(100, bundle.num_rows().await?);
    assert_eq!(12, bundle.schema().await?.fields().len());

    bundle
        .attach(test_datafile("customers-101-150.csv"), None)
        .await?;

    assert_vec_regexp(
        vec![
            "ATTACH: memory:///test_data/customers-0-100.csv",
            "ATTACH: memory:///test_data/customers-101-150.csv",
        ],
        bundle
            .status()
            .operations()
            .iter()
            .map(|x| x.describe())
            .collect::<Vec<String>>(),
    );

    assert_eq!(150, bundle.num_rows().await?);
    assert_eq!(12, bundle.schema().await?.fields().len());

    Ok(())
}

#[tokio::test]
async fn test_column_id_reuse_across_blocks() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_url();
    let bundle = bundlebase::BundleBuilder::create(data_dir.as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;
    bundle
        .attach(test_datafile("customers-101-150.csv"), None)
        .await?;

    let ops = bundle.status().operations();

    let ids_block1 = op_field!(ops[0], AnyOperation::AttachBlock, column_ids);
    let ids_block2 = op_field!(ops[1], AnyOperation::AttachBlock, column_ids);

    assert_eq!(
        ids_block1.len(),
        ids_block2.len(),
        "Both blocks should have the same number of column IDs"
    );
    assert_eq!(
        ids_block1, ids_block2,
        "Column IDs should be identical across blocks with the same schema"
    );

    Ok(())
}
