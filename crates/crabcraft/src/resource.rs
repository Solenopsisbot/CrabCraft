//! Asynchronous transactional CPU resource preparation.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};

use crab_assets::{AssetError, ResourceSet};
use crab_registry::RegistrySet;

struct Request {
    generation: u64,
    archive: PathBuf,
}

pub struct PreparedResources {
    pub generation: u64,
    pub archive: PathBuf,
    pub resources: Result<ResourceSet, AssetError>,
}

/// One-worker bounded resource builder. ZIP parsing and image/model decoding
/// never run on the winit thread; only the final GPU commit does.
pub struct ResourceManager {
    request_tx: SyncSender<Request>,
    result_rx: Receiver<PreparedResources>,
    next_generation: u64,
}

impl ResourceManager {
    pub fn new(registries: RegistrySet, entity_models: Option<PathBuf>) -> Self {
        let entity_models = entity_models.map(normalize_models_dir);
        let (request_tx, request_rx) = mpsc::sync_channel::<Request>(1);
        let (result_tx, result_rx) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("crab-resource-builder".to_owned())
            .spawn(move || {
                while let Ok(request) = request_rx.recv() {
                    let resources = crab_assets::load_resource_set(
                        &request.archive,
                        registries,
                        entity_models.as_deref(),
                    );
                    if result_tx
                        .send(PreparedResources {
                            generation: request.generation,
                            archive: request.archive,
                            resources,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .expect("spawn resource builder");
        Self {
            request_tx,
            result_rx,
            next_generation: 1,
        }
    }

    pub fn request(&mut self, archive: PathBuf) -> Result<u64, PathBuf> {
        let generation = self.next_generation;
        match self.request_tx.try_send(Request {
            generation,
            archive,
        }) {
            Ok(()) => {
                self.next_generation = self.next_generation.saturating_add(1);
                Ok(generation)
            }
            Err(TrySendError::Full(request) | TrySendError::Disconnected(request)) => {
                Err(request.archive)
            }
        }
    }

    pub fn try_recv(&self) -> Option<PreparedResources> {
        self.result_rx.try_recv().ok()
    }
}

fn normalize_models_dir(mut models: PathBuf) -> PathBuf {
    if models.join("cow.geo.json").exists() {
        return models;
    }
    for subdirectory in ["resource_pack/models/entity", "models/entity", "entity"] {
        let candidate = models.join(subdirectory);
        if candidate.join("cow.geo.json").exists() {
            models = candidate;
            break;
        }
    }
    models
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn failed_preparation_is_reported_without_replacing_resources() {
        let mut manager = ResourceManager::new(RegistrySet::for_protocol(763), None);
        let generation = manager
            .request(PathBuf::from("/crabcraft-test/missing-resource-pack.zip"))
            .unwrap();
        let prepared = manager
            .result_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("resource worker should return its load error");
        assert_eq!(prepared.generation, generation);
        assert!(prepared.resources.is_err());
    }
}
