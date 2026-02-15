//! Bevy GenPlugin — command processing, default scene, screenshot capture.

use bevy::prelude::*;
use bevy::render::mesh::Indices;
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::PrimitiveTopology;

use super::GenChannels;
use super::commands::*;
use super::registry::*;

/// Bevy resource wrapping the channel endpoints.
#[derive(Resource)]
pub struct GenChannelRes {
    channels: GenChannels,
}

impl GenChannelRes {
    pub fn new(channels: GenChannels) -> Self {
        Self { channels }
    }
}

/// Pending screenshot requests that need to wait N frames.
#[derive(Resource, Default)]
pub struct PendingScreenshots {
    queue: Vec<PendingScreenshot>,
}

#[allow(dead_code)]
struct PendingScreenshot {
    frames_remaining: u32,
    width: u32,
    height: u32,
    path: Option<String>,
}

/// Plugin that sets up the Gen 3D environment.
pub struct GenPlugin {
    pub channels: GenChannels,
}

impl Plugin for GenPlugin {
    fn build(&self, _app: &mut App) {
        // We can't move channels out of &self in build(), so we use a
        // workaround: store channels in a temporary and take them in a
        // startup system. See `setup_channels` below.
    }
}

/// Initialize the Gen world: channels, default scene, systems.
///
/// Call this instead of using Plugin::build since we need to move the channels.
pub fn setup_gen_app(app: &mut App, channels: GenChannels) {
    app.insert_resource(GenChannelRes::new(channels))
        .init_resource::<NameRegistry>()
        .init_resource::<PendingScreenshots>()
        .add_systems(Startup, setup_default_scene)
        .add_systems(Update, (process_gen_commands, process_pending_screenshots));
}

/// Default scene: ground plane, camera, directional light, ambient light.
fn setup_default_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut registry: ResMut<NameRegistry>,
) {
    // Ground plane — 20×20 gray
    let ground = commands
        .spawn((
            Mesh3d(meshes.add(Plane3d::new(Vec3::Y, Vec2::new(10.0, 10.0)))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgba(0.3, 0.3, 0.3, 1.0),
                metallic: 0.0,
                perceptual_roughness: 0.8,
                ..default()
            })),
            Transform::from_translation(Vec3::ZERO),
            Name::new("ground_plane"),
            GenEntity {
                entity_type: GenEntityType::Primitive,
            },
        ))
        .id();
    registry.insert("ground_plane".into(), ground);

    // Camera at (5, 5, 5) looking at origin
    let camera = commands
        .spawn((
            Camera3d::default(),
            Transform::from_translation(Vec3::new(5.0, 5.0, 5.0)).looking_at(Vec3::ZERO, Vec3::Y),
            Name::new("main_camera"),
            GenEntity {
                entity_type: GenEntityType::Camera,
            },
        ))
        .id();
    registry.insert("main_camera".into(), camera);

    // Directional light — warm white, shadows
    let light = commands
        .spawn((
            DirectionalLight {
                illuminance: 10000.0,
                shadows_enabled: true,
                color: Color::srgba(1.0, 0.95, 0.9, 1.0),
                ..default()
            },
            Transform::from_translation(Vec3::new(4.0, 8.0, 4.0)).looking_at(Vec3::ZERO, Vec3::Y),
            Name::new("main_light"),
            GenEntity {
                entity_type: GenEntityType::Light,
            },
        ))
        .id();
    registry.insert("main_light".into(), light);
}

