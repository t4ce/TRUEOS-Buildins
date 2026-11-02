extern crate alloc;
use alloc::{vec, vec::Vec};
use trauma::path;
mod support;
use std::fs;
use support::{Directory, Reply, Server};
use trauma::{
    download::{Download, Status, Summary},
    downloader::Downloader,
};

fn run(server: &Server, directory: &Directory, resumable: bool) -> Summary {
    let download = Download::builder().url(&server.url).unwrap().build();
    let downloader = Downloader::builder()
        .directory(directory.path())
        .retries(0)
        .resumable(resumable)
        .build();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    // Readable bytes must be committed before download returns, not runtime drop.
    let summary = runtime
        .block_on(downloader.download(&[download]))
        .pop()
        .unwrap();
    if directory.file().exists() {
        assert_eq!(summary.size(), directory.bytes().len() as u64);
    }
    summary
}
fn prefix_replies() -> Vec<Reply> {
    vec![
        Reply::metadata("HEAD", 1000, "\"v1\""),
        Reply::partial(1000, vec![b'a'; 500]),
    ]
}
fn ranged() -> Reply {
    Reply::new("GET", 206)
        .header("ETag", "\"v1\"")
        .header("Content-Range", "bytes 500-999/1000")
        .header("Content-Length", 500)
        .body(vec![b'b'; 500])
}
fn assert_failed(summary: &Summary) {
    assert!(matches!(summary.status(), Status::Fail(_)), "{summary:?}");
}
fn make_prefix(server: &Server, directory: &Directory) {
    assert_failed(&run(server, directory, true));
    assert_eq!(directory.bytes(), vec![b'a'; 500]);
    let saved = fs::read_to_string(directory.checkpoint()).unwrap();
    assert!(saved.ends_with("partial\n"));
    assert!(!saved.contains(&server.url));
}

#[test]
fn resumes_actual_interrupted_checkpoint_and_never_skips_500_byte_remainder() {
    let directory = Directory::new();
    let mut replies = prefix_replies();
    replies.extend([
        Reply::metadata("HEAD", 1000, "\"v1\""),
        ranged(),
        Reply::metadata("HEAD", 1000, "\"v1\""),
    ]);
    let server = Server::new(replies);
    make_prefix(&server, &directory);
    let summary = run(&server, &directory, true);
    assert_eq!(summary.status(), &Status::Success);
    assert_eq!(summary.size(), 1000);
    assert_eq!(
        directory.bytes(),
        [vec![b'a'; 500], vec![b'b'; 500]].concat()
    );
    assert!(fs::read_to_string(directory.checkpoint())
        .unwrap()
        .ends_with("complete\n"));
    let summary = run(&server, &directory, true);
    assert!(matches!(summary.status(), Status::Skipped(_)));
    assert_eq!(summary.size(), 1000);
    let requests = server.finish();
    assert_eq!(requests[3].header("range"), Some("bytes=500-"));
    assert_eq!(requests[3].header("if-range"), Some("\"v1\""));
    assert!(requests
        .iter()
        .all(|r| r.header("accept-encoding") == Some("identity")));
}

#[test]
fn ignored_range_or_if_range_change_restarts_with_correct_bytes_and_count() {
    let directory = Directory::new();
    let mut replies = prefix_replies();
    replies.extend([
        Reply::metadata("HEAD", 1000, "\"v1\""),
        Reply::metadata("GET", 1000, "\"v2\"").body(vec![b'z'; 1000]),
    ]);
    let server = Server::new(replies);
    make_prefix(&server, &directory);
    assert_eq!(run(&server, &directory, true).status(), &Status::Success);
    assert_eq!(directory.bytes(), vec![b'z'; 1000]);
    assert_eq!(server.finish()[3].header("if-range"), Some("\"v1\""));
}

