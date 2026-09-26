//! Guards that the kernel's C++ static initialisers have run.
//!
//! `Manifold::sphere` and `Manifold::refine` touch static state set up by those
//! initialisers. When the link drops them the failure is not an assertion: the
//! test *process* dies on SIGFPE, so a passing assertion here means the
//! initialisers ran. Cause and fix: the `[env]` block of `.cargo/config.toml`
//! and "Native build prerequisite" in `docs/guide/recipes.md`.

use manifold_csg::Manifold;

#[test]
fn sphere_is_constructed() {
    let sphere = Manifold::sphere(1.5, 32);
    assert!(sphere.status().is_ok(), "sphere is not a valid solid");
    assert_eq!(sphere.num_tri(), 512, "sphere triangle count changed");
    let volume = sphere.volume();
    assert!(
        (volume - 13.821).abs() < 0.05,
        "sphere volume {volume} is not within 0.05 of 13.821"
    );
}

#[test]
fn refine_subdivides() {
    let refined = Manifold::tetrahedron().refine(2);
    assert!(refined.status().is_ok(), "refined solid is not valid");
    assert_eq!(
        refined.num_tri(),
        16,
        "refine did not subdivide as expected"
    );
}
