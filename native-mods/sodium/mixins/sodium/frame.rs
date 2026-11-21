// native_hooks::Game::native_frame (a hook point): once a frame, in a world or not, after
// Horizon's extras. The sodium mod applies its full-power performance profile when it is on.
fn sodium_frame(game: &mut crate::game::Game<'_>) {
    game.sodium_full_power();
}