#[test]
fn malformed_or_conflicting_partial_responses_preserve_prefix() {
    let responses = vec![
        Reply::new("GET", 206)
            .header("ETag", "\"v1\"")
            .header("Content-Range", "bytes 499-999/1000")
            .header("Content-Length", 501)
            .body(vec![b'x'; 501]),
        Reply::new("GET", 206)
            .header("ETag", "\"v1\"")
            .header("Content-Range", "bytes nope/1000")
            .header("Content-Length", 0),
        Reply::new("GET", 206)
            .header("ETag", "\"v2\"")
            .header("Content-Range", "bytes 500-999/1000")
            .header("Content-Length", 500)
            .body(vec![b'x'; 500]),
        Reply::new("GET", 206)
            .header("ETag", "\"v1\"")
            .header("Content-Range", "bytes 500-1000/1001")
            .header("Content-Length", 501)
            .body(vec![b'x'; 501]),
        Reply::new("GET", 206)
            .header("ETag", "\"v1\"")
            .header("Content-Length", 0),
        ranged().header("Content-Range", "bytes 499-998/1000"),
        Reply::new("GET", 206)
            .header("ETag", "\"v1\"")
            .header("Content-Range", "bytes 500-1000/1000")
            .header("Content-Length", 0),
        Reply::new("GET", 206)
            .header("ETag", "\"v1\"")
            .header("Content-Range", "bytes 500-999/1000")
            .header("Content-Length", 499),
        ranged().header("Content-Type", "multipart/byteranges; boundary=test"),
        ranged().header("ETag", "\"v2\""),
        Reply::new("GET", 206)
            .header("ETag", "\"v1\"")
            .header("Content-Range", "bytes 500-999/*")
            .header("Content-Length", 500),
        Reply::new("GET", 416)
            .header("Content-Range", "bytes */1000")
            .header("Content-Length", 0),
        ranged().header("Content-Encoding", "gzip"),
    ];
    for response in responses {
        let directory = Directory::new();
        let mut replies = prefix_replies();
        replies.extend([Reply::metadata("HEAD", 1000, "\"v1\""), response]);
        let server = Server::new(replies);
        make_prefix(&server, &directory);
        let checkpoint = fs::read(directory.checkpoint()).unwrap();
        assert_failed(&run(&server, &directory, true));
        assert_eq!(directory.bytes(), vec![b'a'; 500]);
        assert_eq!(fs::read(directory.checkpoint()).unwrap(), checkpoint);
        server.finish();
    }
}

#[test]
fn equal_length_legacy_file_is_replaced_and_head_errors_never_skip() {
    let directory = Directory::new();
    fs::write(directory.file(), vec![b'x'; 1000]).unwrap();
    let server = Server::new(vec![
        Reply::metadata("HEAD", 1000, "\"v1\""),
        Reply::metadata("GET", 1000, "\"v1\"").body(vec![b'a'; 1000]),
        Reply::new("HEAD", 403).header("Content-Length", 1000),
    ]);
    assert_eq!(run(&server, &directory, true).status(), &Status::Success);
    assert_eq!(directory.bytes(), vec![b'a'; 1000]);
    assert_failed(&run(&server, &directory, true));
    assert_eq!(directory.bytes(), vec![b'a'; 1000]);
    assert!(server.finish()[1].header("range").is_none());
}

#[test]
fn changed_completed_validator_forces_full_get_even_at_same_length() {
    let directory = Directory::new();
    let server = Server::new(vec![
        Reply::metadata("HEAD", 4, "\"v1\""),
        Reply::metadata("GET", 4, "\"v1\"").body(b"aaaa"),
        Reply::metadata("HEAD", 4, "\"v2\""),
        Reply::metadata("GET", 4, "\"v2\"").body(b"bbbb"),
    ]);
    assert_eq!(run(&server, &directory, true).status(), &Status::Success);
    assert_eq!(run(&server, &directory, true).status(), &Status::Success);
    assert_eq!(directory.bytes(), b"bbbb");
    assert!(server.finish()[3].header("range").is_none());
}

#[test]
fn head_unsupported_falls_back_to_get_and_its_disposition() {
    for status in [405, 501] {
        let directory = Directory::new();
        let server = Server::new(vec![
            Reply::new("HEAD", status).header("Content-Length", 0),
            Reply::new("GET", 200)
                .header("Content-Disposition", "attachment; filename=renamed.bin")
                .header("Content-Length", 4)
                .body(b"body"),
        ]);
        assert_eq!(run(&server, &directory, true).status(), &Status::Success);
        assert_eq!(
            fs::read(directory.path().join("renamed.bin")).unwrap(),
            b"body"
        );
        server.finish();
    }
}