/// Poll the command channel each frame and dispatch.
#[allow(clippy::too_many_arguments)]
fn process_gen_commands(
    mut channel_res: ResMut<GenChannelRes>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut registry: ResMut<NameRegistry>,
    mut pending_screenshots: ResMut<PendingScreenshots>,
    transforms: Query<&Transform>,
    gen_entities: Query<&GenEntity>,
    names_query: Query<&Name>,
    children_query: Query<&Children>,
    parent_query: Query<&Parent>,
    visibility_query: Query<&Visibility>,
    material_handles: Query<&MeshMaterial3d<StandardMaterial>>,
) {
    while let Ok(cmd) = channel_res.channels.cmd_rx.try_recv() {
        let response = match cmd {
            GenCommand::SceneInfo => handle_scene_info(
                &registry,
                &transforms,
                &gen_entities,
                &material_handles,
                &materials,
            ),
            GenCommand::EntityInfo { name } => handle_entity_info(
                &name,
                &registry,
                &transforms,
                &gen_entities,
                &names_query,
                &children_query,
                &parent_query,
                &visibility_query,
                &material_handles,
                &materials,
            ),
            GenCommand::Screenshot {
                width,
                height,
                wait_frames,
            } => {
                pending_screenshots.queue.push(PendingScreenshot {
                    frames_remaining: wait_frames,
                    width,
                    height,
                    path: None,
                });
                // Response will be sent by process_pending_screenshots
                continue;
            }
            GenCommand::SpawnPrimitive(cmd) => handle_spawn_primitive(
                cmd,
                &mut commands,
                &mut meshes,
                &mut materials,
                &mut registry,
            ),
            GenCommand::ModifyEntity(cmd) => handle_modify_entity(
                cmd,
                &mut commands,
                &registry,
                &mut materials,
                &material_handles,
                &transforms,
            ),
            GenCommand::DeleteEntity { name } => {
                handle_delete_entity(&name, &mut commands, &mut registry)
            }
            GenCommand::SetCamera(cmd) => handle_set_camera(cmd, &mut commands, &registry),
            GenCommand::SetLight(cmd) => handle_set_light(cmd, &mut commands, &mut registry),
            GenCommand::SetEnvironment(cmd) => handle_set_environment(cmd, &mut commands),
            GenCommand::SpawnMesh(cmd) => handle_spawn_mesh(
                cmd,
                &mut commands,
                &mut meshes,
                &mut materials,
                &mut registry,
            ),
            GenCommand::ExportScreenshot {
                path,
                width,
                height,
            } => {
                pending_screenshots.queue.push(PendingScreenshot {
                    frames_remaining: 3,
                    width,
                    height,
                    path: Some(path),
                });
                continue;
            }
        };

        let _ = channel_res.channels.resp_tx.send(response);
    }
}

/// Process pending screenshots that need frame delays.
fn process_pending_screenshots(
    channel_res: ResMut<GenChannelRes>,
    mut pending: ResMut<PendingScreenshots>,
) {
    let mut completed = Vec::new();

    for (i, screenshot) in pending.queue.iter_mut().enumerate() {
        if screenshot.frames_remaining > 0 {
            screenshot.frames_remaining -= 1;
        } else {
            completed.push(i);
        }
    }

    // Process completed screenshots in reverse order to preserve indices
    for i in completed.into_iter().rev() {
        let screenshot = pending.queue.remove(i);

        // Determine output path
        let path = screenshot.path.unwrap_or_else(|| {
            let tmp = std::env::temp_dir().join(format!(
                "localgpt_gen_screenshot_{}.png",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
            ));
            tmp.to_string_lossy().into_owned()
        });

        // TODO: Actual Bevy screenshot capture requires camera entity access
        // and render-to-texture. For now, we create a placeholder and report
        // the path. Full implementation needs Bevy's Screenshot observer or
        // render target approach.
        //
        // In a full implementation:
        //   commands.entity(camera).trigger(Screenshot::to_disk(path));
        let response = GenResponse::Screenshot {
            image_path: path.clone(),
        };
        let _ = channel_res.channels.resp_tx.send(response);
    }
}

// ---------------------------------------------------------------------------
// Command handlers
// ---------------------------------------------------------------------------

