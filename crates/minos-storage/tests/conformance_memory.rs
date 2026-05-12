use minos_storage::{conformance::run_conformance_suite, InMemoryStorage};

#[test]
fn in_memory_passes_conformance_suite() {
    let s = InMemoryStorage::new();
    run_conformance_suite(&s);
}
