#![no_std]
#![no_main]
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let _ = tokio_native_fs_probe::exercise_native_fs().await;
        let _ = tokio_native_fs_probe::exercise_native_stdio().await;
        let _ = tokio_native_fs_probe::exercise_native_network().await;
    });
    trueos::entry::finish()
}
