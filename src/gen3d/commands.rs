//! GenCommand / GenResponse protocol between agent and Bevy.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Commands (agent → Bevy)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum GenCommand {
    // Tier 1: Perceive
    SceneInfo,
    Screenshot {
        width: u32,
        height: u32,
        wait_frames: u32,
    },
    EntityInfo {
        name: String,
    },

    // Tier 2: Mutate
    SpawnPrimitive(SpawnPrimitiveCmd),
    ModifyEntity(ModifyEntityCmd),
    DeleteEntity {
        name: String,
    },
    SetCamera(CameraCmd),
    SetLight(SetLightCmd),
    SetEnvironment(EnvironmentCmd),

    // Tier 3: Advanced
    SpawnMesh(RawMeshCmd),

    // Tier 4: Export
    ExportScreenshot {
        path: String,
        width: u32,
        height: u32,
    },
}

// ---------------------------------------------------------------------------
// Command data structures
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnPrimitiveCmd {
    pub name: String,
    pub shape: PrimitiveShape,
    #[serde(default)]
    pub dimensions: HashMap<String, f32>,
    #[serde(default = "default_position")]
    pub position: [f32; 3],
    #[serde(default)]
    pub rotation_degrees: [f32; 3],
    #[serde(default = "default_scale")]
    pub scale: [f32; 3],
    #[serde(default = "default_color")]
    pub color: [f32; 4],
    #[serde(default)]
    pub metallic: f32,
    #[serde(default = "default_roughness")]
    pub roughness: f32,
    #[serde(default)]
    pub emissive: [f32; 4],
    pub parent: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum PrimitiveShape {
    Cuboid,
    Sphere,
    Cylinder,
    Cone,
    Capsule,
    Torus,
    Plane,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModifyEntityCmd {
    pub name: String,
    pub position: Option<[f32; 3]>,
    pub rotation_degrees: Option<[f32; 3]>,
    pub scale: Option<[f32; 3]>,
    pub color: Option<[f32; 4]>,
    pub metallic: Option<f32>,
    pub roughness: Option<f32>,
    pub emissive: Option<[f32; 4]>,
    pub visible: Option<bool>,
    pub parent: Option<Option<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraCmd {
    #[serde(default = "default_camera_pos")]
    pub position: [f32; 3],
    #[serde(default)]
    pub look_at: [f32; 3],
    #[serde(default = "default_fov")]
    pub fov_degrees: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetLightCmd {
    pub name: String,
    #[serde(default = "default_light_type")]
    pub light_type: LightType,
    #[serde(default = "default_white")]
    pub color: [f32; 4],
    #[serde(default = "default_intensity")]
    pub intensity: f32,
    pub position: Option<[f32; 3]>,
    pub direction: Option<[f32; 3]>,
    #[serde(default = "default_true")]
    pub shadows: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LightType {
    Directional,
    Point,
    Spot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentCmd {
    pub background_color: Option<[f32; 4]>,
    pub ambient_light: Option<f32>,
    pub ambient_color: Option<[f32; 4]>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawMeshCmd {
    pub name: String,
    pub vertices: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    pub normals: Option<Vec<[f32; 3]>>,
    pub uvs: Option<Vec<[f32; 2]>>,
    #[serde(default = "default_color")]
    pub color: [f32; 4],
    #[serde(default)]
    pub metallic: f32,
    #[serde(default = "default_roughness")]
    pub roughness: f32,
    #[serde(default)]
    pub position: [f32; 3],
}

// ---------------------------------------------------------------------------
// Responses (Bevy → agent)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum GenResponse {
    SceneInfo(SceneInfoData),
    Screenshot { image_path: String },
    EntityInfo(EntityInfoData),
    Spawned { name: String, entity_id: u64 },
    Modified { name: String },
    Deleted { name: String },
    CameraSet,
    LightSet { name: String },
    EnvironmentSet,
    Exported { path: String },
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneInfoData {
    pub entity_count: usize,
    pub entities: Vec<EntitySummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntitySummary {
    pub name: String,
    pub entity_type: String,
    pub position: [f32; 3],
    pub scale: [f32; 3],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<[f32; 4]>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityInfoData {
    pub name: String,
    pub entity_id: u64,
    pub entity_type: String,
    pub position: [f32; 3],
    pub rotation_degrees: [f32; 3],
    pub scale: [f32; 3],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<[f32; 4]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metallic: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roughness: Option<f32>,
    pub visible: bool,
    pub children: Vec<String>,
    pub parent: Option<String>,
}

// ---------------------------------------------------------------------------
// Default helpers
// ---------------------------------------------------------------------------

fn default_position() -> [f32; 3] {
    [0.0, 0.0, 0.0]
}
fn default_scale() -> [f32; 3] {
    [1.0, 1.0, 1.0]
}
fn default_color() -> [f32; 4] {
    [0.8, 0.8, 0.8, 1.0]
}
fn default_roughness() -> f32 {
    0.5
}
fn default_camera_pos() -> [f32; 3] {
    [5.0, 5.0, 5.0]
}
fn default_fov() -> f32 {
    45.0
}
fn default_light_type() -> LightType {
    LightType::Directional
}
fn default_white() -> [f32; 4] {
    [1.0, 1.0, 1.0, 1.0]
}
fn default_intensity() -> f32 {
    1000.0
}
fn default_true() -> bool {
    true
}
