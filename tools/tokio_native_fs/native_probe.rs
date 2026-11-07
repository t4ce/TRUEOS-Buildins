#![no_std]
extern crate alloc;
use trueos::{
    fs,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom},
};

pub async fn exercise_native_fs() -> tokio::io::Result<()> {
    fs::write("probe", b"native filesystem").await?;
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("probe")
        .await?;
    file.write_all(b"same open file").await?;
    file.seek(SeekFrom::Start(0)).await?;
    let mut data = alloc::vec::Vec::new();
    file.read_to_end(&mut data).await?;
    file.set_len(3).await?;
    file.sync_all().await?;
    let cloned = file.try_clone().await?;
    let metadata = cloned.metadata().await?;
    cloned.set_permissions(metadata.permissions()).await?;
    let native = cloned.into_native().await;
    let _file = fs::File::from_native(native);
    let _time_error = file
        .set_modified(tokio::fs::FileTimestamp::from_unix_duration(
            core::time::Duration::ZERO,
        ))
        .await;
    tokio::fs::set_permissions("probe", metadata.permissions()).await?;
    let _ = fs::read("probe").await?;
    let _ = fs::read_to_string("probe").await?;
    let canonical = fs::canonicalize("probe").await?;
    let _ = fs::try_exists(canonical).await?;
    let mut entries = fs::read_dir("/").await?;
    while let Some(entry) = entries.next_entry().await? {
        let _ = entry.metadata().await?;
    }
    fs::copy("probe", "copy").await?;
    fs::rename("copy", "renamed").await?;
    fs::remove_file("renamed").await?;
    fs::create_dir_all("probe-dir").await?;
    tokio::fs::remove_dir("probe-dir").await?;
    Ok(())
}

pub async fn exercise_native_stdio() -> tokio::io::Result<()> {
    let mut out = trueos::io::stdout();
    out.write_all(b"native stdout\n").await?;
    out.flush().await?;
    let mut err = trueos::io::stderr();
    err.write_all(b"native stderr\n").await?;
    err.flush().await?;
    let mut bytes = [0; 16];
    let _ = trueos::io::stdin().read(&mut bytes).await?;
    Ok(())
}

pub async fn exercise_native_network() -> trueos::io::Result<()> {
    let listener = trueos::net::TcpListener::bind("0.0.0.0:0").await?;
    let (mut stream, _) = listener.accept().await?;
    stream.write_all(b"native network").await?;
    let socket = trueos::net::UdpSocket::bind("0.0.0.0:0").await?;
    let _ = socket.send_to(b"native", listener.local_addr()?).await?;
    Ok(())
}
