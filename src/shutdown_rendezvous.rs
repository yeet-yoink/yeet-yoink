use tokio::sync::mpsc;
use tracing::{debug, error, info};

/// The [`ShutdownRendezvous`] handler ensures that all relevant background threads
/// have exited correctly. This happens mainly by ensuring that the event senders are
/// dropped, although the individual services are intended to send a relevant event
/// for diagnostics purposes.
pub struct ShutdownRendezvous {
    tx: Option<mpsc::Sender<ShutdownRendezvousEvent>>,
    rx: mpsc::Receiver<ShutdownRendezvousEvent>,
}

impl ShutdownRendezvous {
    /// Creates a new instance of `ShutdownRendezvous`.
    ///
    /// # Returns
    ///
    /// * A new `ShutdownRendezvous` instance.
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel(16);
        Self { tx: Some(tx), rx }
    }

    /// Retrieves a clone of the `mpsc::Sender` used for triggering shutdown rendezvous events.
    ///
    /// # Returns
    ///
    /// The cloned `mpsc::Sender` used for sending `ShutdownRendezvousEvent` messages.
    ///
    /// # Panics
    ///
    /// If the rendezvous channel has been dropped, this method will panic with the message
    /// "Rendezvous channel was dropped".
    ///
    /// # Example
    ///
    /// ```
    /// use std::sync::mpsc;
    ///
    /// // Assume the `tx` field is of type `mpsc::Sender<ShutdownRendezvousEvent>`.
    /// let sender = my_struct.get_trigger();
    /// ```
    pub fn get_trigger(&self) -> mpsc::Sender<ShutdownRendezvousEvent> {
        self.tx.clone().expect("Rendezvous channel was dropped")
    }

    /// Performs the rendezvous; waits until all senders are dropped, thereby implying the
    /// relevant workload has finished.
    pub async fn rendezvous(mut self) {
        drop(self.tx.take());

        // Wait for all services to shut down.
        while let Some(event) = self.rx.recv().await {
            match event {
                ShutdownRendezvousEvent::BackendRegistry => {
                    debug!("Backend registry worker thread finished")
                }
                ShutdownRendezvousEvent::Backbone => debug!("Backbone worker thread finished"),
            }
        }
        info!("Shutdown rendezvous completed");
    }
}

/// This enum represents the different events that can trigger a shutdown rendezvous.
///
/// # Variants
///
/// - `BackendRegistry`: Represents the event related to the backend registry.
/// - `Backbone`: Represents the event related to the backbone.
pub enum ShutdownRendezvousEvent {
    BackendRegistry,
    Backbone,
}

impl Drop for ShutdownRendezvous {
    fn drop(&mut self) {
        #[cfg(debug_assertions)]
        if self.tx.is_some() {
            error!("Implementation error: Rendezvous method not invoked")
        }
    }
}
