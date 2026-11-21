// Sodium's adapter to the game: what the rules in `crate::game::sodium` (runtime.rs)
// decide, done in the game that simulates the world (single player's server thread, a LAN
// host's, in-frame single player). Injected as `crate::game::sodium_native`. A window joined
// to a server does nothing: the server's world is not its own.
use super::sodium::{self as sn};
use super::Game;

impl Game<'_> {
    /// The full-power performance profile, applied once a frame when the mod is on and the
    /// player is in a world of their own. It raises every load off the frame the profile holds,
    /// and only raises: the player's own choices are the starting point and the mod never lowers
    /// one, so a no-settings sodium means the optimization is simply there.
    pub(super) fn sodium_full_power(&mut self) {
        if !self.native_on(sn::ID) || !self.native_playing() {
            return;
        }
        let profile = sn::FULL_POWER;
        let graphics = &mut self.settings.horizon;
        graphics.entity_culling = profile.entity_culling;
        graphics.leaf_culling = profile.leaf_culling;
        graphics.cave_culling = profile.cave_culling;
        graphics.cache_shulker_boxes = profile.block_entity_cache;
        graphics.cache_signs = profile.block_entity_cache;
        graphics.cache_decorated_pots = profile.block_entity_cache;
        graphics.cache_bells = profile.block_entity_cache;
        graphics.cache_chests = profile.block_entity_cache;
        graphics.cache_banners = profile.block_entity_cache;
        graphics.cache_copper_golem_statues = profile.block_entity_cache;
        graphics.translucent_batching = profile.translucent_batching;
    }
}
