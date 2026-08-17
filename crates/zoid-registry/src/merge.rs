//! Merge user registry over shipped (filled in Task 5).
use zoid_model::{Registry, RegistryPatch};

pub fn merge(shipped: Registry, _user: RegistryPatch) -> Registry {
    shipped
}