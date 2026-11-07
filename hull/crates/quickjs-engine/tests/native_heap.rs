//! Run with native-allocator-test to exercise the production allocation format
//! through the full C runtime, including its accounting and failure recovery.
use hull_quickjs::{Engine, Limits};

#[test]
fn heap_limit_rejects_large_buffers_and_recovers_after_reallocation_and_gc() {
    let mut engine = Engine::with_limits(Limits {
        heap_bytes: 512 * 1024,
        ..Limits::default()
    }).unwrap();
    engine.eval(br#"
        let denied = false;
        try { globalThis.tooLarge = new ArrayBuffer(2 * 1024 * 1024); }
        catch (error) { denied = true; }
        if (!denied) throw Error('heap limit was not enforced');
        for (let round = 0; round < 100; round++) {
            let array = [];
            for (let i = 0; i < 256; i++) array.push(i);
            if (array[255] !== 255) throw Error('reallocation corrupted values');
        }
        if (typeof tooLarge !== 'undefined') throw Error('failed allocation escaped');
    "#, "native-heap-limit.js", false).unwrap();
    engine.eval(b"if ([1, 2, 3].join(',') !== '1,2,3') throw Error('recovery failed')",
        "native-heap-recovery.js", false).unwrap();
    while engine.execute_pending_job().unwrap() {}
}
