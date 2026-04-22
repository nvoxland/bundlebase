// Quick Rust binary to profile Bundle::open.
// Usage: cargo build --release --example profile_open -p bundlebase
//        samply record target/release/examples/profile_open <bundle_path> [iters]
use bundlebase::BundlebaseError;
use std::time::Instant;

#[tokio::main]
async fn main() -> Result<(), BundlebaseError> {
    bundlebase_catalog::init();
    let path = std::env::args().nth(1).ok_or_else(|| {
        BundlebaseError::from(
            "usage: profile_open <bundle_path> [iters]".to_string(),
        )
    })?;
    let iters: u32 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);

    // Warmup: one untimed open
    let _ = bundlebase::Bundle::open(&path, None).await?;

    let t_all = Instant::now();
    let mut open_sum = std::time::Duration::ZERO;
    let mut drop_sum = std::time::Duration::ZERO;
    for _ in 0..iters {
        let t1 = Instant::now();
        let b = bundlebase::Bundle::open(&path, None).await?;
        open_sum += t1.elapsed();
        let t2 = Instant::now();
        drop(b);
        drop_sum += t2.elapsed();
    }
    let total = t_all.elapsed();
    let n = iters as u32;
    println!(
        "total={:?} iters={} avg_open={:?} avg_drop={:?} avg_total={:?}",
        total,
        iters,
        open_sum / n,
        drop_sum / n,
        total / n
    );
    Ok(())
}
