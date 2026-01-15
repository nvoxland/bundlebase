use bundlebase;
use bundlebase::bundle::BundleFacade;
use bundlebase::test_utils::{assert_vec_regexp, random_memory_url, test_datafile};
use bundlebase::BundlebaseError;
use bundlebase::Operation;

mod common;

#[tokio::test]
async fn test_adding_blocks() -> Result<(), BundlebaseError> {
    let data_dir = random_memory_url();
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.as_str(), None).await?;

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
