# TRUEOS parking_lot_core overlay

Based on crates.io `parking_lot_core` 0.9.12. Upstream sources, package metadata,
and both licenses are retained.

The `target_os = "trueos"` thread parker uses TRUEOS's existing scoped native
wait/wake ABI instead of constructing a pthread mutex and condition variable.
An atomic flag owns the unpark token; wait generations close the wake-before-wait
race, and finite deadlines round up to milliseconds and are rechecked. Deferred
unpark handles retain only the opaque numeric wait key, so they never dereference
a parker that has already exited or been reused.

The backend is selected before the Unix backend, and the libc dependency is
excluded for TRUEOS. Other targets retain their upstream backend and dependencies.
