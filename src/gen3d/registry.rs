//! Name → Entity bidirectional registry.
//!
//! All Gen entities are referenced by human-readable names rather than
//! opaque Bevy Entity IDs. This registry maintains the mapping.

use bevy::prelude::*;
use std::collections::HashMap;

/// Marker component attached to every Gen-managed entity.
#[derive(Component)]
pub struct GenEntity {
    /// What kind of entity this is (for scene_info reporting).
    pub entity_type: GenEntityType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenEntityType {
    Primitive,
    Light,
    Camera,
    Mesh,
    Group,
}

impl GenEntityType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Primitive => "primitive",
            Self::Light => "light",
            Self::Camera => "camera",
            Self::Mesh => "mesh",
            Self::Group => "group",
        }
    }
}

/// Bevy resource that maps names ↔ entities.
#[derive(Resource, Default)]
pub struct NameRegistry {
    name_to_entity: HashMap<String, Entity>,
    entity_to_name: HashMap<Entity, String>,
}

impl NameRegistry {
    pub fn insert(&mut self, name: String, entity: Entity) {
        self.name_to_entity.insert(name.clone(), entity);
        self.entity_to_name.insert(entity, name);
    }

    pub fn get_entity(&self, name: &str) -> Option<Entity> {
        self.name_to_entity.get(name).copied()
    }

    pub fn get_name(&self, entity: Entity) -> Option<&str> {
        self.entity_to_name.get(&entity).map(|s| s.as_str())
    }

    pub fn remove_by_name(&mut self, name: &str) -> Option<Entity> {
        if let Some(entity) = self.name_to_entity.remove(name) {
            self.entity_to_name.remove(&entity);
            Some(entity)
        } else {
            None
        }
    }

    pub fn remove_by_entity(&mut self, entity: Entity) -> Option<String> {
        if let Some(name) = self.entity_to_name.remove(&entity) {
            self.name_to_entity.remove(&name);
            Some(name)
        } else {
            None
        }
    }

    pub fn contains_name(&self, name: &str) -> bool {
        self.name_to_entity.contains_key(name)
    }

    pub fn all_names(&self) -> impl Iterator<Item = (&str, Entity)> {
        self.name_to_entity.iter().map(|(k, v)| (k.as_str(), *v))
    }

    pub fn len(&self) -> usize {
        self.name_to_entity.len()
    }

    pub fn is_empty(&self) -> bool {
        self.name_to_entity.is_empty()
    }
}
