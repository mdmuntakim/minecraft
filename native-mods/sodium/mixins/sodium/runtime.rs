// Sodium's rules, with nothing of the game in them: the full-power performance
// profile the mod applies when it is on, and the switches it exists to turn on. Injected
// as `crate::game::sodium`; native.rs applies it to the game that runs the world (single
// player's server thread, a LAN host's, in-frame single player).
//
// Sodium is the performance mod: it takes the load off the frame. This bundle ships the
// whole of its profile as it is, at the full power it is named for, so a phone does not have
// to be tuned by hand - the player gets the optimization without a settings screen, and the
// mod only ever raises what the player left off, never lowering a choice they made.
use horizon_client::graphics::{EntityCullingMode, LeafCullingMode};

pub const ID: &str = "sodium";
pub const VERSION: &str = "0.1.0";
pub const DESCRIPTION: &str = "Automatic performance: entity and leaf culling, cave culling, the \
     block-entity caches and translucent batching, all at the full power Sodium names them at, with \
     no settings to tune.";

/// The full-power performance profile: every load the sodium family of mods can take off the
/// frame, at its strongest setting that keeps the picture (the player's own resource packs and
/// shader packs still decide what may be culled).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Profile {
    /// Entity culling: the vanilla same-frame CPU test, or asynchronous, the worker-based test
    /// that keeps a mesh only while its camera, bounds and occluder evidence still match. Full
    /// power is asynchronous.
    pub entity_culling: EntityCullingMode,
    /// Leaf faces inside a canopy: off, solid, solid-aggressive or hollow. Full power is
    /// solid-aggressive: the shell a canopy needs to read as leaves, thinned as far as the
    /// silhouette still holds.
    pub leaf_culling: LeafCullingMode,
    /// Cave culling: skip the sections hidden behind closed ones.
    pub cave_culling: bool,
    /// The block-entity model caches (shulker boxes, sign text, decorated pots, bells, chests,
    /// banners, copper-golem statues), applied to all of them at once.
    pub block_entity_cache: bool,
    /// Translucent quads in vertex buffers of their own, so their back-to-front draws go out in
    /// a few calls instead of page switches.
    pub translucent_batching: bool,
}

/// The full-power profile sodium ships: entity culling at full power, leaf culling solid
/// aggressive, cave culling, the block-entity caches and translucent batching, all on.
pub const FULL_POWER: Profile = Profile {
    entity_culling: EntityCullingMode::Asynchronous,
    leaf_culling: LeafCullingMode::SolidAggressive,
    cave_culling: true,
    block_entity_cache: true,
    translucent_batching: true,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_full_power_profile_culls_without_a_player_choice() {
        let profile = FULL_POWER;
        assert_eq!(profile.entity_culling, EntityCullingMode::Asynchronous);
        assert_eq!(profile.leaf_culling, LeafCullingMode::SolidAggressive);
        assert!(profile.cave_culling);
        assert!(profile.block_entity_cache);
        assert!(profile.translucent_batching);
    }

    #[test]
    fn the_full_power_profile_never_disables_what_it_keeps() {
        let profile = FULL_POWER;
        assert_ne!(profile.entity_culling, EntityCullingMode::Off);
        assert_ne!(profile.leaf_culling, LeafCullingMode::Off);
    }
}
