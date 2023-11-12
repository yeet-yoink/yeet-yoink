use tokio::sync::mpsc;
use tracing::{debug, error, info};

pub struct ShutdownRendezvous {
    tx: Option<mpsc::Sender<ShutdownRendezvousEvent>>,
    rx: mpsc::Receiver<ShutdownRendezvousEvent>,
}

impl ShutdownRendezvous {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel(16);
        Self { tx: Some(tx), rx }
    }

    pub fn get_trigger(&self) -> mpsc::Sender<ShutdownRendezvousEvent> {
        self.tx.clone().expect("Rendezvous channel was dropped")
    }

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
