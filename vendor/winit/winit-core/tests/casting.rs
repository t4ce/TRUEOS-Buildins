//! The public casting macro must not require downstream crates to import alloc.
use core::any::Any;

trait Backend: Any {}
struct First;
struct Second;
impl Backend for First {}
impl Backend for Second {}
winit_core::impl_dyn_casting!(Backend);

#[test]
fn owned_cast_preserves_values_and_does_not_require_an_alloc_import() {
    let value: Box<dyn Backend> = Box::new(First);
    assert!(value.cast::<First>().is_ok());
    let value: Box<dyn Backend> = Box::new(Second);
    let value = value.cast::<First>().err().expect("wrong backend must stay owned");
    assert!(value.cast::<Second>().is_ok());
}