#[test]
fn missing_weak_or_malformed_checkpoint_never_authorizes_range() {
    for damage in [
        "missing",
        "garbage",
        "weak",
        "missing-etag",
        "oversized",
        "partial-at-total",
    ] {
        let directory = Directory::new();
        let mut replies = prefix_replies();
        replies.extend([
            Reply::metadata("HEAD", 1000, "\"v1\""),
            Reply::metadata("GET", 1000, "\"v1\"").body(vec![b'z'; 1000]),
        ]);
        let server = Server::new(replies);
        make_prefix(&server, &directory);
        match damage {
            "missing" => fs::remove_file(directory.checkpoint()).unwrap(),
            "garbage" => fs::write(directory.checkpoint(), b"not a checkpoint").unwrap(),
            "weak" => {
                let saved = fs::read_to_string(directory.checkpoint()).unwrap();
                fs::write(directory.checkpoint(), saved.replace("\"v1\"", "W/\"v1\"")).unwrap();
            }
            "missing-etag" => {
                let saved = fs::read_to_string(directory.checkpoint()).unwrap();
                fs::write(directory.checkpoint(), saved.replace("\"v1\"", "")).unwrap();
            }
            "oversized" => fs::write(directory.file(), vec![b'x'; 1001]).unwrap(),
            "partial-at-total" => fs::write(directory.file(), vec![b'x'; 1000]).unwrap(),
            _ => unreachable!(),
        }
        assert_eq!(run(&server, &directory, true).status(), &Status::Success);
        assert_eq!(directory.bytes(), vec![b'z'; 1000]);
        assert!(
            server.finish()[3].header("range").is_none(),
            "damage={damage}"
        );
    }
}

#[test]
fn changed_url_identity_or_disabled_resume_forces_restart() {
    for disabled in [false, true] {
        let directory = Directory::new();
        let mut replies = prefix_replies();
        replies.extend([
            Reply::metadata("HEAD", 1000, "\"v1\""),
            Reply::metadata("GET", 1000, "\"v1\"").body(vec![b'z'; 1000]),
        ]);
        let mut server = Server::new(replies);
        make_prefix(&server, &directory);
        if !disabled {
            server.url.push_str("?new=token");
        }
        assert_eq!(
            run(&server, &directory, !disabled).status(),
            &Status::Success
        );
        assert_eq!(directory.bytes(), vec![b'z'; 1000]);
        assert!(server.finish()[3].header("range").is_none());
        if disabled {
            assert!(!directory.checkpoint().exists());
        }
    }
}

#[test]
fn weak_or_missing_etag_transfers_succeed_without_checkpoint() {
    for etag in [Some("W/\"v1\""), None] {
        let directory = Directory::new();
        let mut get = Reply::new("GET", 200)
            .header("Content-Length", 4)
            .body(b"body");
        if let Some(etag) = etag {
            get = get.header("ETag", etag);
        }
        let server = Server::new(vec![Reply::metadata("HEAD", 4, "\"v1\""), get]);
        assert_eq!(run(&server, &directory, true).status(), &Status::Success);
        assert_eq!(directory.bytes(), b"body");
        assert!(!directory.checkpoint().exists());
        server.finish();
    }
}

#[test]
fn unknown_total_uses_actual_committed_length_and_encoded_or_unsolicited_partial_fails() {
    let directory = Directory::new();
    let server = Server::new(vec![
        Reply::new("HEAD", 200),
        Reply::new("GET", 200)
            .header("ETag", "\"v1\"")
            .body(b"unknown length"),
    ]);
    let result = run(&server, &directory, true);
    assert_eq!(result.status(), &Status::Success);
    assert_eq!(result.size(), 14);
    server.finish();
    for response in [
        ranged(),
        Reply::new("GET", 200)
            .header("Content-Encoding", "gzip")
            .header("Content-Length", 0),
    ] {
        let server = Server::new(vec![Reply::new("HEAD", 200), response]);
        assert_failed(&run(&server, &directory, false));
        assert_eq!(directory.bytes(), b"unknown length");
        server.finish();
    }
}

#[test]
fn unsupported_range_units_never_authorize_append() {
    for unit in ["none", "bananas", "bytes, bananas"] {
        let directory = Directory::new();
        let mut replies = prefix_replies();
        replies.extend([
            Reply::metadata("HEAD", 1000, "\"v1\"").header("Accept-Ranges", unit),
            Reply::metadata("GET", 1000, "\"v1\"").body(vec![b'z'; 1000]),
        ]);
        let server = Server::new(replies);
        make_prefix(&server, &directory);
        assert_eq!(run(&server, &directory, true).status(), &Status::Success);
        assert_eq!(directory.bytes(), vec![b'z'; 1000]);
        assert!(server.finish()[3].header("range").is_none());
    }
}

