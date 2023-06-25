//! Contains HTTP metrics related code, notably [`HttpMetrics`].

use lazy_static::lazy_static;
use prometheus_client::encoding::LabelValueEncoder;
use prometheus_client::encoding::{EncodeLabelSet, EncodeLabelValue};
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::registry::{Registry, Unit};
use std::fmt::{Display, Formatter, Write};

lazy_static! {
    static ref TRANSFER_SIZES: Family<Labels, Counter> = Family::default();
    static ref TRANSFER_COUNT: Family<Labels, Counter> = Family::default();
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct Labels {
    method: TransferMethod,
}

/// The HTTP method to track.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub enum TransferMethod {
    Store,
    Fetch,
}

impl EncodeLabelValue for TransferMethod {
    fn encode(&self, encoder: &mut LabelValueEncoder) -> Result<(), std::fmt::Error> {
        encoder.write_str(self.to_string().as_str())
    }
}

impl Display for TransferMethod {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            TransferMethod::Store => write!(f, "store"),
            TransferMethod::Fetch => write!(f, "fetch"),
        }
    }
}

/// Register the `http_requests` metric family with the registry.
pub(crate) fn register_transfer_metrics(registry: &mut Registry) {
    registry.register_with_unit(
        "transfer_size",
        "Number of bytes received or sent",
        Unit::Bytes,
        TRANSFER_SIZES.clone(),
    );

    registry.register(
        "transfer",
        "Number of transfers initiated",
        TRANSFER_COUNT.clone(),
    );
}

/// HTTP call metrics. Can be cheaply cloned.
/// Used by [`HttpCallMetrics`](crate::services::HttpCallMetrics).
#[derive(Default)]
pub struct TransferMetrics;

impl TransferMetrics {
    /// Tracks one call to the specified transfer method.
    pub fn track_transfer<M: Into<TransferMethod>>(transfer: M) {
        TRANSFER_COUNT
            .get_or_create(&Labels {
                method: transfer.into(),
            })
            .inc();
    }

    /// Tracks an increase in transfer payload size.
    pub fn track_bytes_transferred<M: Into<TransferMethod>>(transfer: M, bytes: usize) {
        TRANSFER_SIZES
            .get_or_create(&Labels {
                method: transfer.into(),
            })
            .inc_by(bytes as _);
    }
}
