// Profile bundlebase query execution on a given bundle + SQL.
// Usage:
//   cargo build --release --example profile_query -p bundlebase \
//       --config 'profile.release.strip="none"' --config 'profile.release.debug=true'
//   samply record target/maturin/release/examples/profile_query <bundle_path> "<sql>" <iters>
use bundlebase::bundle::BundleFacade;
use bundlebase::BundlebaseError;
use futures::StreamExt;
use std::time::Instant;

#[tokio::main]
async fn main() -> Result<(), BundlebaseError> {
    bundlebase_catalog::init();
    let path = std::env::args().nth(1).ok_or_else(|| {
        BundlebaseError::from(
            "usage: profile_query <bundle_path> [sql] [iters]".to_string(),
        )
    })?;
    let sql = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "SELECT DISTINCT type FROM bundle".to_string());
    let iters: u32 = std::env::args()
        .nth(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);

    println!("bundle: {}", path);
    println!("sql:    {}", sql);
    println!("iters:  {}", iters);

    // Open once; queries share the same Bundle.
    let t_open = Instant::now();
    let bundle = bundlebase::Bundle::open(&path, None).await?;
    println!("open:   {:?}", t_open.elapsed());

    // Warmup query (not timed).
    {
        let mut stream = bundle.query(&sql, vec![], None).await?;
        while let Some(b) = stream.next().await {
            let _ = b?;
        }
    }

    let t_all = Instant::now();
    let mut query_sum = std::time::Duration::ZERO;
    let mut rows: usize = 0;
    for _ in 0..iters {
        let t1 = Instant::now();
        let mut stream = bundle.query(&sql, vec![], None).await?;
        rows = 0;
        while let Some(b) = stream.next().await {
            rows += b?.num_rows();
        }
        query_sum += t1.elapsed();
    }
    let n = iters;
    println!(
        "total={:?} iters={} avg_query={:?} rows={}",
        t_all.elapsed(),
        iters,
        query_sum / n,
        rows
    );
    Ok(())
}
