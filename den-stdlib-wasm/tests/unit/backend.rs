use super::{USES_PULLEY, new_engine};

#[test]
fn the_engine_is_pulley_exactly_when_the_build_selects_it() {
    let engine = new_engine().expect("engine");
    assert_eq!(
        engine.is_pulley(),
        USES_PULLEY,
        "Engine::is_pulley() must match the jit/host-ISA selection"
    );
}