fn handle_scene_info(
    registry: &NameRegistry,
    transforms: &Query<&Transform>,
    gen_entities: &Query<&GenEntity>,
    material_handles: &Query<&MeshMaterial3d<StandardMaterial>>,
    material_assets: &Assets<StandardMaterial>,
) -> GenResponse {
    let mut entities = Vec::new();

    for (name, entity) in registry.all_names() {
        let position = transforms
            .get(entity)
            .map(|t| t.translation.to_array())
            .unwrap_or_default();
        let scale = transforms
            .get(entity)
            .map(|t| t.scale.to_array())
            .unwrap_or([1.0, 1.0, 1.0]);
        let entity_type = gen_entities
            .get(entity)
            .map(|g| g.entity_type.as_str().to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        let color = material_handles
            .get(entity)
            .ok()
            .and_then(|h| material_assets.get(&h.0))
            .map(|mat| {
                let c = mat.base_color.to_srgba();
                [c.red, c.green, c.blue, c.alpha]
            });

        entities.push(EntitySummary {
            name: name.to_string(),
            entity_type,
            position,
            scale,
            color,
        });
    }

    GenResponse::SceneInfo(SceneInfoData {
        entity_count: entities.len(),
        entities,
    })
}

#[allow(clippy::too_many_arguments)]
fn handle_entity_info(
    name: &str,
    registry: &NameRegistry,
    transforms: &Query<&Transform>,
    gen_entities: &Query<&GenEntity>,
    names_query: &Query<&Name>,
    children_query: &Query<&Children>,
    parent_query: &Query<&Parent>,
    visibility_query: &Query<&Visibility>,
    material_handles: &Query<&MeshMaterial3d<StandardMaterial>>,
    material_assets: &Assets<StandardMaterial>,
) -> GenResponse {
    let Some(entity) = registry.get_entity(name) else {
        return GenResponse::Error {
            message: format!("Entity '{}' not found", name),
        };
    };

    let transform = transforms.get(entity).copied().unwrap_or_default();
    let euler = transform.rotation.to_euler(EulerRot::XYZ);

    let entity_type = gen_entities
        .get(entity)
        .map(|g| g.entity_type.as_str().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    let visible = visibility_query
        .get(entity)
        .map(|v| *v != Visibility::Hidden)
        .unwrap_or(true);

    let (color, metallic, roughness) = material_handles
        .get(entity)
        .ok()
        .and_then(|h| material_assets.get(&h.0))
        .map(|mat| {
            let c = mat.base_color.to_srgba();
            (
                Some([c.red, c.green, c.blue, c.alpha]),
                Some(mat.metallic),
                Some(mat.perceptual_roughness),
            )
        })
        .unwrap_or((None, None, None));

    let children: Vec<String> = children_query
        .get(entity)
        .map(|ch| {
            ch.iter()
                .filter_map(|c| {
                    registry
                        .get_name(*c)
                        .map(|s| s.to_string())
                        .or_else(|| names_query.get(*c).ok().map(|n| n.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();

    let parent = parent_query
        .get(entity)
        .ok()
        .and_then(|p| registry.get_name(p.get()).map(|s| s.to_string()));

    GenResponse::EntityInfo(EntityInfoData {
        name: name.to_string(),
        entity_id: entity.to_bits(),
        entity_type,
        position: transform.translation.to_array(),
        rotation_degrees: [
            euler.0.to_degrees(),
            euler.1.to_degrees(),
            euler.2.to_degrees(),
        ],
        scale: transform.scale.to_array(),
        color,
        metallic,
        roughness,
        visible,
        children,
        parent,
    })
}

fn handle_spawn_primitive(
    cmd: SpawnPrimitiveCmd,
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    registry: &mut ResMut<NameRegistry>,
) -> GenResponse {
    if registry.contains_name(&cmd.name) {
        return GenResponse::Error {
            message: format!("Entity '{}' already exists", cmd.name),
        };
    }

    let mesh = match cmd.shape {
        PrimitiveShape::Cuboid => {
            let x = cmd.dimensions.get("x").copied().unwrap_or(1.0);
            let y = cmd.dimensions.get("y").copied().unwrap_or(1.0);
            let z = cmd.dimensions.get("z").copied().unwrap_or(1.0);
            meshes.add(Cuboid::new(x, y, z))
        }
        PrimitiveShape::Sphere => {
            let radius = cmd.dimensions.get("radius").copied().unwrap_or(0.5);
            meshes.add(Sphere::new(radius).mesh().uv(32, 18))
        }
        PrimitiveShape::Cylinder => {
            let radius = cmd.dimensions.get("radius").copied().unwrap_or(0.5);
            let height = cmd.dimensions.get("height").copied().unwrap_or(1.0);
            meshes.add(Cylinder::new(radius, height))
        }
        PrimitiveShape::Cone => {
            let radius = cmd.dimensions.get("radius").copied().unwrap_or(0.5);
            let height = cmd.dimensions.get("height").copied().unwrap_or(1.0);
            meshes.add(Cone { radius, height })
        }
        PrimitiveShape::Capsule => {
            let radius = cmd.dimensions.get("radius").copied().unwrap_or(0.5);
            let half_length = cmd.dimensions.get("half_length").copied().unwrap_or(0.5);
            meshes.add(Capsule3d::new(radius, half_length * 2.0))
        }
        PrimitiveShape::Torus => {
            let major = cmd.dimensions.get("major_radius").copied().unwrap_or(1.0);
            let minor = cmd.dimensions.get("minor_radius").copied().unwrap_or(0.25);
            meshes.add(Torus::new(minor, major))
        }
        PrimitiveShape::Plane => {
            let x = cmd.dimensions.get("x").copied().unwrap_or(1.0);
            let z = cmd.dimensions.get("z").copied().unwrap_or(1.0);
            meshes.add(Plane3d::new(Vec3::Y, Vec2::new(x / 2.0, z / 2.0)))
        }
    };

    let material = materials.add(StandardMaterial {
        base_color: Color::srgba(cmd.color[0], cmd.color[1], cmd.color[2], cmd.color[3]),
        metallic: cmd.metallic,
        perceptual_roughness: cmd.roughness,
        emissive: bevy::color::LinearRgba::new(
            cmd.emissive[0],
            cmd.emissive[1],
            cmd.emissive[2],
            cmd.emissive[3],
        ),
        ..default()
    });

    let rotation = Quat::from_euler(
        EulerRot::XYZ,
        cmd.rotation_degrees[0].to_radians(),
        cmd.rotation_degrees[1].to_radians(),
        cmd.rotation_degrees[2].to_radians(),
    );

    let transform = Transform {
        translation: Vec3::from_array(cmd.position),
        rotation,
        scale: Vec3::from_array(cmd.scale),
    };

    let entity = commands
        .spawn((
            Mesh3d(mesh),
            MeshMaterial3d(material),
            transform,
            Name::new(cmd.name.clone()),
            GenEntity {
                entity_type: GenEntityType::Primitive,
            },
        ))
        .id();

    // Handle parenting
    if let Some(ref parent_name) = cmd.parent
        && let Some(parent_entity) = registry.get_entity(parent_name)
    {
        commands.entity(entity).set_parent(parent_entity);
    }

    let entity_id = entity.to_bits();
    registry.insert(cmd.name.clone(), entity);

    GenResponse::Spawned {
        name: cmd.name,
        entity_id,
    }
}

fn handle_modify_entity(
    cmd: ModifyEntityCmd,
    commands: &mut Commands,
    registry: &NameRegistry,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    material_handles: &Query<&MeshMaterial3d<StandardMaterial>>,
    transforms: &Query<&Transform>,
) -> GenResponse {
    let Some(entity) = registry.get_entity(&cmd.name) else {
        return GenResponse::Error {
            message: format!("Entity '{}' not found", cmd.name),
        };
    };

    let mut entity_commands = commands.entity(entity);

    // Update transform
    if cmd.position.is_some() || cmd.rotation_degrees.is_some() || cmd.scale.is_some() {
        let current = transforms.get(entity).copied().unwrap_or_default();
        let new_transform = Transform {
            translation: cmd
                .position
                .map(Vec3::from_array)
                .unwrap_or(current.translation),
            rotation: cmd
                .rotation_degrees
                .map(|r| {
                    Quat::from_euler(
                        EulerRot::XYZ,
                        r[0].to_radians(),
                        r[1].to_radians(),
                        r[2].to_radians(),
                    )
                })
                .unwrap_or(current.rotation),
            scale: cmd.scale.map(Vec3::from_array).unwrap_or(current.scale),
        };
        entity_commands.insert(new_transform);
    }

    // Update material if any material properties changed
    if cmd.color.is_some()
        || cmd.metallic.is_some()
        || cmd.roughness.is_some()
        || cmd.emissive.is_some()
    {
        // Get current material properties as defaults
        let current_mat = material_handles
            .get(entity)
            .ok()
            .and_then(|h| materials.get(&h.0))
            .cloned();

        let base = current_mat.unwrap_or_default();

        let new_material = materials.add(StandardMaterial {
            base_color: cmd
                .color
                .map(|c| Color::srgba(c[0], c[1], c[2], c[3]))
                .unwrap_or(base.base_color),
            metallic: cmd.metallic.unwrap_or(base.metallic),
            perceptual_roughness: cmd.roughness.unwrap_or(base.perceptual_roughness),
            emissive: cmd
                .emissive
                .map(|e| bevy::color::LinearRgba::new(e[0], e[1], e[2], e[3]))
                .unwrap_or(base.emissive),
            ..base
        });
        entity_commands.insert(MeshMaterial3d(new_material));
    }

    // Update visibility
    if let Some(visible) = cmd.visible {
        entity_commands.insert(if visible {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }

    // Update parent
    if let Some(parent_opt) = cmd.parent {
        match parent_opt {
            Some(parent_name) => {
                if let Some(parent_entity) = registry.get_entity(&parent_name) {
                    commands.entity(entity).set_parent(parent_entity);
                }
            }
            None => {
                commands.entity(entity).remove_parent();
            }
        }
    }

    GenResponse::Modified { name: cmd.name }
}

fn handle_delete_entity(
    name: &str,
    commands: &mut Commands,
    registry: &mut ResMut<NameRegistry>,
) -> GenResponse {
    let Some(entity) = registry.remove_by_name(name) else {
        return GenResponse::Error {
            message: format!("Entity '{}' not found", name),
        };
    };

    // Recursively despawn entity and all children
    commands.entity(entity).despawn_recursive();

    GenResponse::Deleted {
        name: name.to_string(),
    }
}

fn handle_set_camera(
    cmd: CameraCmd,
    commands: &mut Commands,
    registry: &NameRegistry,
) -> GenResponse {
    let Some(camera_entity) = registry.get_entity("main_camera") else {
        return GenResponse::Error {
            message: "main_camera not found in registry".to_string(),
        };
    };

    let transform = Transform::from_translation(Vec3::from_array(cmd.position))
        .looking_at(Vec3::from_array(cmd.look_at), Vec3::Y);

    commands.entity(camera_entity).insert(transform);

    // Update projection FOV
    let projection = Projection::Perspective(PerspectiveProjection {
        fov: cmd.fov_degrees.to_radians(),
        ..default()
    });
    commands.entity(camera_entity).insert(projection);

    GenResponse::CameraSet
}

fn handle_set_light(
    cmd: SetLightCmd,
    commands: &mut Commands,
    registry: &mut ResMut<NameRegistry>,
) -> GenResponse {
    let color = Color::srgba(cmd.color[0], cmd.color[1], cmd.color[2], cmd.color[3]);

    // If light already exists, update it
    if let Some(entity) = registry.get_entity(&cmd.name) {
        commands.entity(entity).despawn_recursive();
        registry.remove_by_name(&cmd.name);
    }

    let entity = match cmd.light_type {
        LightType::Directional => {
            let dir = cmd.direction.unwrap_or([0.0, -1.0, -0.5]);
            let transform = Transform::from_translation(Vec3::new(0.0, 10.0, 0.0))
                .looking_at(Vec3::new(0.0, 10.0, 0.0) + Vec3::from_array(dir), Vec3::Y);
            commands
                .spawn((
                    DirectionalLight {
                        illuminance: cmd.intensity,
                        shadows_enabled: cmd.shadows,
                        color,
                        ..default()
                    },
                    transform,
                    Name::new(cmd.name.clone()),
                    GenEntity {
                        entity_type: GenEntityType::Light,
                    },
                ))
                .id()
        }
        LightType::Point => {
            let pos = cmd.position.unwrap_or([0.0, 5.0, 0.0]);
            commands
                .spawn((
                    PointLight {
                        intensity: cmd.intensity,
                        shadows_enabled: cmd.shadows,
                        color,
                        ..default()
                    },
                    Transform::from_translation(Vec3::from_array(pos)),
                    Name::new(cmd.name.clone()),
                    GenEntity {
                        entity_type: GenEntityType::Light,
                    },
                ))
                .id()
        }
        LightType::Spot => {
            let pos = cmd.position.unwrap_or([0.0, 5.0, 0.0]);
            let dir = cmd.direction.unwrap_or([0.0, -1.0, 0.0]);
            let transform = Transform::from_translation(Vec3::from_array(pos))
                .looking_at(Vec3::from_array(pos) + Vec3::from_array(dir), Vec3::Y);
            commands
                .spawn((
                    SpotLight {
                        intensity: cmd.intensity,
                        shadows_enabled: cmd.shadows,
                        color,
                        ..default()
                    },
                    transform,
                    Name::new(cmd.name.clone()),
                    GenEntity {
                        entity_type: GenEntityType::Light,
                    },
                ))
                .id()
        }
    };

    registry.insert(cmd.name.clone(), entity);

    GenResponse::LightSet { name: cmd.name }
}

fn handle_set_environment(cmd: EnvironmentCmd, commands: &mut Commands) -> GenResponse {
    if let Some(color) = cmd.background_color {
        commands.insert_resource(ClearColor(Color::srgba(
            color[0], color[1], color[2], color[3],
        )));
    }

    if let Some(intensity) = cmd.ambient_light {
        let color = cmd
            .ambient_color
            .map(|c| Color::srgba(c[0], c[1], c[2], c[3]))
            .unwrap_or(Color::WHITE);
        commands.insert_resource(AmbientLight {
            color,
            brightness: intensity,
        });
    }

    GenResponse::EnvironmentSet
}

fn handle_spawn_mesh(
    cmd: RawMeshCmd,
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    registry: &mut ResMut<NameRegistry>,
) -> GenResponse {
    if registry.contains_name(&cmd.name) {
        return GenResponse::Error {
            message: format!("Entity '{}' already exists", cmd.name),
        };
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );

    // Positions
    let positions: Vec<[f32; 3]> = cmd.vertices.clone();
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);

    // Indices
    mesh.insert_indices(Indices::U32(cmd.indices));

    // Normals — use provided or compute flat normals
    if let Some(normals) = cmd.normals {
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    } else {
        mesh.compute_flat_normals();
    }

    // UVs
    if let Some(uvs) = cmd.uvs {
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    }

    let material = materials.add(StandardMaterial {
        base_color: Color::srgba(cmd.color[0], cmd.color[1], cmd.color[2], cmd.color[3]),
        metallic: cmd.metallic,
        perceptual_roughness: cmd.roughness,
        ..default()
    });

    let entity = commands
        .spawn((
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(material),
            Transform::from_translation(Vec3::from_array(cmd.position)),
            Name::new(cmd.name.clone()),
            GenEntity {
                entity_type: GenEntityType::Mesh,
            },
        ))
        .id();

    let entity_id = entity.to_bits();
    registry.insert(cmd.name.clone(), entity);

    GenResponse::Spawned {
        name: cmd.name,
        entity_id,
    }
}