#[test]
fn managed_header_conflicts_fail_before_network_or_mutation() {
    use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
    for (name, value) in [
        ("range", "bytes=3-"),
        ("if-range", "\"v1\""),
        ("accept-encoding", "gzip"),
    ] {
        let directory = Directory::new();
        fs::write(directory.file(), b"old").unwrap();
        let server = Server::new(vec![]);
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(value).unwrap(),
        );
        let downloader = Downloader::builder()
            .directory(directory.path())
            .headers(headers)
            .retries(0)
            .build();
        let download = Download::builder().url(&server.url).unwrap().build();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let summary = runtime
            .block_on(downloader.download(&[download]))
            .pop()
            .unwrap();
        assert_failed(&summary);
        assert_eq!(summary.size(), 3);
        assert_eq!(directory.bytes(), b"old");
        assert!(server.finish().is_empty());
    }
}

#[test]
fn missing_empty_output_is_recreated_instead_of_skipped() {
    let directory = Directory::new();
    let server = Server::new(vec![
        Reply::metadata("HEAD", 0, "\"v1\""),
        Reply::metadata("GET", 0, "\"v1\""),
        Reply::metadata("HEAD", 0, "\"v1\""),
        Reply::metadata("GET", 0, "\"v1\""),
    ]);
    assert_eq!(run(&server, &directory, true).status(), &Status::Success);
    assert!(directory.checkpoint().exists());
    fs::remove_file(directory.file()).unwrap();
    assert_eq!(run(&server, &directory, true).status(), &Status::Success);
    assert!(directory.file().exists());
    assert!(directory.bytes().is_empty());
    server.finish();
}

#[test]
fn redirect_changed_resolved_identity_cannot_append_old_prefix() {
    let directory = Directory::new();
    let mut replies = prefix_replies();
    replies.extend([
        Reply::metadata("HEAD", 1000, "\"v1\""),
        Reply::new("GET", 307)
            .header("Location", "/moved.bin")
            .header("Content-Length", 0),
        ranged(),
    ]);
    let server = Server::new(replies);
    make_prefix(&server, &directory);
    assert_failed(&run(&server, &directory, true));
    assert_eq!(directory.bytes(), vec![b'a'; 500]);
    assert_eq!(server.finish()[4].target, "/moved.bin");
}

#[test]
fn get_range_hint_is_optional_when_head_advertises_it() {
    let directory = Directory::new();
    let server = Server::new(vec![
        Reply::metadata("HEAD", 1000, "\"v1\""),
        Reply::new("GET", 200)
            .header("Content-Length", 1000)
            .header("ETag", "\"v1\"")
            .body(vec![b'a'; 500]),
        Reply::metadata("HEAD", 1000, "\"v1\""),
        ranged(),
    ]);
    let first = run(&server, &directory, true);
    assert_failed(&first);
    assert!(first.resumable());
    assert_eq!(run(&server, &directory, true).status(), &Status::Success);
    assert_eq!(
        directory.bytes(),
        [vec![b'a'; 500], vec![b'b'; 500]].concat()
    );
    server.finish();
}

#[test]
fn prepared_headless_context_reuses_resume_and_skip_behavior() {
    let directory = Directory::new();
    let mut replies = prefix_replies();
    replies.extend([
        Reply::metadata("HEAD", 1000, "\"v1\""),
        ranged(),
        Reply::metadata("HEAD", 1000, "\"v1\""),
    ]);
    let server = Server::new(replies);
    let download = Download::builder().url(&server.url).unwrap().build();
    let prepared = Downloader::builder()
        .directory(directory.path())
        .retries(0)
        .build()
        .prepare()
        .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        assert_failed(&prepared.download_one(&download).await);
        assert_eq!(directory.bytes(), vec![b'a'; 500]);
        assert_eq!(
            prepared.download_one(&download).await.status(),
            &Status::Success
        );
        assert_eq!(
            directory.bytes(),
            [vec![b'a'; 500], vec![b'b'; 500]].concat()
        );
        assert!(matches!(
            prepared.download_one(&download).await.status(),
            Status::Skipped(_)
        ));
    });
    server.finish();
}
