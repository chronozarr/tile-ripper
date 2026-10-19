//! Suppress native URL-bearing diagnostics without changing the host's handler.
use std::{
    marker::PhantomData,
    os::raw::{c_char, c_int},
    rc::Rc,
};
type Handler = Option<unsafe extern "system" fn(c_int, c_int, *const c_char)>;
// Stable CPL API already linked through GDAL. The Rust wrapper has no scoped API.
unsafe extern "system" {
    fn CPLPushErrorHandler(handler: Handler);
    fn CPLPopErrorHandler();
}
unsafe extern "system" fn ignore(_: c_int, _: c_int, _: *const c_char) {}
/// !Send/!Sync enforces pairing push/pop on the same thread, including unwinding.
pub(crate) struct QuietNative(PhantomData<Rc<()>>);
impl QuietNative {
    pub(crate) fn enter() -> Self {
        Self::push(Some(ignore))
    }
    fn push(handler: Handler) -> Self {
        // SAFETY: static C ABI callback, no captured data; GDAL owns its TLS stack.
        unsafe {
            CPLPushErrorHandler(handler);
        }
        Self(PhantomData)
    }
}
impl Drop for QuietNative {
    fn drop(&mut self) {
        // SAFETY: paired with this guard's push on the same thread.
        unsafe {
            CPLPopErrorHandler();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    unsafe extern "system" {
        fn CPLGetErrorHandler(data: *mut *mut std::ffi::c_void) -> Handler;
    }
    std::thread_local! {static CALLS:std::cell::Cell<usize>=const {std::cell::Cell::new(0)};}
    unsafe extern "system" fn count(_: c_int, _: c_int, _: *const c_char) {
        CALLS.with(|n| n.set(n.get() + 1));
    }
    fn handler() -> Handler {
        unsafe { CPLGetErrorHandler(std::ptr::null_mut()) }
    }
    fn address(h: Handler) -> Option<usize> {
        h.map(|f| f as usize)
    }
    #[test]
    fn native_handler_is_scoped_and_restored_on_unwind() {
        let previous = address(handler());
        {
            let _outer = QuietNative::push(Some(count));
            let result = std::panic::catch_unwind(|| {
                let _inner = QuietNative::enter();
                assert_eq!(address(handler()), address(Some(ignore)));
                unsafe {
                    handler().unwrap()(3, 1, std::ptr::null());
                }
                panic!("cancel");
            });
            assert!(result.is_err());
            assert_eq!(address(handler()), address(Some(count)));
            assert_eq!(CALLS.with(|n| n.get()), 0);
        }
        assert_eq!(address(handler()), previous);
    }
}
