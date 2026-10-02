//! Asset file names and room loading.
//!
//! Mesh, texture, and audio bytes load from disk through the public asset
//! APIs into one [`GameAssets`] value; the binaries move each store into
//! the world as a resource, so the audio trigger resolves pickup voices
//! against the world's [`AssetStore<Sound>`](canary_assets::AssetStore)
//! every tick. The room document loads through the authored-state seam.

use std::path::{Path, PathBuf};

use canary_assets::{
    load_mesh, load_sound, load_texture, AssetError, AssetHandle, AssetStore, Mesh, Sound, Texture,
};
use canary_state::{AuthoredDocument, StateError};

/// The authored room document.
pub const ROOM_FILE: &str = "room.json";
/// Quad mesh fixture (player sprite geometry).
pub const QUAD_MESH: &str = "quad.glb";
/// Box mesh fixture (goal marker geometry).
pub const BOX_MESH: &str = "box.glb";
/// Sprite texture fixture.
pub const SPRITE_PNG: &str = "rgba2x2.png";
/// Pickup sound fixture.
pub const PICKUP_SOUND: &str = "tone-mono-8k.wav";

/// The example's asset directory: `<crate>/assets`.
#[must_use]
pub fn asset_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("assets")
}

/// Loads the authored room document at `path`.
pub fn load_room(path: &Path) -> Result<AuthoredDocument, StateError> {
    AuthoredDocument::load(path)
}

/// Every loaded game asset: one store per type plus the handles the
/// simulation and scene bind against. The binaries move the stores into
/// the world as resources and hand the handles to the setup seams.
pub struct GameAssets {
    /// Loaded meshes, keyed by the handles below.
    pub meshes: AssetStore<Mesh>,
    /// Player sprite geometry ([`QUAD_MESH`]).
    pub player_mesh: AssetHandle<Mesh>,
    /// Goal marker geometry ([`BOX_MESH`]).
    pub goal_mesh: AssetHandle<Mesh>,
    /// Loaded textures, keyed by the handle below.
    pub textures: AssetStore<Texture>,
    /// Sprite texture ([`SPRITE_PNG`]).
    pub sprite: AssetHandle<Texture>,
    /// Loaded sounds, keyed by the handle below.
    pub sounds: AssetStore<Sound>,
    /// Pickup voice ([`PICKUP_SOUND`]).
    pub pickup_sound: AssetHandle<Sound>,
}

/// Loads every game asset from [`asset_dir`]: the player and goal meshes,
/// the sprite texture, and the pickup sound. Typed errors name the file;
/// nothing panics on missing or malformed content.
pub fn load_game_assets() -> Result<GameAssets, AssetError> {
    let dir = asset_dir();
    let mut meshes = AssetStore::new();
    let player_mesh = meshes.insert(first_mesh(&dir.join(QUAD_MESH))?);
    let goal_mesh = meshes.insert(first_mesh(&dir.join(BOX_MESH))?);
    let mut textures = AssetStore::new();
    let sprite = textures.insert(load_texture(&dir.join(SPRITE_PNG))?);
    let mut sounds = AssetStore::new();
    let pickup_sound = sounds.insert(load_sound(&dir.join(PICKUP_SOUND))?);
    Ok(GameAssets {
        meshes,
        player_mesh,
        goal_mesh,
        textures,
        sprite,
        sounds,
        pickup_sound,
    })
}

/// Loads the first mesh in a GLB file: the fixtures carry exactly one
/// primitive each, so "the mesh" is unambiguous, and an empty file is a
/// typed format error rather than an index panic.
fn first_mesh(path: &Path) -> Result<Mesh, AssetError> {
    load_mesh(path)?
        .into_iter()
        .next()
        .ok_or_else(|| AssetError::invalid_format(path, "GLB contains no meshes"))
}

/// The `$asset` resolver for the spawner: maps an asset id named in the
/// room document to its on-disk path string.
///
/// Accepts both the short gameplay names (`player.glb`, `tile.png`,
/// `pickup.wav`) and the real fixture file names, so authored content can
/// speak gameplay while the loader stays honest about which bytes exist.
/// Unknown ids resolve to `None`, which aborts the spawn before the first
/// entity (see [`AuthoredSpawner`](canary_runtime::AuthoredSpawner)). The
/// current `room.json` carries no `$asset` markers, so this resolver idles
/// through today's spawn and pays off the moment content names an asset.
pub fn resolve_asset(id: &str) -> Option<String> {
    let file = match id {
        "player.glb" | QUAD_MESH => QUAD_MESH,
        "goal.glb" | BOX_MESH => BOX_MESH,
        "tile.png" | "sprite.png" | SPRITE_PNG => SPRITE_PNG,
        "pickup.wav" | PICKUP_SOUND => PICKUP_SOUND,
        _ => return None,
    };
    Some(asset_dir().join(file).to_string_lossy().into_owned())
}
