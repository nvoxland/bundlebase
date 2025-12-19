use bundlebase::BundleBuilder;
use parking_lot::RwLock;

pub struct State {
    pub(crate) bundle: RwLock<BundleBuilder>,
}

impl State {
    pub(crate) fn new(bundle: BundleBuilder) -> Self {
        Self {
            bundle: RwLock::new(bundle),
        }
    }
}
